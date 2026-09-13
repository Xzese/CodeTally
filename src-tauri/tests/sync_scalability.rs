//! Integration coverage for the durable, paginated GitHub activity importer.
//!
//! These tests use a process-local fake `gh`.  The real application still
//! invokes `gh` as a child process, so the fixture exercises the same command
//! boundary without making network requests.  Keep the PATH guard locked: the
//! other integration-test binary also has a discovery fixture that installs a
//! fake `gh`.

use chrono::{Duration, Utc};
use codetally_lib::db::Database;
use codetally_lib::github_sync;
use codetally_lib::models::{Repository, SyncProgress};
use codetally_lib::sync::{self, AppState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static GH_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn gh_env_lock() -> &'static Mutex<()> {
    GH_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

fn unique_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "codetally-sync-scalability-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn repository(id: &str, name: &str) -> Repository {
    Repository {
        github_id: id.to_string(),
        owner: "me".into(),
        name: name.to_string(),
        name_with_owner: format!("me/{name}"),
        url: format!("https://github.com/me/{name}"),
        ssh_url: format!("git@github.com:me/{name}.git"),
        default_branch: "main".into(),
        created_at: Some("2024-01-01T00:00:00Z".into()),
        github_updated_at: Some("2026-09-10T00:00:00Z".into()),
        pushed_at: Some("2026-09-10T00:00:00Z".into()),
        ..Repository::default()
    }
}

fn seed_database(root: &Path, repositories: &[Repository]) -> (Database, PathBuf) {
    std::fs::create_dir_all(root).expect("test database directory");
    let database_path = root.join("codetally.sqlite3");
    let cache_path = root.join("cache");
    let database = Database::new(&database_path);
    database.init().expect("database schema");

    // Keep activity-only tests away from Git and Tokei.  Discovery preserves
    // these cached fields on a repository upsert, and the recent sweep keeps
    // automatic LOC work out of sync_activity.
    database
        .set_metadata(
            sync::LOC_SWEEP_METADATA_KEY,
            &(Utc::now() - Duration::minutes(1)).to_rfc3339(),
        )
        .expect("recent LOC sweep");
    for mut repo in repositories.iter().cloned() {
        let local_path = cache_path.join(&repo.owner).join(&repo.name);
        std::fs::create_dir_all(local_path.join(".git")).expect("cached repository marker");
        repo.local_path = Some(local_path.to_string_lossy().into_owned());
        repo.loc_backfill_complete = true;
        repo.last_fetched_pushed_at = repo.pushed_at.clone();
        database
            .upsert_repository(&repo)
            .expect("seed repository");
    }
    (database, database_path)
}

fn state(database_path: PathBuf, root: &Path) -> AppState {
    AppState {
        db_path: database_path,
        cache_dir: root.join("cache"),
        progress: Arc::new(Mutex::new(SyncProgress::default())),
        job_lock: Arc::new(Mutex::new(())),
    }
}

struct FakeGh {
    root: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    scenario: String,
}

impl FakeGh {
    fn new(label: &str, scenario: &str, repositories: usize) -> Self {
        let root = unique_root(label);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("fake gh directory");
        let log = root.join("gh.log");
        let gh = bin.join("gh");
        std::fs::write(&gh, fake_gh_script())
            .expect("fake gh script");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&gh).expect("fake gh metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&gh, permissions).expect("fake gh executable");
        }
        std::fs::write(&log, "").expect("fake gh log");
        Self {
            root,
            bin,
            log,
            scenario: format!("{scenario}:{repositories}"),
        }
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.log).expect("fake gh log")
    }

    fn clear_calls(&self) {
        std::fs::write(&self.log, "").expect("clear fake gh log");
    }

    fn with_path<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _lock = gh_env_lock().lock().expect("fake gh environment lock");
        let original_path = std::env::var_os("PATH");
        let original_log = std::env::var_os("CODETALLY_FAKE_GH_LOG");
        let original_scenario = std::env::var_os("CODETALLY_FAKE_GH_SCENARIO");
        let mut paths = vec![self.bin.clone()];
        if let Some(path) = original_path.as_ref() {
            paths.extend(std::env::split_paths(path));
        }
        std::env::set_var(
            "PATH",
            std::env::join_paths(paths).expect("fake gh PATH"),
        );
        std::env::set_var("CODETALLY_FAKE_GH_LOG", &self.log);
        std::env::set_var("CODETALLY_FAKE_GH_SCENARIO", &self.scenario);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
        match original_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        restore_env("CODETALLY_FAKE_GH_LOG", original_log);
        restore_env("CODETALLY_FAKE_GH_SCENARIO", original_scenario);
        drop(_lock);
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

