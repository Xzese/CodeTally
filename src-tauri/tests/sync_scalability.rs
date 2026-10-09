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
use codetally_lib::models::{AppSettings, PullRequest, Repository, Snapshot, SyncProgress};
use codetally_lib::sync::{self, AppState};
use codetally_lib::kanban;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static GH_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn gh_env_lock() -> &'static Mutex<()> {
    GH_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn failed_link_refresh_retains_the_last_complete_cache() {
    let root = unique_root("linked-work-offline");
    let (database, database_path) = seed_database(&root, &[repository("repo-1", "one")]);
    database.set_metadata("github_login", "me").expect("account");
    sync::save_app_settings(&database, &AppSettings { kanban_enabled: true, ..AppSettings::default() }).expect("board enabled");
    kanban::remember_repository(&database, "repo-1").expect("account repository");
    let repo_id = database.repositories().expect("repositories")[0].id;
    database.upsert_pull_request(&PullRequest { repository_id: repo_id, number: 7, title: "Fix bug".into(), state: "OPEN".into(), created_at: "2026-01-01T00:00:00Z".into(), updated_at: "2026-01-02T00:00:00Z".into(), url: "https://github.com/me/one/pull/7".into(), ..PullRequest::default() }).expect("PR");
    let key = kanban::item_key("repo-1", "pr", 7, None);
    let cached = serde_json::json!([{"kind":"issue","repository":"me/one","number":9,"title":"Linked issue","state":"OPEN","url":"https://github.com/me/one/issues/9"}]);
    rusqlite::Connection::open(&database_path).expect("database connection").execute(
        "INSERT INTO kanban_links_cache(account_scope,item_key,links_json,partial,updated_at) VALUES (?1,?2,?3,0,?4)",
        rusqlite::params!["github.com:me", key, cached.to_string(), "2026-01-01T00:00:00Z"],
    ).expect("cached links");
    let fake = FakeGh::new("linked-work-offline", "permanent", 1);
    let result = fake.with_path(|| kanban::load_links(&database, &key).expect("stale links should remain available"));
    assert!(result.partial);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].number, 9);
    assert_eq!(kanban::cached_links(&database, &key).expect("persistent cache").items.len(), 1);
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
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
        database.upsert_repository(&repo).expect("seed repository");
    }
    (database, database_path)
}

fn state(database_path: PathBuf, root: &Path) -> AppState {
    AppState {
        db_path: database_path,
        cache_dir: root.join("cache"),
        progress: Arc::new(Mutex::new(SyncProgress::default())),
        dashboard_cache: Arc::new(Mutex::new(Default::default())),
        job_lock: Arc::new(Mutex::new(())),
    }
}

