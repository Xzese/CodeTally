//! Integration coverage for the fast personal-activity refresh.
//!
//! The fixture installs a process-local fake `gh` and records every request.
//! Keep the environment guard in this file: PATH and the fake-`gh` variables
//! are process-global, so tests in this binary must not overlap.

use codetally_lib::db::Database;
use codetally_lib::github_sync;
use codetally_lib::models::{ActivityRelationship, AppSettings, Issue, PullRequest, Repository, SyncProgress};
use codetally_lib::personal_activity;
use codetally_lib::sync::AppState;
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
        "codetally-personal-activity-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn repository(id: &str, name: &str) -> Repository {
    Repository {
        github_id: id.into(),
        owner: "me".into(),
        name: name.into(),
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
    database
        .set_metadata("github_login", "me")
        .expect("cached GitHub login");
    database
        .set_metadata("github_discovered_at", "2026-09-10T00:00:00Z")
        .expect("completed initial repository import");

    // The fast backend must never enter discovery or LOC code. Cached clone
    // markers also make this fixture safe if a regression accidentally reads
    // the repository cache while refreshing activity.
    for mut repo in repositories.iter().cloned() {
        let local_path = cache_path.join(&repo.owner).join(&repo.name);
        std::fs::create_dir_all(local_path.join(".git")).expect("cached repository marker");
        repo.local_path = Some(local_path.to_string_lossy().into_owned());
        repo.loc_backfill_complete = true;
        repo.last_fetched_pushed_at = repo.pushed_at.clone();
        database.upsert_repository(&repo).expect("seed repository");
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

fn save_settings(database: &Database, settings: AppSettings) {
    codetally_lib::sync::save_app_settings(database, &settings).expect("save test settings");
}

struct FakeGh {
    root: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    scenario: String,
}

impl FakeGh {
    fn new(label: &str, scenario: &str) -> Self {
        let root = unique_root(label);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("fake gh directory");
        let log = root.join("gh.log");
        let gh = bin.join("gh");
        std::fs::write(&gh, fake_gh_script()).expect("fake gh script");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&gh)
                .expect("fake gh metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&gh, permissions).expect("fake gh executable");
        }
        std::fs::write(&log, "").expect("fake gh log");
        Self {
            root,
            bin,
            log,
            scenario: scenario.into(),
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
        restore_env("PATH", original_path);
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
scenario="${CODETALLY_FAKE_GH_SCENARIO:-initial}"
printf '%s\n' "$*" >> "$log"

if [ "${1:-}" = "--version" ]; then
  printf 'gh version 2.0.0\n'
  exit 0
fi
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  exit 0
fi

  if [ "${1:-}" = "search" ]; then
  kind="${2:-}"
  if [ "$scenario" = "rate" ]; then
    printf 'API rate limit exceeded\n' >&2
    exit 1
  fi
  if [ "$scenario" = "fail" ] && [ "$kind" = "issues" ] && printf '%s' "$*" | grep -q -- '--assignee @me'; then
    printf 'personal activity search failed\n' >&2
    exit 1
  fi
  if [ "$scenario" = "empty" ]; then
    printf '[]\n'
    exit 0
  fi
  if [ "$kind" = "prs" ]; then
    if printf '%s' "$*" | grep -q -- '--assignee @me'; then
      printf '%s\n' '[{"repository":{"name":"one","nameWithOwner":"me/one","owner":{"login":"me"}}},{"repository":{"name":"two","nameWithOwner":"me/two","owner":{"login":"me"}}}]'
    else
      printf '%s\n' '[{"repository":{"name":"one","nameWithOwner":"me/one","owner":{"login":"me"}}},{"repository":{"name":"two","nameWithOwner":"me/two","owner":{"login":"me"}}}]'
    fi
  else
    if printf '%s' "$*" | grep -q -- '--assignee @me'; then
      printf '%s\n' '[{"repository":{"name":"one","nameWithOwner":"me/one","owner":{"login":"me"}}},{"repository":{"name":"two","nameWithOwner":"me/two","owner":{"login":"me"}}}]'
    else
      printf '%s\n' '[{"repository":{"name":"one","nameWithOwner":"me/one","owner":{"login":"me"}}},{"repository":{"name":"two","nameWithOwner":"me/two","owner":{"login":"me"}}}]'
    fi
  fi
  exit 0
fi

# Previously matched repositories are refreshed through the normal complete
# activity importer. This response is deliberately empty so the test can
# observe which repository targets were retained without doing LOC work.
if [ "${1:-}" = "api" ] && [ "${2:-}" = "graphql" ]; then
  if [ "$scenario" = "fail-graphql" ]; then
    printf 'personal activity refresh failed\n' >&2
    exit 1
  fi
  is_pr=0
  if printf '%s' "$*" | grep -q 'items:pullRequests'; then is_pr=1; fi
  if [ "$scenario" = "initial" ]; then
    if [ "$is_pr" = "1" ]; then
      items='{"nodes":[{"number":1,"title":"cached PR","state":"OPEN","isDraft":false,"createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","mergedAt":null,"closedAt":null,"url":"https://github.com/me/one/pull/1","additions":1,"deletions":0,"changedFiles":1,"commits":{"nodes":[]}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}'
    else
      items='{"nodes":[{"number":2,"title":"cached issue","state":"OPEN","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-10T00:00:00Z","closedAt":null,"url":"https://github.com/me/one/issues/2","author":{"login":"me"},"labels":{"nodes":[]},"assignees":{"nodes":[]}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}'
    fi
  else
    items='{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}'
  fi
  sibling='{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}'
  printf '{"data":{"repository":{"openPRs":{"totalCount":0},"openIssues":{"totalCount":0},"sibling":%s,"items":%s}}}\n' "$sibling" "$items"
  exit 0
fi

printf 'unexpected fake gh invocation: %s\n' "$*" >&2
exit 1
"##
}

fn pull_requests(database: &Database, repository_id: i64) -> Vec<PullRequest> {
    database
        .pull_requests(Some(repository_id), None, 100)
        .expect("pull requests")
}

fn issues(database: &Database, repository_id: i64) -> Vec<Issue> {
    database
        .issues(Some(repository_id), None, 100)
        .expect("issues")
}

fn search_calls(calls: &str) -> Vec<&str> {
    calls.lines().filter(|line| line.starts_with("search ")).collect()
}

#[test]
fn personal_activity_uses_persisted_relationship_and_deduplicates_selected_repositories() {
    let root = unique_root("four-searches");
    let fake = FakeGh::new("four-searches", "initial");
    let repositories = [repository("repo-1", "one"), repository("repo-2", "two")];
    let (database, database_path) = seed_database(&root, &repositories);
    save_settings(
        &database,
        AppSettings {
            include_company_repositories: false,
            excluded_repository_ids: vec!["repo-2".into()],
            ..AppSettings::default()
        },
    );
    let selected = database
        .repositories()
        .expect("repositories")
        .into_iter()
        .find(|repo| repo.name == "one")
        .expect("selected repository");
    let state = state(database_path, &root);

    // The persisted default is authored-by-me, so the fast pass starts with
    // two searches. The other relationship settings select the documented
    // two-search assignee pass and the four-search union pass.
    let result = fake.with_path(|| personal_activity::sync(&state).expect("personal activity sync"));
    assert!(result.ok, "fast refresh failed: {:?}", result.errors);

    let calls = fake.calls();
    let searches = search_calls(&calls);
    assert_eq!(searches.len(), 2, "the default author relationship uses two searches: {calls}");
    for expected in [
        "search prs --author @me",
        "search issues --author @me",
    ] {
        assert!(
            searches.iter().any(|call| call.contains(expected)),
            "missing {expected} search in {calls}"
        );
    }
    assert!(searches.iter().all(|call| call.contains("--limit 100")), "fast search should use the bounded result limit: {calls}");
    assert!(!searches.iter().any(|call| call.contains("--assignee @me")), "default author relationship must not search assignees: {calls}");
    assert!(!calls.lines().any(|call| call.starts_with("repo list")), "fast refresh must not discover repositories: {calls}");
    assert!(!calls.lines().any(|call| call.contains("git ") || call.contains("tokei")), "fast refresh must not run LOC tooling: {calls}");

    // Every search names the selected repository (and an excluded one). A
    // backend that refreshes each matching result independently would issue
    // duplicate repository work; the target set must contain one selected
    // repository and exclude repo-2.
    let graphql_calls: Vec<_> = calls.lines().filter(|call| call.starts_with("api graphql")).collect();
    assert!(!graphql_calls.is_empty(), "the selected repository should be hydrated: {calls}");
    assert!(graphql_calls.iter().all(|call| call.contains("name=one")), "only the selected repository may be refreshed: {calls}");
    assert!(!graphql_calls.iter().any(|call| call.contains("name=two")), "excluded repositories must not be refreshed: {calls}");

    assert_eq!(pull_requests(&database, selected.id).len(), 1, "the selected repository should be hydrated once");
    assert_eq!(issues(&database, selected.id).len(), 1, "the selected repository should be hydrated once");
    assert!(database
        .pull_requests(
            Some(database.repositories().expect("repositories")[1].id),
            None,
            100,
        )
        .expect("excluded pull requests")
        .is_empty());
    assert_eq!(result.loc_repositories_synced, 0, "fast refresh must not scan LOC");
    assert_eq!(result.snapshots_created, 0, "fast refresh must not create snapshots");

    save_settings(
        &database,
        AppSettings {
            include_company_repositories: false,
            activity_relationship: ActivityRelationship::Assignee,
            excluded_repository_ids: vec!["repo-2".into()],
            ..AppSettings::default()
        },
    );
    fake.clear_calls();
    fake.with_path(|| personal_activity::sync(&state).expect("assignee personal activity sync"));
    let assignee_calls = fake.calls();
    let assignee_searches = search_calls(&assignee_calls);
    assert_eq!(assignee_searches.len(), 2, "assignee relationship uses two searches: {assignee_calls}");
    assert!(assignee_searches.iter().all(|call| call.contains("--assignee @me")), "assignee pass must use signed-in assignee searches: {assignee_calls}");
    assert!(assignee_searches.iter().all(|call| call.contains("--limit 100")), "assignee pass should use the bounded result limit: {assignee_calls}");

    save_settings(
        &database,
        AppSettings {
            include_company_repositories: false,
            activity_relationship: ActivityRelationship::AuthorOrAssignee,
            excluded_repository_ids: vec!["repo-2".into()],
            ..AppSettings::default()
        },
    );
    fake.clear_calls();
    fake.with_path(|| personal_activity::sync(&state).expect("combined personal activity sync"));
    let combined_calls = fake.calls();
    let combined_searches = search_calls(&combined_calls);
    assert_eq!(combined_searches.len(), 4, "combined relationship uses all four searches: {combined_calls}");
    assert!(combined_searches.iter().any(|call| call.contains("--author @me")), "combined pass must include author searches: {combined_calls}");
    assert!(combined_searches.iter().any(|call| call.contains("--assignee @me")), "combined pass must include assignee searches: {combined_calls}");

    save_settings(
        &database,
        AppSettings {
            include_company_repositories: false,
            activity_relationship: ActivityRelationship::Everyone,
            excluded_repository_ids: vec!["repo-2".into()],
            ..AppSettings::default()
        },
    );
    fake.clear_calls();
    let everyone = fake.with_path(|| personal_activity::sync(&state).expect("everyone personal activity sync"));
    assert!(everyone.ok, "everyone relationship should disable the fast pass: {:?}", everyone.errors);
    assert!(fake.calls().is_empty(), "everyone relationship must make no fast-search requests: {}", fake.calls());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn personal_activity_makes_no_requests_when_no_repositories_are_selected() {
    let root = unique_root("none-selected");
    let fake = FakeGh::new("none-selected", "initial");
    let (database, database_path) = seed_database(&root, &[repository("repo-1", "one")]);
    save_settings(
        &database,
        AppSettings {
            include_personal_repositories: false,
            include_company_repositories: false,
            ..AppSettings::default()
        },
    );
    let state = state(database_path, &root);

    let result = fake.with_path(|| personal_activity::sync(&state).expect("empty personal activity sync"));
    assert!(result.ok, "empty selection should be a successful no-op: {:?}", result.errors);
    assert!(fake.calls().is_empty(), "no selected repositories means no gh requests: {}", fake.calls());
    assert_eq!(result.activity_repositories_synced, 0);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn personal_activity_failure_preserves_previous_repository_set_and_cached_items() {
    let root = unique_root("failure-preserves");
    let mut fake = FakeGh::new("failure-preserves", "initial");
    let (database, database_path) = seed_database(&root, &[repository("repo-1", "one")]);
    let stored = database.repositories().expect("repositories").remove(0);
    let state = state(database_path, &root);

    fake.with_path(|| personal_activity::sync(&state).expect("initial personal activity sync"));
    let prior_prs = pull_requests(&database, stored.id).len();
    let prior_issues = issues(&database, stored.id).len();
    assert!(prior_prs + prior_issues > 0, "initial sync should seed cached activity");
    let prior_matches = database
        .metadata(personal_activity::MATCHES_METADATA_KEY)
        .expect("personal activity match metadata")
        .expect("initial match metadata");

    fake.scenario = "fail".into();
    fake.clear_calls();
    let failed = fake.with_path(|| personal_activity::sync(&state));
    assert!(failed.is_err() || !failed.as_ref().expect("failed sync result").ok, "failed search must be reported");
    assert_eq!(pull_requests(&database, stored.id).len(), prior_prs, "failed refresh must retain PRs");
    assert_eq!(issues(&database, stored.id).len(), prior_issues, "failed refresh must retain issues");
    assert_eq!(
        database
            .metadata(personal_activity::MATCHES_METADATA_KEY)
            .expect("personal activity match metadata")
            .expect("match metadata after failure"),
        prior_matches,
        "a failed search must preserve the prior repository match set"
    );

    // An empty successful search must still recheck the previous match. This
    // is observable without depending on the backend's metadata key name.
    fake.scenario = "empty".into();
    fake.clear_calls();
    fake.with_path(|| personal_activity::sync(&state).expect("recovery personal activity sync"));
    let recovery_calls = fake.calls();
    assert!(
        recovery_calls.lines().any(|call| call.starts_with("api graphql") && call.contains("name=one")),
        "the previous match must survive a failed cycle: {recovery_calls}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn personal_activity_rechecks_a_disappeared_repository_until_a_complete_success() {
    let root = unique_root("disappeared");
    let mut fake = FakeGh::new("disappeared", "initial");
    let (database, database_path) = seed_database(&root, &[repository("repo-1", "one")]);
    let stored = database.repositories().expect("repositories").remove(0);
    let state = state(database_path, &root);

    fake.with_path(|| personal_activity::sync(&state).expect("initial personal activity sync"));

    // Search results can disappear because the open PR or issue was closed.
    // Keep the repository in the work set for this successful cycle so the
    // complete activity importer can observe that transition.
    fake.scenario = "empty".into();
    fake.clear_calls();
    fake.with_path(|| personal_activity::sync(&state).expect("disappeared repository refresh"));
    let retained_calls = fake.calls();
    assert!(
        retained_calls.lines().any(|call| call.starts_with("api graphql") && call.contains("name=one")),
        "a disappeared repository must be rechecked before being retired: {retained_calls}"
    );
    assert_eq!(database.repositories().expect("repositories").len(), 1, "repository metadata remains cached");
    assert!(database.repository(stored.id).expect("repository lookup").is_some());

    // The successful complete cycle retires the old match. A later empty
    // cycle should still perform the four searches but no repository refresh.
    fake.clear_calls();
    fake.with_path(|| personal_activity::sync(&state).expect("post-retirement personal activity sync"));
    let retired_calls = fake.calls();
    assert_eq!(
        retired_calls.lines().filter(|call| call.starts_with("api graphql")).count(),
        0,
        "a repository retired by a complete sync must not be refreshed forever: {retired_calls}"
    );
    assert_eq!(search_calls(&retired_calls).len(), 2, "the default author relationship still runs two searches");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn personal_activity_search_cooldown_is_separate_and_blocks_the_next_pass() {
    let root = unique_root("search-cooldown");
    let fake = FakeGh::new("search-cooldown", "rate");
    let (database, database_path) = seed_database(&root, &[repository("repo-1", "one")]);
    let state = state(database_path, &root);

    let failed = fake.with_path(|| personal_activity::sync(&state));
    assert!(failed.is_err() || !failed.as_ref().expect("rate-limited result").ok, "rate-limited search must be reported");
    assert!(
        database
            .metadata(personal_activity::SEARCH_PAUSE_METADATA_KEY)
            .expect("personal activity pause metadata")
            .is_some(),
        "a rate-limited search must save a fast-path cooldown"
    );
    assert!(
        database
            .metadata(github_sync::PAUSE_KEY)
            .expect("regular GitHub pause metadata")
            .is_none(),
        "personal search quota must not pause regular activity sync"
    );
    assert!(
        database
            .metadata("github_quota_reset")
            .expect("regular GitHub quota metadata")
            .is_none(),
        "personal search quota must not write the regular GraphQL quota"
    );

    fake.clear_calls();
    let next = fake.with_path(|| personal_activity::sync(&state));
    assert!(next.is_err() || !next.as_ref().expect("cooldown result").ok, "the active cooldown should remain visible");
    assert!(fake.calls().is_empty(), "a fast search cooldown must suppress the next pass: {}", fake.calls());

    let _ = std::fs::remove_dir_all(root);
}