impl Drop for FakeGh {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn restore_env(name: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => std::env::set_var(name, value),
        None => std::env::remove_var(name),
    }
}

fn fake_gh_script() -> &'static str {
    r##"#!/bin/sh
set -eu

log="${CODETALLY_FAKE_GH_LOG:?}"
scenario="${CODETALLY_FAKE_GH_SCENARIO:-pages:1}"
mode="${scenario%%:*}"
repo_count="${scenario##*:}"
printf '%s\n' "$*" >> "$log"

if [ "${1:-}" = "--version" ]; then
  printf 'gh version 2.0.0\n'
  exit 0
fi
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  exit 0
fi
if [ "${1:-}" = "api" ] && [ "${2:-}" = "user" ]; then
  printf 'me\n'
  exit 0
fi
if printf '%s' "$*" | grep -q 'user/orgs'; then
  exit 0
fi
if [ "${1:-}" = "repo" ] && [ "${2:-}" = "list" ]; then
  if [ "$repo_count" = "2" ]; then
    printf '%s\n' '[{"id":"repo-1","name":"one","nameWithOwner":"me/one","url":"https://github.com/me/one","sshUrl":"git@github.com:me/one.git","isPrivate":false,"isFork":false,"isArchived":false,"stargazerCount":0,"forkCount":0,"defaultBranchRef":{"name":"main"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","pushedAt":"2026-09-10T00:00:00Z"},{"id":"repo-2","name":"two","nameWithOwner":"me/two","url":"https://github.com/me/two","sshUrl":"git@github.com:me/two.git","isPrivate":false,"isFork":false,"isArchived":false,"stargazerCount":0,"forkCount":0,"defaultBranchRef":{"name":"main"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","pushedAt":"2026-09-10T00:00:00Z"}]'
  else
    printf '%s\n' '[{"id":"repo-1","name":"one","nameWithOwner":"me/one","url":"https://github.com/me/one","sshUrl":"git@github.com:me/one.git","isPrivate":false,"isFork":false,"isArchived":false,"stargazerCount":0,"forkCount":0,"defaultBranchRef":{"name":"main"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","pushedAt":"2026-09-10T00:00:00Z"}]'
  fi
  exit 0
fi