#[cfg(unix)]
fn install_command_guard(bin: &Path, name: &str, log: &Path) {
    let script = bin.join(name);
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '$0 $*' >> '{}'\nexit 0\n",
            log.display()
        ),
    )
    .expect("write command guard");
    let mut permissions = std::fs::metadata(&script)
        .expect("command guard metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(script, permissions).expect("make command guard executable");
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
        std::env::set_var("PATH", std::env::join_paths(paths).expect("fake gh PATH"));
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
    r##"#!/usr/bin/env python3
import datetime
import json
import os
import re
import shlex
import sys

args = sys.argv[1:]
log = os.environ["CODETALLY_FAKE_GH_LOG"]
scenario = os.environ.get("CODETALLY_FAKE_GH_SCENARIO", "pages:1")
mode, _, repo_count = scenario.partition(":")
repo_count = int(repo_count or "1")
with open(log, "a", encoding="utf-8") as stream:
    stream.write(shlex.join(args) + "\n")

def emit(value):
    print(json.dumps(value, separators=(",", ":")))

def repository(repo_id, name, owner="me", stars=0, forks=0, language=None):
    return {
        "id": repo_id, "name": name, "nameWithOwner": owner + "/" + name,
        "url": "https://github.com/" + owner + "/" + name,
        "sshUrl": "git@github.com:" + owner + "/" + name + ".git",
        "isPrivate": False, "isFork": False, "isArchived": False,
        "stargazerCount": stars, "forkCount": forks,
        "defaultBranchRef": {"name": "main"},
        "primaryLanguage": {"name": language} if language else None,
        "createdAt": "2024-01-01T00:00:00Z",
        "updatedAt": "2026-09-10T00:00:00Z",
        "pushedAt": "2026-09-10T00:00:00Z",
    }

def activity_repository(repo_id):
    if mode == "rename-transfer" and repo_id == "repo-1":
        return repository(repo_id, "renamed", "new-owner", 41, 7, "Go")
    if mode == "rename-transfer" and repo_id == "repo-2":
        return repository(repo_id, "one")
    name = {"repo-1": "one", "repo-2": "two"}.get(repo_id)
    if name is None:
        name = repo_id[5:] if repo_id and repo_id.startswith("repo-") else (repo_id or "unknown")
    return repository(repo_id, name)

def variables_from_args():
    values = {}
    for index, value in enumerate(args[:-1]):
        if value in ("-f", "-F") and "=" in args[index + 1]:
            key, raw = args[index + 1].split("=", 1)
            values[key] = raw
    return values

if args == ["--version"]:
    print("gh version 2.0.0")
    raise SystemExit(0)
if args[:2] == ["auth", "status"]:
    raise SystemExit(0)
if args[:2] == ["api", "user"]:
    print("me")
    raise SystemExit(0)
if "user/orgs" in " ".join(args):
    if mode == "auth-org":
        print("gh: Bad credentials (HTTP 401)", file=sys.stderr)
        raise SystemExit(1)
    raise SystemExit(0)
if args[:2] == ["repo", "list"]:
    repos = [repository("repo-1", "one")]
    if repo_count == 2:
        repos.append(repository("repo-2", "two"))
    emit(repos)
    raise SystemExit(0)

if args[:2] == ["api", "graphql"]:
    query = next((item.split("=", 1)[1] for item in args if item.startswith("query=")), "")
    variables = variables_from_args()
    with open(log, "r", encoding="utf-8") as stream:
        graphql_calls = sum(line.startswith("api graphql ") for line in stream)

    if mode == "transient" and graphql_calls <= 2:
        print("operation timed out", file=sys.stderr)
        raise SystemExit(1)
    if mode == "http-transient" and graphql_calls <= 2:
        status = 502 if graphql_calls == 1 else 503
        message = "Bad Gateway" if status == 502 else "Service Unavailable"
        with open(log, "a", encoding="utf-8") as stream:
            stream.write("HTTP " + str(status) + " " + message + "\n")
        if status == 502:
            # The installed app's exact failure had only stderr status text.
            print("gh: HTTP 502", file=sys.stderr)
        else:
            # Also exercise retry classification from response headers alone.
            print("HTTP/2.0 503 Service Unavailable\n\n{}")
        raise SystemExit(1)
    if mode == "retry-exhausted":
        print("HTTP/2.0 503 Service Unavailable\n\n{}")
        print("gh: HTTP 503 Service Unavailable", file=sys.stderr)
        raise SystemExit(1)
    if mode == "permanent":
        print("permission denied", file=sys.stderr)
        raise SystemExit(1)
    if mode == "missing-repository" and variables.get("repositoryId") == "repo-1":
        emit({"data": {"repository": None}, "errors": [{"type": "NOT_FOUND", "path": ["repository"], "message": "Repository not found"}]})
        print("gh: Could not resolve to a Repository with the name 'me/one'.", file=sys.stderr)
        raise SystemExit(1)

    if mode == "auth-missing":
        print("To get started with GitHub CLI, please run: gh auth login", file=sys.stderr)
        raise SystemExit(4)
    if mode == "auth-expired" or (mode == "auth-late" and variables.get("prOpenAfter", "").startswith("CURSOR_")):
        print('HTTP/2.0 401 Unauthorized\r\nContent-Type: application/json\r\n\r\n{"message":"Bad credentials"}')
        print("gh: Bad credentials (HTTP 401)", file=sys.stderr)
        raise SystemExit(1)
    if mode == "permission-denied" and variables.get("repositoryId") == "repo-1":
        print("gh: Resource not accessible (HTTP 403)", file=sys.stderr)
        raise SystemExit(1)

    if mode == "rate-first" and graphql_calls >= 2:
        emit({"data": {"rateLimit": {"remaining": 0, "resetAt": "2099-01-01T00:00:00Z"}},
              "errors": [{"type": "RATE_LIMITED", "message": "API rate limit exceeded"}]})
        raise SystemExit(0)

    aliases = re.findall(r"\b(prOpen|issueOpen|prClosed|issueClosed):(pullRequests|issues)\(", query)
    if mode == "null-repository" and aliases:
        emit({"data": {"rateLimit": {"remaining": 5000}, "repository": None}})
        raise SystemExit(0)
    if mode == "missing-repository-field":
        emit({"data": {"rateLimit": {"remaining": 5000, "resetAt": "2099-01-01T00:00:00Z"}}})
        raise SystemExit(0)
    if not aliases:
        start = 100 if variables.get("after") == "OWNED_100" else 0
        if mode == "discovery-failed" and start:
            print("repository discovery page failed", file=sys.stderr)
            raise SystemExit(1)
        repos = [repository("repo-" + str(index + 1), "one" if index == 0 else "two" if index == 1 else "extra" + str(index + 1))
                 for index in range(start, min(start + 100, repo_count))]
        if mode == "discovery-failed" and repos:
            repos[0]["isArchived"] = True
        more = start + 100 < repo_count
        emit({"data": {"rateLimit": {"remaining": 5000, "resetAt": "2099-01-01T00:00:00Z"},
                        "viewer": {"login": "other" if mode == "discovery-account" and start else "me", "repositories": {"nodes": repos,
                            "pageInfo": {"hasNextPage": more, "endCursor": "OWNED_100" if more else None}}}}})
        raise SystemExit(0)

    if mode == "cursor-error" and any(variables.get(alias + "After") == "STALE_CURSOR" for alias, _ in aliases):
        emit({"data": {"rateLimit": {"remaining": 5000, "resetAt": "2099-01-01T00:00:00Z"}},
              "errors": [{"type": "INVALID_CURSOR", "message": "Invalid cursor", "path": ["repository", "prOpen"]}]})
        raise SystemExit(0)
    if mode == "cursor-no-path" and any(variables.get(alias + "After") == "STALE_CURSOR" for alias, _ in aliases):
        print("Invalid cursor", file=sys.stderr)
        raise SystemExit(1)
    if mode == "fail-page" and variables.get("prOpenAfter") == "CURSOR_prOpen_1":
        print("pagination page failed", file=sys.stderr)
        raise SystemExit(1)

    timestamp = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    fixed_timestamp = "2026-09-10T00:00:00Z"

    def make_node(alias, kind, number, title, state, updated, ci=None):
        closed = alias.endswith("Closed")
        node = {
            "id": ("PR_NODE_" if kind == "pr" else "ISSUE_NODE_") + str(number),
            "stateReason": "COMPLETED" if kind == "issue" else None,
            "author": {"login": "octocat"}, "assignees": {"nodes": []},
            "number": number, "title": title, "state": state,
            "createdAt": "2026-09-01T00:00:00Z", "updatedAt": updated,
            "closedAt": updated if closed else None,
            "url": "https://github.com/me/one/" + ("pull" if kind == "pr" else "issues") + "/" + str(number),
        }
        if kind == "pr":
            node.update({"isDraft": False, "mergedAt": None, "additions": number,
                         "deletions": 0, "changedFiles": 1,
                         "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": ci or "SUCCESS"}}}]}})
        else:
            node.update({"author": {"login": "octocat"}, "labels": {"nodes": []},
                         "assignees": {"nodes": []}})
        return node

    def filter_nodes(alias, nodes):
        since = variables.get("issueClosedSince") if alias == "issueClosed" else None
        if since:
            cutoff = datetime.datetime.fromisoformat(since.replace("Z", "+00:00"))
            return [node for node in nodes if datetime.datetime.fromisoformat(
                node["updatedAt"].replace("Z", "+00:00")) >= cutoff]
        return nodes

    def page_for(alias):
        after = variables.get(alias + "After", "")
        cursor_match = re.fullmatch(r"CURSOR_" + re.escape(alias) + r"_(\d+)", after)
        page_number = int(cursor_match.group(1)) + 1 if cursor_match else 1
        kind = "pr" if alias.startswith("pr") else "issue"
        closed = alias.endswith("Closed")
        base = 100 if closed and kind == "pr" else 200 if closed else 0

        if mode == "incremental":
            if alias == "prOpen":
                records = [(8, "same timestamp CI refresh", "OPEN", fixed_timestamp, "FAILURE"),
                           (9, "new pull request", "OPEN", timestamp, "SUCCESS")]
            elif alias == "prClosed":
                records = [(7, "transitioned to closed", "CLOSED", fixed_timestamp, "SUCCESS")]
            elif alias == "issueOpen":
                records = [(2, "same timestamp issue refresh", "OPEN", fixed_timestamp, None),
                           (3, "new issue", "OPEN", timestamp, None)]
            elif alias == "issueClosed":
                records = [(51, "recent closed issue refresh", "CLOSED", timestamp, None)]
            else:
                records = []
            nodes = [make_node(alias, kind, number, title, state, updated, ci)
                     for number, title, state, updated, ci in records]
            return filter_nodes(alias, nodes), False, None

        if mode == "low-final":
            more = alias == "prOpen"
            nodes = [make_node(alias, kind, base + 1, "low quota durable page", "CLOSED" if closed else "OPEN", timestamp)]
            return filter_nodes(alias, nodes), more, "CURSOR_" + alias + "_1" if more else None

        if mode == "asym":
            last_page = {"prOpen": 2, "issueOpen": 2, "prClosed": 1, "issueClosed": 2}[alias]
            more = page_number < last_page
        elif mode == "many":
            more = page_number < 7
        elif mode in ("single", "cursor-error"):
            more = False
        else:
            more = page_number < 2

        number = base + page_number
        nodes = [make_node(alias, kind, number, alias + " page " + str(page_number),
                           "CLOSED" if closed else "OPEN", timestamp)]
        return filter_nodes(alias, nodes), more, "CURSOR_" + alias + "_" + str(page_number) if more else None

    repository_id = variables.get("repositoryId")
    repo_data = activity_repository(repository_id)
    if mode == "mismatched-repository" and repository_id == "repo-1":
        repo_data["id"] = "different-github-node"
    data = {"rateLimit": {"remaining": 0 if mode == "low-final" else 5000,
                          "resetAt": "2099-01-01T00:00:00Z"},
            "repository": repo_data}
    data["repository"]["openPRs"] = {"totalCount": 17}
    data["repository"]["openIssues"] = {"totalCount": 23}
    for alias, _connection in aliases:
        nodes, more, cursor = page_for(alias)
        data["repository"][alias] = {"nodes": nodes,
                                     "pageInfo": {"hasNextPage": more, "endCursor": cursor}}
    emit({"data": data})
    raise SystemExit(0)

print("unexpected fake gh invocation: " + shlex.join(args), file=sys.stderr)
raise SystemExit(1)
"##
}
fn pull_requests(
    database: &Database,
    repository_id: i64,
) -> Vec<codetally_lib::models::PullRequest> {
    database
        .pull_requests(Some(repository_id), None, 100)
        .expect("pull requests")
}

fn issues(database: &Database, repository_id: i64) -> Vec<codetally_lib::models::Issue> {
    database
        .issues(Some(repository_id), None, 100)
        .expect("issues")
}

fn activity_calls(fake: &FakeGh) -> Vec<String> {
    fake.calls()
        .lines()
        .filter(|call| {
            call.starts_with("api graphql ")
                && [
                    "prOpen:pullRequests",
                    "issueOpen:issues",
                    "prClosed:pullRequests",
                    "issueClosed:issues",
                ]
                .iter()
                .any(|alias| call.contains(alias))
        })
        .map(str::to_owned)
        .collect()
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

    assert_eq!(
        pull_requests(&database, stored.id).len(),
        4,
        "open and closed PR feeds must both persist"
    );
    assert_eq!(
        issues(&database, stored.id).len(),
        4,
        "open and closed issue feeds must both persist"
    );
    assert_eq!(
        stored.id,
        database.repositories().expect("repositories")[0].id
    );
    let refreshed = database
        .repository(stored.id)
        .expect("repository lookup")
        .expect("stored repository");
    assert_eq!(
        refreshed.open_pr_count, 17,
        "feed totals must not be replaced by page length"
    );
    assert_eq!(
        refreshed.open_issue_count, 23,
        "feed totals must not be replaced by page length"
    );
    let calls = activity_calls(&fake);
    assert_eq!(
        calls.len(),
        2,
        "all four two-page feeds should share exactly two request waves: {}",
        fake.calls()
    );
    for call in &calls {
        for alias in [
            "prOpen:pullRequests",
            "issueOpen:issues",
            "prClosed:pullRequests",
            "issueClosed:issues",
        ] {
            assert!(
                call.contains(alias),
                "each wave should include every unfinished feed ({alias}): {call}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn activity_refresh_tracks_repository_rename_and_transfer_by_node_identity() {
    let root = unique_root("rename-transfer");
    let fake = FakeGh::new("rename-transfer", "rename-transfer", 2);
    let repo = repository("repo-1", "one");
    let mut name_reuse = repository("repo-2", "other");
    name_reuse.owner = "reused-owner".into();
    name_reuse.name_with_owner = "reused-owner/other".into();
    name_reuse.url = "https://github.com/reused-owner/other".into();
    name_reuse.ssh_url = "git@github.com:reused-owner/other.git".into();
    let (database, database_path) = seed_database(&root, &[repo, name_reuse]);
    let stored_repos = database.repositories().expect("repositories");
    let stored = stored_repos.iter().find(|repo| repo.github_id == "repo-1").expect("original node").clone();
    let reused_name = stored_repos.iter().find(|repo| repo.github_id == "repo-2").expect("repository reusing old name").clone();
    let original_path = stored.local_path.clone().expect("seeded clone path");
    database.mark_sync(stored.id, Some("previous refresh error")).expect("seed sync error");
    database.upsert_snapshot(&Snapshot {
        id: 0,
        repository_id: stored.id,
        commit_sha: "before-transfer".into(),
        commit_date: "2026-08-01T00:00:00Z".into(),
        snapshot_date: "2026-08-01T00:00:00Z".into(),
        total_loc: 123,
        source_loc: 100,
        test_loc: 20,
        docs_loc: 3,
        created_at: "2026-08-01T00:00:00Z".into(),
    }).expect("seed history");
    database.set_metadata("github_discovered_at", &Utc::now().to_rfc3339()).expect("skip discovery");
    let app = state(database_path.clone(), &root);

    let result = fake.with_path(|| sync::sync_activity(&app).expect("renamed repository refresh"));
    assert!(result.ok, "refresh should succeed: {:?}", result.errors);
    assert_eq!(result.activity_repositories_synced, 2);
    let calls = fake.calls();
    assert!(calls.contains("repositoryId=repo-1"), "activity lookup should address the saved node ID: {calls}");
    assert!(calls.contains("repositoryId=repo-2"), "the repository reusing the old name has a separate node ID: {calls}");
    assert!(!calls.contains("name=one"), "activity lookup should not address the stale repository name: {calls}");

    let refreshed = database.repository(stored.id).expect("repository lookup").expect("same repository row");
    assert_eq!(refreshed.id, stored.id);
    assert_eq!(refreshed.github_id, "repo-1");
    assert_eq!(refreshed.owner, "new-owner");
    assert_eq!(refreshed.name, "renamed");
    assert_eq!(refreshed.name_with_owner, "new-owner/renamed");
    assert_eq!(refreshed.url, "https://github.com/new-owner/renamed");
    assert_eq!(refreshed.primary_language.as_deref(), Some("Go"));
    assert_eq!(refreshed.star_count, 41);
    assert_eq!(refreshed.fork_count, 7);
    assert_eq!(refreshed.local_path.as_deref(), Some(original_path.as_str()));
    assert_eq!(refreshed.last_error, None);
    assert!(pull_requests(&database, stored.id).iter().all(|item| item.repository == "new-owner/renamed"));
    assert!(issues(&database, stored.id).iter().all(|item| item.repository == "new-owner/renamed"));
    assert!(database.history(Some(stored.id)).expect("retained history").iter().any(|point| point.snapshot_date == "2026-08-01"));
    let reused_name = database.repository(reused_name.id).expect("repository lookup").expect("reused old name row");
    assert_eq!(reused_name.github_id, "repo-2");
    assert_eq!(reused_name.name_with_owner, "me/one", "the old name can now identify a different node");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn missing_or_mismatched_activity_repository_identity_preserves_cached_data() {
    use codetally_lib::error::AppError;

    let root = unique_root("invalid-activity-identity");
    let mut fake = FakeGh::new("invalid-activity-identity", "pages", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);
    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("seed activity cache"));
    let old_repository = database.repository(stored.id).expect("repository lookup").expect("cached repository");
    let old_repository_json = serde_json::to_value(&old_repository).expect("repository JSON");
    let old_prs = serde_json::to_value(pull_requests(&database, stored.id)).expect("cached PR JSON");
    let old_issues = serde_json::to_value(issues(&database, stored.id)).expect("cached issue JSON");
    let cursor_keys = [
        format!("github_activity_v2:{}:pullRequests:true", stored.id),
        format!("github_activity_v2:{}:issues:true", stored.id),
        format!("github_activity_v2:{}:pullRequests:false", stored.id),
        format!("github_activity_v2:{}:issues:false", stored.id),
    ];
    let old_cursors: Vec<_> = cursor_keys.iter().map(|key| database.metadata(key).expect("activity cursor")).collect();

    for (mode, expected) in [
        ("missing-repository", "unavailable"),
        ("null-repository", "unavailable"),
        ("missing-repository-field", "invalid"),
        ("mismatched-repository", "invalid"),
    ] {
        fake.scenario = format!("{mode}:1");
        fake.clear_calls();
        let error = fake.with_path(|| github_sync::sync_activity(&database, &stored).expect_err("invalid repository identity should fail"));
        if expected == "unavailable" {
            assert!(matches!(error, AppError::RepositoryUnavailable), "{mode}: {error:?}");
        } else {
            assert!(matches!(error, AppError::InvalidArgument(_)), "{mode}: {error:?}");
        }
        assert_eq!(serde_json::to_value(database.repository(stored.id).expect("repository lookup").unwrap()).expect("repository JSON"), old_repository_json, "{mode} must preserve canonical metadata");
        assert_eq!(serde_json::to_value(pull_requests(&database, stored.id)).expect("cached PR JSON"), old_prs, "{mode} must preserve cached PRs");
        assert_eq!(serde_json::to_value(issues(&database, stored.id)).expect("cached issue JSON"), old_issues, "{mode} must preserve cached issues");
        for (key, old_cursor) in cursor_keys.iter().zip(&old_cursors) {
            assert_eq!(database.metadata(key).expect("activity cursor").as_deref(), old_cursor.as_deref(), "{mode} must preserve feed checkpoints");
        }
    }

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
    assert_eq!(pull_requests(&database, stored.id).len(), 4);
    assert_eq!(issues(&database, stored.id).len(), 4);
    assert_eq!(fake.calls().matches("api graphql").count(), 4, "the fake should record two retries before the paginated feeds: {}", fake.calls());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn transient_http_502_and_503_failures_are_retried_before_activity_import() {
    let root = unique_root("http-transient-retry");
    let fake = FakeGh::new("http-transient-retry", "http-transient", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);

    let imported = fake.with_path(|| {
        github_sync::sync_activity(&database, &stored)
            .expect("transient HTTP gateway failures should be retried")
    });
    assert!(imported.2, "activity feeds should finish after HTTP retries");
    assert_eq!(pull_requests(&database, stored.id).len(), 4);
    assert_eq!(issues(&database, stored.id).len(), 4);
    assert_eq!(fake.calls().matches("api graphql").count(), 4);
    assert!(fake.calls().contains("HTTP 502 Bad Gateway"));
    assert!(fake.calls().contains("HTTP 503 Service Unavailable"));

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
    assert!(error.to_string().contains("HTTP 503 Service Unavailable"));
    assert_eq!(fake.calls().matches("api graphql").count(), 4, "retry exhaustion should make one initial request and three retries: {}", fake.calls());
    assert_eq!(pull_requests(&database, stored.id).len(), 4, "cached pull requests should survive retry exhaustion");
    assert_eq!(issues(&database, stored.id).len(), 4, "cached issues should survive retry exhaustion");

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
    assert_eq!(pull_requests(&database, stored.id).len(), 4, "cached pull requests should survive a permanent failure");
    assert_eq!(issues(&database, stored.id).len(), 4, "cached issues should survive a permanent failure");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn single_page_incremental_refresh_imports_state_and_ci_changes_at_the_same_timestamp() {
    let root = unique_root("incremental");
    let fake = FakeGh::new("incremental", "incremental", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);

    let same_update = "2026-09-10T00:00:00Z".to_string();
    database
        .upsert_pull_request(&codetally_lib::models::PullRequest {
            repository_id: stored.id,
            number: 7,
            title: "will transition to closed".into(),
            state: "OPEN".into(),
            updated_at: same_update.clone(),
            ci_state: Some("success".into()),
            ..Default::default()
        })
        .expect("seed transitioning PR");
    database
        .upsert_pull_request(&codetally_lib::models::PullRequest {
            repository_id: stored.id,
            number: 8,
            title: "existing CI result".into(),
            state: "OPEN".into(),
            updated_at: same_update.clone(),
            ci_state: Some("success".into()),
            ..Default::default()
        })
        .expect("seed CI refresh PR");
    database
        .upsert_issue(&codetally_lib::models::Issue {
            repository_id: stored.id,
            number: 2,
            title: "existing issue".into(),
            state: "OPEN".into(),
            updated_at: same_update.clone(),
            ..Default::default()
        })
        .expect("seed refreshed issue");
    database
        .upsert_issue(&codetally_lib::models::Issue {
            repository_id: stored.id,
            number: 50,
            title: "cached older closed issue".into(),
            state: "CLOSED".into(),
            updated_at: "2020-01-01T00:00:00Z".into(),
            ..Default::default()
        })
        .expect("seed older cached closed issue");
    database
        .upsert_issue(&codetally_lib::models::Issue {
            repository_id: stored.id,
            number: 51,
            title: "open issue transitioning to closed".into(),
            state: "OPEN".into(),
            updated_at: same_update,
            ..Default::default()
        })
        .expect("seed transitioning issue");

    fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("incremental import"));

    let refreshed_prs = pull_requests(&database, stored.id);
    assert_eq!(
        refreshed_prs.len(),
        3,
        "new and transitioned PRs should coexist"
    );
    assert_eq!(
        refreshed_prs
            .iter()
            .find(|item| item.number == 7)
            .expect("transitioned PR")
            .state,
        "CLOSED"
    );
    let ci_refresh = refreshed_prs
        .iter()
        .find(|item| item.number == 8)
        .expect("CI refresh PR");
    assert_eq!(
        ci_refresh.updated_at, "2026-09-10T00:00:00Z",
        "the API timestamp is unchanged"
    );
    assert_eq!(
        ci_refresh.ci_state.as_deref(),
        Some("failure"),
        "the CI-only update must still be stored"
    );
    assert!(
        refreshed_prs.iter().any(|item| item.number == 9),
        "the new PR must be imported"
    );

    let refreshed_issues = issues(&database, stored.id);
    assert_eq!(refreshed_issues.len(), 4);
    assert_eq!(
        refreshed_issues
            .iter()
            .find(|item| item.number == 2)
            .expect("updated issue")
            .title,
        "same timestamp issue refresh"
    );
    assert_eq!(
        refreshed_issues
            .iter()
            .find(|item| item.number == 51)
            .expect("recent closure")
            .state,
        "CLOSED"
    );
    assert!(
        refreshed_issues
            .iter()
            .any(|item| item.number == 50 && item.title == "cached older closed issue"),
        "an old cached closed issue must remain available"
    );

    let calls = activity_calls(&fake);
    assert_eq!(
        calls.len(),
        1,
        "all four single-page feeds should complete in one request: {}",
        fake.calls()
    );
    assert!(
        calls[0].contains("issueClosedSince="),
        "new closed-issue imports should use the persisted since filter: {}",
        calls[0]
    );

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
            .expect_err("the next request wave failure must be reported");
        assert!(error.to_string().contains("pagination page failed"));
    });
    for (connection, alias) in [
        ("pullRequests:true", "prOpen"),
        ("issues:true", "issueOpen"),
        ("pullRequests:false", "prClosed"),
        ("issues:false", "issueClosed"),
    ] {
        let key = format!("github_activity_v2:{}:{connection}", stored.id);
        let checkpoint = database
            .metadata(&key)
            .expect("checkpoint metadata")
            .expect("durable page one checkpoint");
        let checkpoint: serde_json::Value =
            serde_json::from_str(&checkpoint).expect("checkpoint JSON");
        assert_eq!(
            checkpoint["after"],
            format!("CURSOR_{alias}_1"),
            "each feed advances independently before the next wave fails"
        );
    }
    assert_eq!(
        pull_requests(&database, stored.id).len(),
        2,
        "both first-wave PR pages must persist"
    );
    assert_eq!(
        issues(&database, stored.id).len(),
        2,
        "both first-wave issue pages must persist"
    );

    let key = format!("github_activity_v2:{}:pullRequests:true", stored.id);
    let checkpoint = database
        .metadata(&key)
        .expect("checkpoint metadata")
        .expect("page one checkpoint");
    let checkpoint: serde_json::Value = serde_json::from_str(&checkpoint).expect("checkpoint JSON");
    assert_eq!(checkpoint["after"], "CURSOR_prOpen_1");
    assert!(
        checkpoint["completed_at"].is_null(),
        "a failed page must not mark the feed complete"
    );

    let conn = rusqlite::Connection::open(&database_path).unwrap();
    let first_node: String = conn.query_row("SELECT node_id FROM pull_requests WHERE repository_id=?1 AND number=1", [stored.id], |row| row.get(0)).unwrap();
    assert_eq!(first_node, "PR_NODE_1", "new page rows retain their Kanban identity");
    conn.execute_batch("CREATE TRIGGER reject_identity BEFORE UPDATE OF node_id ON pull_requests WHEN NEW.number=2 BEGIN SELECT RAISE(ABORT, 'identity write failed'); END;").unwrap();
    fake.scenario = "pages:1".into();
    fake.with_path(|| assert!(github_sync::sync_activity(&database, &stored).unwrap_err().to_string().contains("identity write failed")));
    assert_eq!(pull_requests(&database, stored.id).len(), 2, "identity failure rolls back the newly inserted page");
    let unchanged: serde_json::Value = serde_json::from_str(&database.metadata(&key).unwrap().unwrap()).unwrap();
    assert_eq!(unchanged["after"], "CURSOR_prOpen_1", "identity failure preserves the durable cursor");
    conn.execute_batch("DROP TRIGGER reject_identity").unwrap();
    fake.clear_calls();
    fake.with_path(|| {
        github_sync::sync_activity(&database, &stored).expect("retry after failed page")
    });
    let retry_calls = activity_calls(&fake);
    assert_eq!(
        retry_calls.len(),
        1,
        "all four feeds resume together from their own cursors: {}",
        fake.calls()
    );
    for alias in [
        "prOpenAfter=CURSOR_prOpen_1",
        "issueOpenAfter=CURSOR_issueOpen_1",
        "prClosedAfter=CURSOR_prClosed_1",
        "issueClosedAfter=CURSOR_issueClosed_1",
    ] {
        assert!(
            retry_calls[0].contains(alias),
            "retry should include the saved cursor {alias}: {}",
            retry_calls[0]
        );
    }
    assert_eq!(pull_requests(&database, stored.id).len(), 4);
    assert_eq!(issues(&database, stored.id).len(), 4);
    let issue_identity: (String, String) = conn.query_row("SELECT node_id,completion_reason FROM issues WHERE repository_id=?1 AND number=1", [stored.id], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(issue_identity, ("ISSUE_NODE_1".into(), "COMPLETED".into()));
    drop(conn);

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn asymmetric_resume_requests_only_the_unfinished_feed_on_the_next_wave() {
    let root = unique_root("asymmetric-resume");
    let fake = FakeGh::new("asymmetric-resume", "asym", 1);
    let (database, _) = seed_database(&root, &[repository("repo-1", "one")]);
    let stored = database.repositories().unwrap().remove(0);
    database
        .upsert_pull_request(&codetally_lib::models::PullRequest {
            repository_id: stored.id,
            number: 1,
            title: "saved open PR page one".into(),
            state: "OPEN".into(),
            updated_at: "2026-09-10T00:00:00Z".into(),
            ..Default::default()
        })
        .unwrap();
    database
        .upsert_issue(&codetally_lib::models::Issue {
            repository_id: stored.id,
            number: 1,
            title: "saved open issue page one".into(),
            state: "OPEN".into(),
            updated_at: "2026-09-10T00:00:00Z".into(),
            ..Default::default()
        })
        .unwrap();
    for (connection, alias) in [
        ("pullRequests:true", "prOpen"),
        ("issues:true", "issueOpen"),
    ] {
        let key = format!("github_activity_v2:{}:{connection}", stored.id);
        database
            .set_metadata(
                &key,
                &format!(r#"{{"after":"CURSOR_{alias}_1","started_at":"2026-09-10T00:00:00Z"}}"#),
            )
            .unwrap();
    }
    fake.with_path(|| github_sync::sync_activity(&database, &stored).unwrap());
    let calls = activity_calls(&fake);
    assert_eq!(
        calls.len(),
        2,
        "one feed continues to a second page after the other three finish: {}",
        fake.calls()
    );
    for alias in [
        "prOpen:pullRequests",
        "issueOpen:issues",
        "prClosed:pullRequests",
        "issueClosed:issues",
    ] {
        assert!(
            calls[0].contains(alias),
            "the first wave should include every pending feed: {}",
            calls[0]
        );
    }
    assert!(calls[0].contains("prOpenAfter=CURSOR_prOpen_1"));
    assert!(calls[0].contains("issueOpenAfter=CURSOR_issueOpen_1"));
    assert!(
        calls[0].contains("issueClosedSince="),
        "new closed-issue cursors carry their filter bound"
    );
    assert!(
        calls[1].contains("issueClosed:issues"),
        "only the unfinished closed-issue feed should continue: {}",
        calls[1]
    );
    for alias in [
        "prOpen:pullRequests",
        "issueOpen:issues",
        "prClosed:pullRequests",
    ] {
        assert!(
            !calls[1].contains(alias),
            "completed feeds should be absent from later waves: {}",
            calls[1]
        );
    }
    let closed_issue_key = format!("github_activity_v2:{}:issues:false", stored.id);
    let checkpoint: serde_json::Value =
        serde_json::from_str(&database.metadata(&closed_issue_key).unwrap().unwrap()).unwrap();
    assert!(checkpoint["after"].is_null());
    assert!(checkpoint["completed_at"].is_string());
    assert_eq!(
        pull_requests(&database, stored.id).len(),
        3,
        "saved first page plus resumed open and closed items remain"
    );
    assert_eq!(
        issues(&database, stored.id).len(),
        4,
        "saved first page plus open and two closed issue pages remain"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn five_page_limit_persists_closed_issue_filter_and_retry_finishes_remaining_pages() {
    let root = unique_root("five-page-cap");
    let fake = FakeGh::new("five-page-cap", "many", 1);
    let (database, _) = seed_database(&root, &[repository("repo-1", "one")]);
    let stored = database.repositories().unwrap().remove(0);

    let first_cycle =
        fake.with_path(|| github_sync::sync_activity(&database, &stored).expect("five page cycle"));
    assert!(
        !first_cycle.2,
        "feeds with more pages must remain retryable after the per-cycle cap"
    );
    let first_calls = activity_calls(&fake);
    assert_eq!(
        first_calls.len(),
        5,
        "the cycle should issue five shared request waves: {}",
        fake.calls()
    );
    assert_eq!(
        pull_requests(&database, stored.id).len(),
        10,
        "five pages each for open and closed PRs should be durable"
    );
    assert_eq!(
        issues(&database, stored.id).len(),
        10,
        "five pages each for open and closed issues should be durable"
    );

    let issue_key = format!("github_activity_v2:{}:issues:false", stored.id);
    let first_checkpoint: serde_json::Value =
        serde_json::from_str(&database.metadata(&issue_key).unwrap().unwrap()).unwrap();
    assert_eq!(first_checkpoint["after"], "CURSOR_issueClosed_5");
    let since = first_checkpoint["since"]
        .as_str()
        .expect("persisted issue filter bound")
        .to_owned();
    assert!(
        first_calls[0].contains(&format!("issueClosedSince={since}")),
        "initial request sends the closed issue cutoff"
    );

    fake.clear_calls();
    let second_cycle = fake.with_path(|| {
        github_sync::sync_activity(&database, &stored).expect("retry remaining pages")
    });
    assert!(
        second_cycle.2,
        "the retry should finish the remaining two pages"
    );
    let retry_calls = activity_calls(&fake);
    assert_eq!(
        retry_calls.len(),
        2,
        "only pages six and seven should be fetched on retry: {}",
        fake.calls()
    );
    assert!(retry_calls[0].contains("issueClosedAfter=CURSOR_issueClosed_5"));
    assert!(
        retry_calls[0].contains(&format!("issueClosedSince={since}")),
        "retry must preserve the exact filter cutoff"
    );
    assert_eq!(pull_requests(&database, stored.id).len(), 14);
    assert_eq!(issues(&database, stored.id).len(), 14);
    let completed: serde_json::Value =
        serde_json::from_str(&database.metadata(&issue_key).unwrap().unwrap()).unwrap();
    assert!(completed["after"].is_null());
    assert!(completed["completed_at"].is_string());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_cursor_clears_only_the_stale_cursor_before_retrying_from_watermark() {
    let root = unique_root("invalid-cursor");
    let mut fake = FakeGh::new("invalid-cursor", "cursor-error", 1);
    let repo = repository("repo-1", "one");
    let (database, database_path) = seed_database(&root, std::slice::from_ref(&repo));
    let stored = database.repositories().expect("repositories").remove(0);
    let key = format!("github_activity_v2:{}:pullRequests:true", stored.id);
    let legacy_key = format!("github_activity_v2:{}:issues:false", stored.id);
    database
        .set_metadata(
            &key,
            r#"{"started_at":"2026-09-09T00:00:00Z","after":"STALE_CURSOR","completed_at":null}"#,
        )
        .expect("stale cursor checkpoint");
    database
        .set_metadata(
            &legacy_key,
            r#"{"started_at":"2026-09-09T00:00:00Z","after":"CURSOR_issueClosed_1"}"#,
        )
        .expect("legacy closed issue cursor");
    database
        .upsert_issue(&codetally_lib::models::Issue {
            repository_id: stored.id,
            number: 201,
            title: "saved first closed issue page".into(),
            state: "CLOSED".into(),
            updated_at: "2026-09-10T00:00:00Z".into(),
            ..Default::default()
        })
        .expect("seed legacy closed issue page");

    fake.with_path(|| {
        let error = github_sync::sync_activity(&database, &stored)
            .expect_err("stale cursor should be reported once");
        assert!(error.to_string().contains("Invalid cursor"));
    });
    let reset: serde_json::Value = serde_json::from_str(
        &database
            .metadata(&key)
            .expect("checkpoint metadata")
            .expect("reset checkpoint"),
    )
    .expect("reset checkpoint JSON");
    assert!(
        reset["after"].is_null(),
        "the invalid cursor must not be retried forever"
    );
    assert_eq!(
        reset["started_at"], "2026-09-09T00:00:00Z",
        "the initial cycle start must survive cursor recovery"
    );
    let legacy: serde_json::Value =
        serde_json::from_str(&database.metadata(&legacy_key).unwrap().unwrap()).unwrap();
    assert_eq!(
        legacy["after"], "CURSOR_issueClosed_1",
        "the healthy legacy cursor must survive the error"
    );
    assert!(
        legacy.get("since").is_none(),
        "legacy cursors finish without introducing a new server filter"
    );

    let stale_checkpoint =
        r#"{"started_at":"2026-09-09T00:00:00Z","after":"STALE_CURSOR","completed_at":null}"#;
    database
        .set_metadata(&key, stale_checkpoint)
        .expect("restore stale cursor for pathless error");
    fake.scenario = "cursor-no-path:1".into();
    fake.clear_calls();
    fake.with_path(|| {
        let error = github_sync::sync_activity(&database, &stored)
            .expect_err("pathless cursor failure should trigger bounded single-feed probes");
        assert!(error.to_string().contains("Invalid cursor"));
    });
    let probe_calls = activity_calls(&fake);
    assert_eq!(
        probe_calls.len(),
        3,
        "one failed batch plus two single-feed probes are expected: {}",
        fake.calls()
    );
    assert!(probe_calls[0].contains("prOpen:pullRequests"));
    assert!(probe_calls[0].contains("issueClosed:issues"));
    assert!(probe_calls[0].contains("prOpenAfter=STALE_CURSOR"));
    assert!(probe_calls[0].contains("issueClosedAfter=CURSOR_issueClosed_1"));
    assert!(probe_calls[1].contains("prOpen:pullRequests"));
    assert!(!probe_calls[1].contains("issueClosed:issues"));
    assert!(probe_calls[1].contains("prOpenAfter=STALE_CURSOR"));
    assert!(probe_calls[2].contains("issueClosed:issues"));
    assert!(!probe_calls[2].contains("prOpen:pullRequests"));
    assert!(probe_calls[2].contains("issueClosedAfter=CURSOR_issueClosed_1"));

    let reset: serde_json::Value =
        serde_json::from_str(&database.metadata(&key).unwrap().unwrap()).unwrap();
    assert!(
        reset["after"].is_null(),
        "only the stale cursor should be reset after probing"
    );
    assert_eq!(
        reset["started_at"], "2026-09-09T00:00:00Z",
        "cursor recovery keeps the original watermark"
    );
    assert_eq!(
        database.metadata(&legacy_key).unwrap().unwrap(),
        r#"{"started_at":"2026-09-09T00:00:00Z","after":"CURSOR_issueClosed_1"}"#,
        "the healthy cursor metadata should remain byte-for-byte unchanged"
    );
    assert_eq!(
        issues(&database, stored.id).len(),
        1,
        "diagnostic probes must not commit response pages"
    );

    fake.scenario = "pages:1".into();
    fake.clear_calls();
    fake.with_path(|| {
        github_sync::sync_activity(&database, &stored).expect("retry from watermark")
    });
    let retry_calls = activity_calls(&fake);
    assert_eq!(
        retry_calls.len(),
        2,
        "reset and valid legacy feeds resume in shared waves: {}",
        fake.calls()
    );
    assert!(!retry_calls[0].contains("STALE_CURSOR"));
    assert!(retry_calls[0].contains("issueClosedAfter=CURSOR_issueClosed_1"));
    assert!(
        !retry_calls[0].contains("issueClosedSince="),
        "legacy cursor query must remain unfiltered"
    );
    assert_eq!(pull_requests(&database, stored.id).len(), 4);
    assert_eq!(issues(&database, stored.id).len(), 4);

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
    assert_eq!(
        result.pull_requests_synced, 2,
        "open and closed durable PR feeds should be reported"
    );
    assert_eq!(
        result.issues_synced, 2,
        "open and closed durable issue feeds should be reported"
    );
    assert_eq!(
        result.loc_repositories_synced, 0,
        "a paused final response must not start LOC work"
    );
    assert!(result
        .errors
        .iter()
        .any(|error| error.contains("paused until")));
    assert_eq!(
        pull_requests(&database, stored.id).len(),
        2,
        "all successful PR feed pages must commit before quota pause"
    );
    assert_eq!(
        issues(&database, stored.id).len(),
        2,
        "all successful issue feed pages must commit before quota pause"
    );
    assert_eq!(
        activity_calls(&fake).len(),
        1,
        "the low-quota response is committed before the next page request is blocked"
    );

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
    assert!(
        calls.lines().any(|call| call.contains("repositoryId=repo-1")),
        "first repository should reach GraphQL"
    );
    assert!(
        !calls.lines().any(|call| call.contains("repositoryId=repo-2")),
        "rate limit must prevent following repositories: {calls}"
    );
    assert!(database
        .metadata(github_sync::PAUSE_KEY)
        .expect("pause metadata")
        .is_some());

    fake.clear_calls();
    let restarted_state = state(database_path.clone(), &root);
    let paused = fake.with_path(|| sync::sync_activity(&restarted_state));
    let paused_error = paused.expect_err("restart should remain paused");
    assert!(paused_error.to_string().contains("paused until"));
    assert!(
        fake.calls().is_empty(),
        "a paused refresh must not invoke gh: {}",
        fake.calls()
    );

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
    assert!(
        !fake
            .calls()
            .lines()
            .any(|call| call.starts_with("repo list")),
        "fresh discovery should be reused for one hour: {}",
        fake.calls()
    );
    assert!(!fake.calls().lines().any(|call| call.starts_with("auth status")), "data requests authenticate the refresh");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}

#[test]
fn owned_discovery_paginates_before_publishing_and_failed_pages_retain_cache() {
    let root = unique_root("owned-discovery");
    let fake = FakeGh::new("owned-discovery", "discovery-pages", 101);
    let (database, database_path) = seed_database(&root, &[repository("repo-1", "one")]);
    let app = state(database_path, &root);
    let discovered = fake.with_path(|| sync::discover(&app)).unwrap();
    assert_eq!(discovered.len(), 101);
    assert!(discovered.iter().any(|repo| repo.name == "extra101"));
    let owned = discovered.iter().find(|repo| repo.name == "one").unwrap();
    assert!(owned.loc_backfill_complete);
    assert!(owned.local_path.is_some());
    assert_eq!(database.metadata("github_login").unwrap().as_deref(), Some("me"));
    let calls = fake.calls();
    assert_eq!(calls.lines().filter(|call| call.starts_with("api graphql")).count(), 2);
    assert!(calls.contains("after=OWNED_100"));
    assert!(!calls.lines().any(|call| call.starts_with("repo list")), "owned repositories share the identity request");

    let before = serde_json::to_value(database.repositories().unwrap()).unwrap();
    let timestamp = database.metadata("github_discovered_at").unwrap();
    let failing = FakeGh::new("owned-discovery-failed", "discovery-failed", 101);
    let error = failing.with_path(|| sync::discover(&app)).unwrap_err();
    assert!(error.to_string().contains("discovery page failed"));
    assert_eq!(serde_json::to_value(database.repositories().unwrap()).unwrap(), before, "a partial owned listing must not publish changed metadata");
    assert_eq!(database.metadata("github_discovered_at").unwrap(), timestamp);
    assert!(!failing.calls().contains("user/orgs"), "do not continue discovery after an incomplete owned listing");
    let switched = FakeGh::new("owned-discovery-account", "discovery-account", 101);
    let error = switched.with_path(|| sync::discover(&app)).unwrap_err();
    assert!(error.to_string().contains("account changed"));
    assert_eq!(serde_json::to_value(database.repositories().unwrap()).unwrap(), before);
    assert_eq!(database.metadata("github_login").unwrap().as_deref(), Some("me"));
    assert!(!switched.calls().contains("user/orgs"));

    let scoped = FakeGh::new("owned-discovery-selection", "single", 1);
    for (personal, company) in [(false, true), (true, false), (false, false)] {
        sync::save_app_settings(&database, &AppSettings { include_personal_repositories: personal, include_company_repositories: company, ..AppSettings::default() }).unwrap();
        scoped.clear_calls();
        scoped.with_path(|| sync::discover(&app)).unwrap();
        let calls = scoped.calls();
        assert_eq!(calls.contains("ownerAffiliations:[OWNER]"), personal, "owned listing follows group selection: {calls}");
        assert_eq!(calls.contains("user/orgs"), company, "organization listing follows group selection: {calls}");
        assert_eq!(calls.lines().filter(|call| call.starts_with("api graphql")).count(), usize::from(personal || company));
        assert!(!calls.lines().any(|call| call.starts_with("api user") || call.starts_with("auth status")), "identity shares the discovery request: {calls}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn authentication_failures_stop_refresh_and_preserve_committed_pages() {
    for (mode, cached, refresh, expected_pages) in [
        ("auth-missing", false, "all", 0),
        ("auth-org", false, "activity", 0),
        ("auth-expired", true, "activity", 0),
        ("auth-late", true, "activity", 1),
        ("auth-org", false, "work", 0),
        ("auth-expired", true, "work", 0),
        ("auth-late", true, "work", 1),
        ("auth-expired", true, "personal", 0),
    ] {
        let root = unique_root(mode);
        let fake = FakeGh::new(mode, mode, 2);
        let (database, database_path) = seed_database(&root, &[repository("repo-1", "one"), repository("repo-2", "two")]);
        if cached { database.set_metadata("github_discovered_at", &Utc::now().to_rfc3339()).unwrap(); }
        database.set_metadata("github_login", "me").unwrap();
        kanban::remember_repository(&database, "repo-1").unwrap();
        kanban::remember_repository(&database, "repo-2").unwrap();
        let app = state(database_path, &root);
        let result = fake.with_path(|| match refresh {
            "all" => sync::sync_all(&app),
            "work" => sync::sync_work_items(&app),
            "personal" => sync::sync_personal_work_items(&app),
            _ => sync::sync_activity(&app),
        }).unwrap();
        assert!(!result.ok, "{mode}");
        assert!(result.message.contains("not authenticated"), "{mode}: {result:?}");
        assert!(result.errors.iter().any(|error| error.contains("gh auth login")));
        assert!(!app.progress().running);
        let calls = fake.calls();
        assert!(!calls.lines().any(|call| call.starts_with("auth status")));
        if refresh == "personal" {
            assert_eq!(calls.lines().filter(|call| call.starts_with("api graphql")).count(), 1, "authentication failure skips totals refresh");
        }
        assert!(!calls.contains("repositoryId=repo-2"), "authentication failure stops subsequent repositories: {calls}");
        assert!(!calls.lines().any(|call| call.starts_with("repo list")), "failed identity request stops discovery");
        let one = database.repositories().unwrap().into_iter().find(|repo| repo.name == "one").unwrap();
        assert_eq!(pull_requests(&database, one.id).len(), expected_pages * 2);
        assert_eq!(issues(&database, one.id).len(), expected_pages * 2);
        assert_eq!(result.pull_requests_synced, (expected_pages * 2) as i64);
        assert_eq!(result.issues_synced, (expected_pages * 2) as i64);
        if expected_pages > 0 {
            let checkpoint = database.metadata(&format!("github_activity_v2:{}:pullRequests:true", one.id)).unwrap().unwrap();
            assert!(checkpoint.contains("CURSOR_prOpen_1"));
        }
        assert!(database.metadata(github_sync::PAUSE_KEY).unwrap().is_none(), "login failures must not create a rate-limit cooldown");
        std::fs::remove_dir_all(root).unwrap();
    }

    // A per-repository permission failure must not look like a global login failure.
    let root = unique_root("permission-denied");
    let fake = FakeGh::new("permission-denied", "permission-denied", 2);
    let (database, database_path) = seed_database(&root, &[repository("repo-1", "one"), repository("repo-2", "two")]);
    database.set_metadata("github_discovered_at", &Utc::now().to_rfc3339()).unwrap();
    let app = state(database_path, &root);
    let result = fake.with_path(|| sync::sync_activity(&app)).unwrap();
    assert!(!result.ok);
    assert!(!result.message.contains("not authenticated"));
    assert_eq!(result.activity_repositories_synced, 1);
    assert!(fake.calls().contains("repositoryId=repo-2"));
    std::fs::remove_dir_all(root).unwrap();
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
    assert!(database.metadata(sync::LAST_FULL_REFRESH_METADATA_KEY).expect("broad refresh marker").is_some());
    let calls = fake.calls();
    assert!(calls.lines().any(|call| call.contains("repositoryId=repo-1")), "selected repository should reach GitHub: {calls}");
    assert!(!calls.lines().any(|call| call.contains("repositoryId=repo-2")), "excluded repository must not reach GitHub: {calls}");

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

#[test]
fn unavailable_repository_is_removed_with_cached_data_and_other_repositories_continue() {
    let root = unique_root("unavailable-repository");
    let mut fake = FakeGh::new("unavailable-repository", "pages", 2);
    let repos = [repository("repo-1", "one"), repository("repo-2", "two")];
    let (database, database_path) = seed_database(&root, &repos);
    // This test covers the legacy removal policy when the board is explicitly disabled.
    sync::save_app_settings(&database, &AppSettings { kanban_enabled: false, ..AppSettings::default() }).expect("disable board");
    let sync_state = state(database_path, &root);
    fake.with_path(|| sync::sync_activity(&sync_state).expect("seed cached activity"));
    let stored = database.repositories().unwrap().remove(0);
    let cached_prs = pull_requests(&database, stored.id).len();
    assert!(cached_prs > 0);
    database.set_metadata("github_repository_unavailable:repo-1", "true").unwrap();
    let cursor_key = format!("github_activity_v2:{}:pullRequests:true", stored.id);
    database.set_metadata(&cursor_key, r#"{"completed_at":null,"started_at":null,"after":null}"#).unwrap();

    fake.scenario = "missing-repository:2".into();
    fake.clear_calls();
    let result = fake.with_path(|| sync::sync_activity(&sync_state).unwrap());
    assert!(result.ok, "missing repository must not fail refresh: {:?}", result.errors);
    assert_eq!(result.activity_repositories_synced, 1);
    assert_eq!(fake.calls().lines().filter(|call| call.contains("repositoryId=repo-1")).count(), 1);
    assert!(database.repository(stored.id).unwrap().is_none());
    assert!(!database.repository_selection().unwrap().iter().any(|repo| repo.github_id == stored.github_id));
    assert!(!database.summaries().unwrap().iter().any(|repo| repo.id == stored.id));
    assert!(database.metadata("github_repository_unavailable:repo-1").unwrap().is_none());
    assert!(database.metadata(&cursor_key).unwrap().is_none());
    let conn = rusqlite::Connection::open(database.path()).unwrap();
    for table in ["pull_requests", "issues", "code_snapshots", "loc_observations", "classification_rules"] {
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE repository_id=?1"), [stored.id], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "{table} should be deleted with the repository");
    }

    fake.clear_calls();
    let restarted = state(sync_state.db_path.clone(), &root);
    assert!(fake.with_path(|| sync::sync_activity(&restarted).unwrap()).ok);
    assert!(!fake.calls().contains("repositoryId=repo-1"));
    assert!(fake.calls().contains("repositoryId=repo-2"));

    fake.clear_calls();
    let manual = fake.with_path(|| sync::sync_one(&restarted, stored.id).expect_err("removed repository should no longer be addressable"));
    assert!(manual.to_string().contains("was not found"));
    assert!(!fake.calls().contains("repositoryId=repo-1"));

    fake.scenario = "pages:2".into();
    fake.with_path(|| sync::discover(&restarted).unwrap());
    assert!(database.repositories().unwrap().iter().any(|repo| repo.github_id == stored.github_id));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn only_missing_repository_errors_are_classified_as_unavailable() {
    use codetally_lib::error::AppError;
    let root = unique_root("missing-repository-response");
    let (database, _) = seed_database(&root, &[]);
    for success in [true, false] {
        let error = github_sync::parse_response(&database, success,
            r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","path":["repository"]}]}"#, "").unwrap_err();
        assert!(matches!(error, AppError::RepositoryUnavailable));
    }
    let error = github_sync::parse_response(&database, false, "",
        "gh: Could not resolve to a Repository with the name 'Xzese/pnpm'.").unwrap_err();
    assert!(matches!(error, AppError::RepositoryUnavailable));
    for body in [
        r#"{"errors":[{"type":"NOT_FOUND","path":["repository","issues"]}]}"#,
        r#"{"errors":[{"type":"FORBIDDEN","path":["repository"]}]}"#,
        r#"{"errors":[{"type":"NOT_FOUND","path":["repository"]},{"type":"INTERNAL"}]}"#,
    ] {
        assert!(matches!(github_sync::parse_response(&database, false, body, "gh: Could not resolve to a Repository with the name 'me/one'.").unwrap_err(), AppError::Command { .. }));
    }
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn explicit_work_item_sync_does_not_touch_loc_even_when_all_loc_triggers_are_due() {
    let root = unique_root("work-items-loc-isolation");
    let fake = FakeGh::new("work-items-loc-isolation", "pages", 1);
    let forbidden_log = fake.root.join("git-tokei.log");
    std::fs::write(&forbidden_log, "").expect("forbidden command log");
    install_command_guard(&fake.bin, "git", &forbidden_log);
    install_command_guard(&fake.bin, "tokei", &forbidden_log);

    let (database, database_path) = seed_database(&root, &[]);
    let pushed_at = "2026-09-10T00:00:00Z";
    let fetched_pushed_at = "2026-09-01T00:00:00Z";
    let mut repo = repository("repo-work-items", "work-items");
    repo.pushed_at = Some(pushed_at.into());
    repo.last_fetched_pushed_at = Some(fetched_pushed_at.into());
    repo.local_path = None;
    repo.loc_backfill_complete = false;
    let repo_id = database.upsert_repository(&repo).expect("seed repository");
    let stored = database
        .repository(repo_id)
        .expect("repository lookup")
        .expect("stored repository");

    let old_sweep = "2020-01-01T00:00:00Z";
    database
        .set_metadata("github_login", "me")
        .expect("cached login");
    database
        .set_metadata("github_discovered_at", &Utc::now().to_rfc3339())
        .expect("fresh discovery cache");
    database
        .set_metadata(sync::LOC_SWEEP_METADATA_KEY, old_sweep)
        .expect("overdue LOC sweep");
    database
        .set_metadata(sync::LAST_LOC_REFRESH_METADATA_KEY, old_sweep)
        .expect("previous line refresh");
    database
        .set_metadata(sync::LAST_FULL_REFRESH_METADATA_KEY, old_sweep)
        .expect("previous broad refresh");
    codetally_lib::kanban::remember_repository(&database, &stored.github_id)
        .expect("remember tracked repo");
    let cached_loc = Snapshot {
        repository_id: repo_id,
        commit_sha: "existing-loc-sample".into(),
        commit_date: "2026-09-01T00:00:00Z".into(),
        snapshot_date: "2026-09-01T00:00:00Z".into(),
        total_loc: 120,
        source_loc: 100,
        test_loc: 20,
        docs_loc: 0,
        created_at: "2026-09-01T00:00:00Z".into(),
        ..Snapshot::default()
    };
    database
        .upsert_snapshot(&cached_loc)
        .expect("seed LOC snapshot");
    database
        .upsert_observation(&cached_loc)
        .expect("seed LOC observation");
    let sync_state = state(database_path.clone(), &root);

    let result = fake.with_path(|| sync::sync_work_items(&sync_state).expect("work-item sync"));
    assert!(
        result.ok,
        "activity refresh should succeed: {:?}",
        result.errors
    );
    assert_eq!(result.activity_repositories_synced, 1);
    assert!(result.pull_requests_synced > 0);
    assert!(result.issues_synced > 0);
    assert!(
        activity_calls(&fake).len() == 2,
        "the activity feeds should share two request waves: {}",
        fake.calls()
    );

    let refreshed = database
        .repository(repo_id)
        .expect("repository lookup")
        .expect("repository remains");
    assert_eq!(refreshed.pushed_at.as_deref(), Some(pushed_at));
    assert_eq!(
        refreshed.last_fetched_pushed_at.as_deref(),
        Some(fetched_pushed_at)
    );
    assert_ne!(
        refreshed.pushed_at, refreshed.last_fetched_pushed_at,
        "explicit activity sync must not advance LOC's pushedAt cursor"
    );
    assert!(
        refreshed.local_path.is_none(),
        "activity sync must leave a missing clone missing"
    );
    assert!(
        !refreshed.loc_backfill_complete,
        "activity sync must not mark an incomplete LOC backfill complete"
    );
    assert_eq!(
        database
            .metadata(sync::LOC_SWEEP_METADATA_KEY)
            .expect("LOC sweep marker")
            .as_deref(),
        Some(old_sweep)
    );
    assert_eq!(database.metadata(sync::LAST_LOC_REFRESH_METADATA_KEY).expect("line refresh marker").as_deref(), Some(old_sweep));
    assert_eq!(database.metadata(sync::LAST_FULL_REFRESH_METADATA_KEY).expect("broad refresh marker").as_deref(), Some(old_sweep));
    assert!(database.metadata(sync::LAST_ACTIVITY_REFRESH_METADATA_KEY).expect("ticket refresh marker").is_some());

    let conn = rusqlite::Connection::open(database.path()).expect("database connection");
    for (table, expected) in [("code_snapshots", 1), ("loc_observations", 1)] {
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE repository_id=?1"),
                [repo_id],
                |row| row.get(0),
            )
            .expect("LOC row count");
        assert_eq!(
            count, expected,
            "work-item sync must preserve cached {table}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&forbidden_log).expect("forbidden command log"),
        "",
        "explicit activity sync must not invoke git or tokei"
    );

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(database_path);
}