if [ "${1:-}" = "api" ] && [ "${2:-}" = "graphql" ]; then
  graphql_calls=$(grep -c '^api graphql' "$log" || true)
  if [ "$mode" = "transient" ] && [ "$graphql_calls" -le 2 ]; then
    printf '%s\n' 'operation timed out' >&2
    exit 1
  fi
  if [ "$mode" = "retry-exhausted" ]; then
    printf '%s\n' 'connection reset by peer' >&2
    exit 1
  fi
  if [ "$mode" = "permanent" ]; then
    printf '%s\n' 'permission denied' >&2
    exit 1
  fi
  if [ "$mode" = "rate-first" ] && [ "$graphql_calls" = "2" ]; then
    printf '%s\n' '{"data":{"rateLimit":{"remaining":0,"resetAt":"2099-01-01T00:00:00Z"}},"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}'
    exit 0
  fi
  if [ "$mode" = "fail-page" ] && printf '%s' "$*" | grep -q 'AFTER_PR_OPEN_1'; then
    printf '%s\n' 'pagination page failed' >&2
    exit 1
  fi
  if [ "$mode" = "cursor-error" ] && printf '%s' "$*" | grep -q 'STALE_CURSOR'; then
    printf '%s\n' 'Invalid cursor' >&2
    exit 1
  fi

  page=1
  if printf '%s' "$*" | grep -Eq 'AFTER_(PR|ISSUE)_(OPEN|CLOSED)_1'; then
    page=2
  fi
  is_pr=0
  if printf '%s' "$*" | grep -q 'items:pullRequests'; then is_pr=1; fi
  scope=OPEN
  if ! printf '%s' "$*" | grep -q 'states:\[OPEN\]'; then scope=CLOSED; fi
  cursor="AFTER_ISSUE_${scope}_1"
  if [ "$is_pr" = "1" ]; then cursor="AFTER_PR_${scope}_1"; fi
  rate_field=
  if [ "$mode" = "low-final" ] && [ "$page" = "1" ] && [ "$is_pr" = "1" ]; then
    rate_field='"rateLimit":{"remaining":0,"resetAt":"2099-01-01T00:00:00Z"},'
  fi
  if [ "$mode" = "incremental" ]; then
    if [ "$is_pr" = "1" ]; then
      node='{"number":2,"title":"overlap refreshed","state":"OPEN","isDraft":false,"createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-09T00:00:00Z","mergedAt":null,"closedAt":null,"url":"https://github.com/me/one/pull/2","additions":2,"deletions":1,"changedFiles":1,"commits":{"nodes":[]}}'
      node2='{"number":3,"title":"new update","state":"OPEN","isDraft":false,"createdAt":"2026-09-09T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","mergedAt":null,"closedAt":null,"url":"https://github.com/me/one/pull/3","additions":3,"deletions":0,"changedFiles":1,"commits":{"nodes":[]}}'
      sibling='{"nodes":[{"number":2,"title":"overlap refreshed","state":"OPEN","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-09T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/2","author":{"login":"octocat"},"labels":{"nodes":[]},"assignees":{"nodes":[]}},{"number":3,"title":"new update","state":"OPEN","createdAt":"2026-09-09T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/3","author":{"login":"octocat"},"labels":{"nodes":[]},"assignees":{"nodes":[]}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}'
    else
      node='{"number":2,"title":"overlap refreshed","state":"OPEN","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-09T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/2","author":{"login":"octocat"},"labels":{"nodes":[]},"assignees":{"nodes":[]}}'
      node2='{"number":3,"title":"new update","state":"OPEN","createdAt":"2026-09-09T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/3","author":{"login":"octocat"},"labels":{"nodes":[]},"assignees":{"nodes":[]}}'
      sibling=null
    fi
    printf '{"data":{%s"repository":{"openPRs":{"totalCount":17},"openIssues":{"totalCount":23},"sibling":%s,"items":{"nodes":[%s,%s],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}\n' "${rate_field:-}" "${sibling:-null}" "$node" "$node2"
    exit 0
  fi
  sibling=null
  sibling_more=true
  if [ "$mode" = "low-final" ]; then sibling_more=false; fi
  if [ "$is_pr" = "1" ]; then
    if [ "$page" = "2" ]; then
      node='{"number":2,"title":"second page pull request","state":"OPEN","isDraft":false,"createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-08T00:00:00Z","mergedAt":null,"closedAt":null,"url":"https://github.com/me/one/pull/2","additions":2,"deletions":1,"changedFiles":1,"commits":{"nodes":[]}}'
      more=false
    else
      node='{"number":1,"title":"first page pull request","state":"OPEN","isDraft":false,"createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-09T00:00:00Z","mergedAt":null,"closedAt":null,"url":"https://github.com/me/one/pull/1","additions":1,"deletions":0,"changedFiles":1,"commits":{"nodes":[]}}'
      more=true
    fi
  else
    if [ "$page" = "2" ]; then
      node='{"number":2,"title":"second page issue","state":"OPEN","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-08T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/2","author":{"login":"octocat"},"labels":{"nodes":[]},"assignees":{"nodes":[]}}'
      more=false
    else
      node='{"number":1,"title":"first page issue","state":"OPEN","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-09T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/1","author":{"login":"octocat"},"labels":{"nodes":[]},"assignees":{"nodes":[]}}'
      more=true
    fi
  fi
  if printf '%s' "$*" | grep -q 'sibling:issues'; then
    sibling='{"nodes":[{"number":1,"title":"first page issue","state":"OPEN","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-09T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/1","author":{"login":"octocat"},"labels":{"nodes":[]},"assignees":{"nodes":[]}}],"pageInfo":{"hasNextPage":'"$sibling_more"',"endCursor":"AFTER_ISSUE_'"$scope"'_1"}}'
  fi
  if [ "$mode" = "low-final" ] && [ "$page" = "1" ] && [ "$is_pr" = "1" ]; then
    more=false
  fi
  printf '{"data":{%s"repository":{"openPRs":{"totalCount":17},"openIssues":{"totalCount":23},"sibling":%s,"items":{"nodes":[%s],"pageInfo":{"hasNextPage":%s,"endCursor":"%s"}}}}}\n' "${rate_field:-}" "${sibling:-null}" "$node" "$more" "$cursor"
  exit 0
fi

printf 'unexpected fake gh invocation: %s\n' "$*" >&2
exit 1
"##
}

fn pull_requests(database: &Database, repository_id: i64) -> Vec<codetally_lib::models::PullRequest> {
    database
        .pull_requests(Some(repository_id), None, 100)
        .expect("pull requests")
}

fn issues(database: &Database, repository_id: i64) -> Vec<codetally_lib::models::Issue> {
    database
        .issues(Some(repository_id), None, 100)
        .expect("issues")
}

#[test]
fn paginated_activity_import_keeps_all_pages_and_preserves_server_counts() {
    let root = unique_root("pages");
    let fake = FakeGh::new("pages", "pages", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database
        .repository(database.repositories().expect("repositories")[0].id)
        .expect("repository lookup")
        .expect("seeded repository");

    fake.with_path(|| {
        let imported = github_sync::sync_activity(&database, &stored)
            .expect("all GraphQL pages should import");
        assert!(imported.2, "the two-page feeds should finish in one cycle");
    });

    assert_eq!(pull_requests(&database, stored.id).len(), 2);
    assert_eq!(issues(&database, stored.id).len(), 2);
    assert_eq!(stored.id, database.repositories().expect("repositories")[0].id);
    let refreshed = database
        .repository(stored.id)
        .expect("repository lookup")
        .expect("stored repository");
    assert_eq!(refreshed.open_pr_count, 17, "feed totals must not be replaced by page length");
    assert_eq!(refreshed.open_issue_count, 23, "feed totals must not be replaced by page length");
    assert!(fake.calls().matches("api graphql").count() >= 4, "both paginated feeds should be requested: {}", fake.calls());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn transient_graphql_failures_are_retried_before_activity_import_fails() {
    let root = unique_root("transient-retry");
    let fake = FakeGh::new("transient-retry", "transient", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);

    let imported = fake.with_path(|| {
        github_sync::sync_activity(&database, &stored).expect("transient GraphQL failures should be retried")
    });
    assert!(imported.2, "activity feeds should finish after transient retries");
    assert_eq!(pull_requests(&database, stored.id).len(), 2);
    assert_eq!(issues(&database, stored.id).len(), 2);
    assert!(fake.calls().matches("api graphql").count() >= 6, "the fake should record two retries before the paginated feeds: {}", fake.calls());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn exhausted_transient_graphql_retries_preserve_cached_activity() {
    let root = unique_root("retry-exhausted");
    let mut fake = FakeGh::new("retry-exhausted", "pages", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);

    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("seed activity cache"));
    fake.scenario = "retry-exhausted:1".into();
    fake.clear_calls();
    let error = fake.with_path(|| {
        github_sync::sync_activity(&database, &stored).expect_err("exhausted transient failures should be reported")
    });
    assert!(error.to_string().contains("connection reset by peer"));
    assert_eq!(fake.calls().matches("api graphql").count(), 4, "retry exhaustion should make one initial request and three retries: {}", fake.calls());
    assert_eq!(pull_requests(&database, stored.id).len(), 2, "cached pull requests should survive retry exhaustion");
    assert_eq!(issues(&database, stored.id).len(), 2, "cached issues should survive retry exhaustion");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn permanent_graphql_failure_is_not_retried_and_preserves_cached_activity() {
    let root = unique_root("permanent-error");
    let mut fake = FakeGh::new("permanent-error", "pages", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);

    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("seed activity cache"));
    fake.scenario = "permanent:1".into();
    fake.clear_calls();
    let error = fake.with_path(|| {
        github_sync::sync_activity(&database, &stored).expect_err("permanent failures should be reported")
    });
    assert!(error.to_string().contains("permission denied"));
    assert_eq!(fake.calls().matches("api graphql").count(), 1, "permanent failures should not be retried: {}", fake.calls());
    assert_eq!(pull_requests(&database, stored.id).len(), 2, "cached pull requests should survive a permanent failure");
    assert_eq!(issues(&database, stored.id).len(), 2, "cached issues should survive a permanent failure");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn incremental_refresh_replays_overlap_and_keeps_new_updates() {
    let root = unique_root("incremental");
    let mut fake = FakeGh::new("incremental", "pages", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);

    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("initial import"));
    fake.scenario = "incremental:1".into();
    fake.clear_calls();
    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("incremental import"));

    let refreshed_prs = pull_requests(&database, stored.id);
    assert_eq!(refreshed_prs.len(), 3, "the new item must not be lost behind the watermark");
    assert_eq!(
        refreshed_prs.iter().find(|item| item.number == 2).expect("overlap item").title,
        "overlap refreshed"
    );
    assert_eq!(issues(&database, stored.id).len(), 3);
    assert!(fake.calls().matches("api graphql").count() > 0);

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn failed_page_leaves_page_cursor_durable_and_retryable() {
    let root = unique_root("failed-page");
    let mut fake = FakeGh::new("failed-page", "fail-page", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);

    fake.with_path(|| {
        let error = github_sync::sync_activity(&database, &stored)
            .expect_err("page two failure must be reported");
        assert!(error.to_string().contains("pagination page failed"));
    });
    let key = format!("github_activity_v2:{}:pullRequests:true", stored.id);
    let checkpoint = database.metadata(&key).expect("checkpoint metadata").expect("page one checkpoint");
    assert!(checkpoint.contains("AFTER_PR_OPEN_1"));
    let checkpoint: serde_json::Value = serde_json::from_str(&checkpoint).expect("checkpoint JSON");
    assert_eq!(checkpoint["after"], "AFTER_PR_OPEN_1");
    assert!(checkpoint["completed_at"].is_null(), "a failed page must not mark the feed complete");

    fake.scenario = "pages:1".into();
    fake.clear_calls();
    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("retry after failed page"));
    assert!(fake.calls().contains("AFTER_PR_OPEN_1"), "retry should resume at the durable page cursor");
    assert_eq!(pull_requests(&database, stored.id).len(), 2);

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn invalid_cursor_clears_only_the_stale_cursor_before_retrying_from_watermark() {
    let root = unique_root("invalid-cursor");
    let mut fake = FakeGh::new("invalid-cursor", "cursor-error", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);
    let key = format!("github_activity_v2:{}:pullRequests:true", stored.id);
    database
        .set_metadata(
            &key,
            r#"{"started_at":"2026-09-09T00:00:00Z","after":"STALE_CURSOR","completed_at":null}"#,
        )
        .expect("stale cursor checkpoint");

    fake.with_path(|| {
        let error = github_sync::sync_activity(&database, &stored)
            .expect_err("stale cursor should be reported once");
        assert!(error.to_string().contains("Invalid cursor"));
    });
    let reset: serde_json::Value = serde_json::from_str(
        &database.metadata(&key).expect("checkpoint metadata").expect("reset checkpoint"),
    )
    .expect("reset checkpoint JSON");
    assert!(reset["after"].is_null(), "the invalid cursor must not be retried forever");
    assert_eq!(
        reset["started_at"],
        "2026-09-09T00:00:00Z",
        "the initial cycle start must survive cursor recovery"
    );

    fake.scenario = "pages:1".into();
    fake.clear_calls();
    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("retry from watermark"));
    let calls = fake.calls();
    let first_call = calls
        .lines()
        .find(|call| call.starts_with("api graphql"))
        .expect("retry GraphQL call");
    assert!(!first_call.contains("STALE_CURSOR"));
    assert_eq!(pull_requests(&database, stored.id).len(), 2);

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn low_quota_final_page_keeps_fetched_counts_but_skips_loc_work() {
    let root = unique_root("low-quota");
    let fake = FakeGh::new("low-quota", "low-final", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);
    let sync_state = state(database_path.clone(), &root);

    let result = fake
        .with_path(|| sync::sync_one(&sync_state, stored.id))
        .expect("sync result should preserve a partial low-quota import");
    assert!(!result.ok);
    assert_eq!(result.pull_requests_synced, 1, "durable PR page count should be reported");
    assert_eq!(result.issues_synced, 1, "durable sibling issue page count should be reported");
    assert_eq!(result.loc_repositories_synced, 0, "a paused final response must not start LOC work");
    assert!(result.errors.iter().any(|error| error.contains("paused until")));

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn rate_limit_stops_following_repositories_and_pause_survives_restart() {
    let root = unique_root("rate-limit");
    let fake = FakeGh::new("rate-limit", "rate-first", 2);
    let repositories = [repository("repo-1", "one"), repository("repo-2", "two")];
    let (database, database_path) = seed_database(&root, &repositories);
    let first_state = state(database_path.clone(), &root);

    let first_result = fake.with_path(|| sync::sync_activity(&first_state));
    let first_message = match first_result {
        Ok(result) => format!("{} {}", result.message, result.errors.join(" ")),
        Err(error) => error.to_string(),
    };
    assert!(first_message.to_ascii_lowercase().contains("paused"));
    let calls = fake.calls();
    assert!(calls.lines().any(|call| call.contains("name=one")), "first repository should reach GraphQL");
    assert!(!calls.lines().any(|call| call.contains("name=two")), "rate limit must prevent following repositories: {calls}");
    assert!(database
        .metadata(github_sync::PAUSE_KEY)
        .expect("pause metadata")
        .is_some());

    fake.clear_calls();
    let restarted_state = state(database_path.clone(), &root);
    let paused = fake.with_path(|| sync::sync_activity(&restarted_state));
    let paused_error = paused.expect_err("restart should remain paused");
    assert!(paused_error.to_string().contains("paused until"));
    assert!(fake.calls().is_empty(), "a paused refresh must not invoke gh: {}", fake.calls());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn repository_discovery_is_reused_within_one_hour() {
    let root = unique_root("discovery-cache");
    let fake = FakeGh::new("discovery-cache", "pages", 1);
    let repo = repository("repo-1", "one");
    let (_database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let first_state = state(database_path.clone(), &root);

    fake.with_path(|| sync::sync_activity(&first_state).expect("initial refresh"));
    fake.clear_calls();
    fake.with_path(|| sync::sync_activity(&first_state).expect("cached discovery refresh"));
    assert!(!fake.calls().lines().any(|call| call.starts_with("repo list")), "fresh discovery should be reused for one hour: {}", fake.calls());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn excluded_repositories_are_skipped_by_activity_sync_but_selected_repositories_are_queried() {
    let root = unique_root("repository-inclusion");
    let fake = FakeGh::new("repository-inclusion", "pages", 2);
    let repositories = [repository("repo-1", "one"), repository("repo-2", "two")];
    let (database, database_path) = seed_database(&root, &repositories);
    database
        .set_metadata("github_login", "me")
        .expect("cached GitHub login");
    database
        .set_metadata("github_discovered_at", &Utc::now().to_rfc3339())
        .expect("cached discovery timestamp");
    sync::save_app_settings(
        &database,
        &codetally_lib::models::AppSettings {
            excluded_repository_ids: vec!["repo-2".into()],
            ..codetally_lib::models::AppSettings::default()
        },
    )
    .expect("save repository exclusion");
    let state = state(database_path.clone(), &root);

    let result = fake.with_path(|| sync::sync_activity(&state).expect("selected repository refresh"));
    assert!(result.ok);
    assert_eq!(result.activity_repositories_synced, 1);
    let calls = fake.calls();
    assert!(calls.lines().any(|call| call.contains("name=one")), "selected repository should reach GitHub: {calls}");
    assert!(!calls.lines().any(|call| call.contains("name=two")), "excluded repository must not reach GitHub: {calls}");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn direct_sync_and_backfill_calls_reject_an_excluded_repository_before_scanning() {
    let root = unique_root("disabled-direct-calls");
    let fake = FakeGh::new("disabled-direct-calls", "pages", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    database
        .set_metadata("github_login", "me")
        .expect("cached GitHub login");
    sync::save_app_settings(
        &database,
        &codetally_lib::models::AppSettings {
            excluded_repository_ids: vec!["repo-1".into()],
            ..codetally_lib::models::AppSettings::default()
        },
    )
    .expect("save repository exclusion");
    let stored = database.repositories().expect("repositories").remove(0);
    let state = state(database_path.clone(), &root);

    fake.with_path(|| {
        let sync_error = sync::sync_one(&state, stored.id).expect_err("disabled sync should be rejected");
        assert!(sync_error.to_string().contains("Repository is disabled in settings"));
        let backfill_error = sync::backfill_one(&state, stored.id).expect_err("disabled backfill should be rejected");
        assert!(backfill_error.to_string().contains("Repository is disabled in settings"));
    });
    let calls = fake.calls();
    assert_eq!(calls.lines().filter(|call| call.contains("api graphql")).count(), 0, "disabled direct calls must not scan GitHub: {calls}");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}
