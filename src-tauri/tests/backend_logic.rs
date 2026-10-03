use chrono::{DateTime, Duration, Utc};
use codetally_lib::migrate_legacy_app_data;
use codetally_lib::classify::{
    classify_tokei_json, is_test_path_with_config, is_tokei_report, month_sample_dates,
    range_start,
};
use codetally_lib::db::{DashboardCache, Database};
use codetally_lib::gitops::{commit_at_or_before, current_commit_optional, earliest_commit, ensure_clone, scan_worktree_with_config};
use codetally_lib::models::{
    ActivityItem, AppSettings, ClassificationConfig, GithubIssueJson, GithubPullRequestJson,
    GithubRepositoryJson, Issue, PullRequest, Repository, Snapshot, SyncProgress,
};
use codetally_lib::sync::{self, decide_loc_sync, AppState};
use serde_json::json;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_database(label: &str) -> (Database, PathBuf) {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "codetally-{label}-{}-{suffix}.sqlite3",
        std::process::id()
    ));
    let database = Database::new(&path);
    database.init().expect("database schema should initialize");
    (database, path)
}

fn remove_database(path: PathBuf) {
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

#[test]
fn legacy_app_data_is_copied_to_codetally_without_overwriting_new_files() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codetally-migration-{}-{suffix}", std::process::id()));
    let legacy = root.join("com.samfaid.github-portfolio");
    let current = root.join("com.samfaid.codetally");
    std::fs::create_dir_all(legacy.join("repositories/owner/repository/.git"))
        .expect("legacy repository directory");
    std::fs::write(legacy.join("github-portfolio.sqlite3"), b"legacy database")
        .expect("legacy database");
    std::fs::write(legacy.join("github-portfolio.sqlite3-wal"), b"legacy wal")
        .expect("legacy WAL");
    std::fs::write(legacy.join("github-portfolio.sqlite3-shm"), b"legacy shared memory")
        .expect("legacy shared memory");
    std::fs::write(
        legacy.join("repositories/owner/repository/.git/config"),
        b"legacy clone",
    )
    .expect("legacy repository file");

    migrate_legacy_app_data(&current).expect("legacy migration");
    assert_eq!(
        std::fs::read(current.join("codetally.sqlite3")).expect("copied database"),
        b"legacy database"
    );
    assert_eq!(
        std::fs::read(current.join("codetally.sqlite3-wal")).expect("copied WAL"),
        b"legacy wal"
    );
    assert_eq!(
        std::fs::read(current.join("repositories/owner/repository/.git/config"))
            .expect("copied repository"),
        b"legacy clone"
    );

    std::fs::write(current.join("codetally.sqlite3"), b"new database")
        .expect("new database");
    std::fs::remove_file(current.join("codetally.sqlite3-wal")).expect("remove copied WAL");
    std::fs::remove_file(current.join("codetally.sqlite3-shm")).expect("remove copied shared memory");
    std::fs::write(legacy.join("github-portfolio.sqlite3"), b"changed legacy database")
        .expect("changed legacy database");
    migrate_legacy_app_data(&current).expect("repeat migration");
    assert_eq!(
        std::fs::read(current.join("codetally.sqlite3")).expect("preserved database"),
        b"new database"
    );
    assert!(!current.join("codetally.sqlite3-wal").exists(), "existing databases keep their own sidecars");
    assert!(!current.join("codetally.sqlite3-shm").exists(), "existing databases keep their own sidecars");
    assert!(legacy.exists(), "legacy data remains available");
    let _ = std::fs::remove_dir_all(root);
}

fn repository(github_id: &str, name: &str) -> Repository {
    Repository {
        github_id: github_id.to_string(),
        owner: "owner".to_string(),
        name: name.to_string(),
        name_with_owner: format!("owner/{name}"),
        url: format!("https://github.com/owner/{name}"),
        ssh_url: format!("git@github.com:owner/{name}.git"),
        default_branch: "main".to_string(),
        created_at: Some("2024-01-01T00:00:00Z".to_string()),
        ..Repository::default()
    }
}

fn snapshot(repository_id: i64, sha: &str, date: &str, total: i64) -> Snapshot {
    Snapshot {
        repository_id,
        commit_sha: sha.to_string(),
        commit_date: date.to_string(),
        snapshot_date: date.to_string(),
        total_loc: total,
        source_loc: total - 20,
        test_loc: 20,
        created_at: date.to_string(),
        ..Snapshot::default()
    }
}

fn completed_repository(github_id: &str, name: &str, pushed_at: &str, fetched_pushed_at: &str) -> Repository {
    Repository {
        pushed_at: Some(pushed_at.to_string()),
        last_fetched_pushed_at: Some(fetched_pushed_at.to_string()),
        local_path: Some(format!("/managed/cache/{name}")),
        loc_backfill_complete: true,
        ..repository(github_id, name)
    }
}

#[test]
fn github_json_models_parse_ids_and_nested_activity_fields() {
    let repository: GithubRepositoryJson = serde_json::from_value(json!({
        "id": "R_kgDOExample",
        "name": "portfolio",
        "nameWithOwner": "owner/portfolio",
        "url": "https://github.com/owner/portfolio",
        "sshUrl": "git@github.com:owner/portfolio.git",
        "isPrivate": true,
        "isFork": false,
        "isArchived": false,
        "stargazerCount": 123,
        "forkCount": 45,
        "defaultBranchRef": {"name": "trunk"},
        "primaryLanguage": {"name": "Rust"},
        "createdAt": "2024-01-01T00:00:00Z",
        "updatedAt": "2026-09-01T00:00:00Z",
        "pushedAt": "2026-09-02T00:00:00Z"
    }))
    .expect("repository JSON should parse");
    assert_eq!(repository.id, "R_kgDOExample");
    assert_eq!(repository.default_branch_ref.expect("branch").name, "trunk");
    assert_eq!(repository.primary_language.expect("language").name, "Rust");
    assert_eq!(repository.star_count, 123);
    assert_eq!(repository.fork_count, 45);

    let numeric_id: GithubRepositoryJson = serde_json::from_value(json!({
        "id": 42,
        "name": "numeric",
        "nameWithOwner": "owner/numeric",
        "url": "https://github.com/owner/numeric",
        "sshUrl": "git@github.com:owner/numeric.git",
        "isPrivate": false,
        "isFork": false,
        "isArchived": false
    }))
    .expect("numeric repository IDs should also parse");
    assert_eq!(numeric_id.id, "42");
    assert_eq!(numeric_id.star_count, 0);
    assert_eq!(numeric_id.fork_count, 0);

    let pull_request: GithubPullRequestJson = serde_json::from_value(json!({
        "number": 17,
        "title": "Improve sync",
        "state": "OPEN",
        "isDraft": true,
        "createdAt": "2026-08-01T00:00:00Z",
        "updatedAt": "2026-09-03T00:00:00Z",
        "mergedAt": null,
        "closedAt": null,
        "url": "https://github.com/owner/portfolio/pull/17",
        "additions": 12,
        "deletions": 4,
        "changedFiles": 2,
        "statusCheckRollup": [{"state": "SUCCESS"}]
    }))
    .expect("pull request JSON should parse");
    assert_eq!(pull_request.number, 17);
    assert!(pull_request.is_draft);
    assert_eq!(pull_request.changed_files, Some(2));
    assert_eq!(pull_request.status_check_rollup.expect("checks")[0]["state"], "SUCCESS");

    let issue: GithubIssueJson = serde_json::from_value(json!({
        "number": 8,
        "title": "Add cache controls",
        "state": "CLOSED",
        "createdAt": "2026-07-01T00:00:00Z",
        "updatedAt": "2026-09-04T00:00:00Z",
        "closedAt": "2026-09-04T00:00:00Z",
        "url": "https://github.com/owner/portfolio/issues/8",
        "author": {"login": "octocat"},
        "labels": [{"name": "enhancement"}],
        "assignees": [{"login": "maintainer"}]
    }))
    .expect("issue JSON should parse");
    assert_eq!(issue.author.expect("author").login, "octocat");
    assert_eq!(issue.labels[0].name, "enhancement");
    assert_eq!(issue.assignees[0].login, "maintainer");
}

#[cfg(unix)]
#[test]
fn earliest_reachable_git_commit_extends_history_before_later_github_creation_date() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codetally-history-git-{}-{suffix}", std::process::id()));
    std::fs::create_dir_all(&root).expect("git repository directory");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(&root)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git command failed: {args:?}");
    };
    git(&["init"]);
    git(&["config", "user.email", "portfolio@example.test"]);
    git(&["config", "user.name", "Portfolio Tests"]);
    std::fs::write(root.join("main.rs"), "fn main() {}\n").expect("first source file");
    git(&["add", "main.rs"]);
    let first_commit = Command::new("git")
        .current_dir(&root)
        .args(["commit", "-m", "first"])
        .env("GIT_AUTHOR_DATE", "2024-06-29T12:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-06-29T12:00:00+00:00")
        .status()
        .expect("first commit");
    assert!(first_commit.success());
    git(&["branch", "-M", "main"]);
    std::fs::write(root.join("main.rs"), "fn main() { println!(\"later\"); }\n").expect("second source file");
    git(&["add", "main.rs"]);
    let second_commit = Command::new("git")
        .current_dir(&root)
        .args(["commit", "-m", "second"])
        .env("GIT_AUTHOR_DATE", "2024-07-02T12:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-07-02T12:00:00+00:00")
        .status()
        .expect("second commit");
    assert!(second_commit.success());

    let (_, first_date) = earliest_commit(&root, "main")
        .expect("earliest commit lookup")
        .expect("non-empty repository");
    assert!(first_date.starts_with("2024-06-29"), "unexpected earliest commit date: {first_date}");
    // The GitHub API can report a later repository creation date even when the
    // managed clone contains an older reachable commit. History sampling must
    // use the commit date in that case rather than the GitHub timestamp.
    let github_created_at = "2024-07-01T00:00:00Z";
    assert!(first_date.as_str() < github_created_at);
    let end: DateTime<Utc> = "2024-08-08T00:00:00Z".parse().expect("history end");
    let dates = month_sample_dates(Some(&first_date), end);
    assert_eq!(dates.first().expect("June sample"), "2024-06-30T23:59:59Z");
    assert_eq!(dates.len(), 3);
    assert!(commit_at_or_before(&root, "main", "2024-06-01T00:00:00Z").unwrap().is_none());
    for date in &dates {
        let (sha, sampled_date) = commit_at_or_before(&root, "main", date).unwrap().unwrap();
        let reference = Command::new("git").current_dir(&root).args(["rev-list", "-n", "1", &format!("--before={date}"), "main"]).output().unwrap();
        assert_eq!(sha, String::from_utf8_lossy(&reference.stdout).trim());
        assert!(sampled_date.starts_with(if date.starts_with("2024-06") { "2024-06-29" } else { "2024-07-02" }));
    }
    let (database, database_path) = temp_database("backfill-restore");
    let repo_id = database.upsert_repository(&Repository { url: root.to_string_lossy().into_owned(), ..repository("history", "history") }).unwrap();
    let state = AppState { db_path: database_path.clone(), cache_dir: root.join("cache"), progress: Arc::new(Mutex::new(SyncProgress::default())), dashboard_cache: Arc::new(Mutex::new(Default::default())), job_lock: Arc::new(Mutex::new(())) };
    let result = sync::backfill_one(&state, repo_id).unwrap();
    assert!(result.ok, "backfill errors: {:?}", result.errors);
    assert_eq!(result.snapshots_created, 2, "monthly samples should scan each SHA once");
    let cached = state.cache_dir.join("owner/history");
    let branch = || String::from_utf8(Command::new("git").current_dir(&cached).args(["symbolic-ref", "--short", "HEAD"]).output().unwrap().stdout).unwrap();
    assert_eq!(branch().trim(), "main");
    assert_eq!(sync::backfill_one(&state, repo_id).unwrap().snapshots_created, 0, "repeat backfill reuses saved samples");
    database.set_classification_config(repo_id, &ClassificationConfig::default()).unwrap();
    let conn = Connection::open(&database_path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_sample BEFORE INSERT ON code_snapshots BEGIN SELECT RAISE(ABORT,'forced sample failure'); END;").unwrap();
    let failed = sync::backfill_one(&state, repo_id).unwrap();
    assert!(!failed.ok);
    assert!(failed.errors.iter().any(|e| e.contains("forced sample failure")));
    assert_eq!(branch().trim(), "main", "early database errors must restore the default branch");
    drop(conn);
    remove_database(database_path);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn automatic_loc_cadence_skips_unchanged_repositories_but_scans_pushed_changes() {
    let now: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().expect("cadence instant");
    let recent_sweep = "2026-09-08T11:30:00Z";
    let unchanged = completed_repository("cadence-unchanged", "unchanged", "2026-09-08T10:00:00Z", "2026-09-08T10:00:00Z");
    let changed = completed_repository("cadence-changed", "changed", "2026-09-08T11:00:00Z", "2026-09-08T10:00:00Z");

    let unchanged_decision = decide_loc_sync(&unchanged, now, Some(recent_sweep), false);
    assert!(!unchanged_decision.run);
    assert!(!unchanged_decision.force_fetch);

    let changed_decision = decide_loc_sync(&changed, now, Some(recent_sweep), false);
    assert!(changed_decision.run);
    assert!(!changed_decision.force_fetch);
}

#[test]
fn managed_clone_scopes_branches_and_handles_empty_remote_and_default_branch_changes() {
    let root = std::env::temp_dir().join(format!("codetally-clone-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&root).unwrap();
    let git = |path: &std::path::Path, args: &[&str]| {
        let output = Command::new("git").current_dir(path).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let remote = root.join("remote.git");
    git(&root, &["init", "--bare", "--initial-branch=main", remote.to_str().unwrap()]);
    let mut repo = Repository { url: format!("file://{}", remote.display()), ..repository("clone", "clone") };
    let empty_cache = root.join("empty-cache");
    let cached = ensure_clone(&repo, &empty_cache).unwrap();
    assert!(current_commit_optional(&cached).unwrap().is_none());
    let source = root.join("source");
    git(&root, &["init", "--initial-branch=main", source.to_str().unwrap()]);
    git(&source, &["config", "user.email", "fixture@example.test"]);
    git(&source, &["config", "user.name", "Fixture"]);
    std::fs::write(source.join("main.rs"), "fn main() {}\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "main"]);
    git(&source, &["remote", "add", "origin", &repo.url]);
    git(&source, &["push", "origin", "main"]);
    let main_sha = git(&source, &["rev-parse", "HEAD"]);
    ensure_clone(&repo, &empty_cache).unwrap();
    assert_eq!(current_commit_optional(&cached).unwrap().unwrap().0, main_sha, "an empty cache must fetch its first commit");
    git(&source, &["checkout", "-b", "unused"]);
    std::fs::write(source.join("unused.rs"), "fn unused() {}\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "unused branch"]);
    git(&source, &["tag", "unused-tag"]);
    git(&source, &["push", "origin", "unused", "--tags"]);
    let unused_sha = git(&source, &["rev-parse", "HEAD"]);
    let fresh_cache = root.join("fresh-cache");
    let fresh = ensure_clone(&repo, &fresh_cache).unwrap();
    let refs = git(&fresh, &["for-each-ref", "--format=%(refname)"]);
    assert!(!refs.contains("origin/unused"));
    assert!(!refs.contains("refs/tags/"));
    git(&source, &["checkout", "main"]);
    std::fs::write(source.join("main.rs"), "fn main() { println!(\"new\"); }\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "main update"]);
    git(&source, &["push", "origin", "main"]);
    let latest_main = git(&source, &["rev-parse", "HEAD"]);
    ensure_clone(&repo, &fresh_cache).unwrap();
    assert_eq!(current_commit_optional(&fresh).unwrap().unwrap().0, latest_main);
    assert!(!git(&fresh, &["for-each-ref", "--format=%(refname)"]).contains("origin/unused"));
    repo.default_branch = "unused".into();
    ensure_clone(&repo, &fresh_cache).unwrap();
    assert_eq!(current_commit_optional(&fresh).unwrap().unwrap().0, unused_sha);
    assert_eq!(git(&fresh, &["symbolic-ref", "--short", "HEAD"]), "unused");
    // A fresh clone also follows discovery's branch when remote HEAD still
    // points at the old default branch.
    let renamed = ensure_clone(&repo, &root.join("renamed-cache")).unwrap();
    assert_eq!(current_commit_optional(&renamed).unwrap().unwrap().0, unused_sha);
    git(&remote, &["config", "receive.denyDeleteCurrent", "ignore"]);
    git(&source, &["push", "origin", ":main"]);
    let dangling_head = ensure_clone(&repo, &root.join("dangling-head-cache")).unwrap();
    assert_eq!(current_commit_optional(&dangling_head).unwrap().unwrap().0, unused_sha, "a deleted remote HEAD must not hide the valid discovered branch");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn automatic_loc_cadence_always_scans_new_or_incomplete_repositories() {
    let now: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().expect("cadence instant");
    let recent_sweep = Some("2026-09-08T11:30:00Z");
    let new_repository = repository("cadence-new", "new");
    let incomplete_repository = Repository {
        loc_backfill_complete: false,
        local_path: Some("/managed/cache/incomplete".to_string()),
        pushed_at: Some("2026-09-08T10:00:00Z".to_string()),
        last_fetched_pushed_at: Some("2026-09-08T10:00:00Z".to_string()),
        ..repository("cadence-incomplete", "incomplete")
    };

    assert!(decide_loc_sync(&new_repository, now, recent_sweep, false).run);
    assert!(decide_loc_sync(&incomplete_repository, now, recent_sweep, false).run);
}

#[test]
fn automatic_loc_cadence_runs_at_the_45_minute_boundary_and_manual_sync_forces_fetch() {
    let now: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().expect("cadence instant");
    let repository = completed_repository("cadence-boundary", "boundary", "2026-09-08T10:00:00Z", "2026-09-08T10:00:00Z");
    let before_boundary = decide_loc_sync(&repository, now, Some("2026-09-08T11:16:00Z"), false);
    assert!(!before_boundary.run);
    assert!(!before_boundary.force_fetch);

    let at_boundary = decide_loc_sync(&repository, now, Some("2026-09-08T11:15:00Z"), false);
    assert!(at_boundary.run);
    assert!(at_boundary.force_fetch);

    let manual = decide_loc_sync(&repository, now, Some("2026-09-08T11:59:00Z"), true);
    assert!(manual.run);
    assert!(manual.force_fetch);
}

#[test]
fn cadence_settings_have_expected_defaults_and_validate_supported_intervals() {
    let defaults = AppSettings::default();
    assert_eq!(defaults.activity_refresh_minutes, 10);
    assert_eq!(defaults.lines_refresh_minutes, 45);
    assert!(defaults.refresh_lines_on_change);
    assert!(sync::validate_app_settings(&defaults).is_ok());

    let invalid_activity = AppSettings {
        activity_refresh_minutes: 3,
        ..defaults.clone()
    };
    assert!(sync::validate_app_settings(&invalid_activity).is_err());
    let invalid_lines = AppSettings {
        lines_refresh_minutes: 31,
        ..defaults
    };
    assert!(sync::validate_app_settings(&invalid_lines).is_err());
}

#[test]
fn cadence_settings_round_trip_through_persisted_app_metadata() {
    let (database, path) = temp_database("cadence-settings");
    let configured = AppSettings {
        activity_refresh_minutes: 5,
        lines_refresh_minutes: 60,
        refresh_lines_on_change: false,
    };
    sync::save_app_settings(&database, &configured).expect("save cadence settings");
    database
        .set_metadata(sync::LOC_SWEEP_METADATA_KEY, "2026-09-08T12:00:00Z")
        .expect("save LOC sweep timestamp");

    let reopened = Database::new(&path);
    let loaded = sync::app_settings(&reopened).expect("load cadence settings");
    assert_eq!(loaded.activity_refresh_minutes, 5);
    assert_eq!(loaded.lines_refresh_minutes, 60);
    assert!(!loaded.refresh_lines_on_change);
    assert_eq!(
        reopened
            .metadata(sync::LOC_SWEEP_METADATA_KEY)
            .expect("load LOC sweep timestamp")
            .as_deref(),
        Some("2026-09-08T12:00:00Z")
    );
    remove_database(path);
}

#[test]
fn cadence_settings_disable_pushed_change_scans_but_keep_new_and_manual_scans() {
    let now: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().expect("cadence instant");
    let settings = AppSettings {
        refresh_lines_on_change: false,
        ..AppSettings::default()
    };
    let changed = completed_repository("cadence-setting-changed", "setting-changed", "2026-09-08T11:00:00Z", "2026-09-08T10:00:00Z");
    let new_repository = repository("cadence-setting-new", "setting-new");

    let changed_decision = sync::decide_loc_sync_with_settings(
        &changed,
        now,
        Some("2026-09-08T11:30:00Z"),
        false,
        &settings,
    );
    assert!(!changed_decision.run);
    assert!(!changed_decision.force_fetch);
    assert!(sync::decide_loc_sync_with_settings(&new_repository, now, Some("2026-09-08T11:30:00Z"), false, &settings).run);
    assert!(sync::decide_loc_sync_with_settings(&changed, now, Some("2026-09-08T11:30:00Z"), true, &settings).run);
    assert!(sync::decide_loc_sync_with_settings(&changed, now, Some("2026-09-08T11:30:00Z"), true, &settings).force_fetch);
}

#[test]
fn cadence_settings_control_the_periodic_sweep_boundary() {
    let now: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().expect("cadence instant");
    let repository = completed_repository("cadence-setting-boundary", "setting-boundary", "2026-09-08T10:00:00Z", "2026-09-08T10:00:00Z");
    let settings = AppSettings {
        lines_refresh_minutes: 60,
        ..AppSettings::default()
    };
    let before = sync::decide_loc_sync_with_settings(&repository, now, Some("2026-09-08T11:01:00Z"), false, &settings);
    assert!(!before.run);
    let at = sync::decide_loc_sync_with_settings(&repository, now, Some("2026-09-08T11:00:00Z"), false, &settings);
    assert!(at.run);
    assert!(at.force_fetch);
}

#[cfg(unix)]
#[test]
fn discovery_includes_org_repositories_deduplicates_ids_and_records_partial_org_failures() {
    use std::os::unix::fs::PermissionsExt;

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codetally-discovery-{}-{suffix}", std::process::id()));
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("fake gh directory");
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        r##"#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/../gh.log"
repo_json='[{"id":"owned-id","name":"portfolio","nameWithOwner":"me/portfolio","url":"https://github.com/me/portfolio","sshUrl":"git@github.com:me/portfolio.git","isPrivate":false,"isFork":false,"isArchived":false,"defaultBranchRef":{"name":"main"},"primaryLanguage":{"name":"Rust"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","pushedAt":"2026-09-02T00:00:00Z"}]'
org_json='[{"id":"org-id","name":"shared","nameWithOwner":"org-one/shared","url":"https://github.com/org-one/shared","sshUrl":"git@github.com:org-one/shared.git","isPrivate":true,"isFork":false,"isArchived":false,"defaultBranchRef":{"name":"main"},"primaryLanguage":{"name":"TypeScript"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","pushedAt":"2026-09-02T00:00:00Z"},{"id":"owned-id","name":"duplicate","nameWithOwner":"org-one/duplicate","url":"https://github.com/org-one/duplicate","sshUrl":"git@github.com:org-one/duplicate.git","isPrivate":false,"isFork":false,"isArchived":false,"defaultBranchRef":{"name":"main"},"primaryLanguage":{"name":"Rust"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","pushedAt":"2026-09-02T00:00:00Z"}]'
if [ "$1" = "api" ] && [ "$2" = "user" ]; then printf 'me\n'; exit 0; fi
if [ "$1" = "api" ] && [ "$2" = "graphql" ]; then printf '{"data":{"viewer":{"login":"me"},"rateLimit":{"remaining":5000,"resetAt":"2099-01-01T00:00:00Z"}}}\n'; exit 0; fi
if [ "$1" = "api" ] && [ "$2" = "--paginate" ]; then printf 'org-one\norg-two\n'; exit 0; fi
if [ "$1" = "repo" ] && [ "$3" = "me" ]; then printf '%s\n' "$repo_json"; exit 0; fi
if [ "$1" = "repo" ] && [ "$3" = "org-one" ]; then printf '%s\n' "$org_json"; exit 0; fi
if [ "$1" = "repo" ] && [ "$3" = "org-two" ]; then printf 'permission denied\n' >&2; exit 1; fi
printf 'unexpected fake gh invocation\n' >&2
exit 1
"##,
    )
    .expect("fake gh script");
    let mut permissions = std::fs::metadata(&gh).expect("fake gh metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&gh, permissions).expect("fake gh executable");

    let database_path = root.join("portfolio.sqlite3");
    let cache_path = root.join("cache");
    let database = Database::new(&database_path);
    database.init().expect("database schema");
    let state = AppState {
        db_path: database_path,
        cache_dir: cache_path,
        progress: Arc::new(Mutex::new(SyncProgress::default())),
        dashboard_cache: Arc::new(Mutex::new(Default::default())), job_lock: Arc::new(Mutex::new(())),
    };

    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(&original_path));
    std::env::set_var("PATH", std::env::join_paths(paths).expect("PATH").to_string_lossy().to_string());
    let discovered = sync::discover(&state);
    std::env::set_var("PATH", original_path);

    let repositories = discovered.expect("discovery should retain usable owners");
    let calls = std::fs::read_to_string(root.join("gh.log")).expect("discovery calls");
    assert!(!calls.lines().any(|call| call.starts_with("api user")), "viewer identity should share the quota request");
    assert!(calls.contains("user/orgs?per_page=100"), "organization discovery should use full REST pages");
    assert_eq!(database.metadata("github_login").unwrap().as_deref(), Some("me"));
    let errors = state.database().metadata("org_discovery_errors").expect("org error metadata").unwrap_or_default();
    let names = repositories.iter().map(|repo| repo.name_with_owner.as_str()).collect::<Vec<_>>();
    assert_eq!(repositories.len(), 2);
    assert_eq!(names, vec!["me/portfolio", "org-one/shared"]);
    assert!(errors.contains("org-two"), "partial org failure should remain visible: {errors}");
    assert_eq!(state.database().repositories().expect("persisted repositories").len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tokei_reports_are_classified_into_source_and_tests_without_docs() {
    let output = json!({
        "Rust": {
            "code": 99,
            "reports": [
                {"name": "src/lib.rs", "stats": {"code": 10}},
                {"name": "tests/lib_test.rs", "stats": {"code": 4}}
            ]
        },
        "TypeScript": {
            "reports": [
                {"name": "src/app.ts", "code": 7},
                {"name": "src/app.test.ts", "code": 3}
            ]
        },
        "Vue": {
            "reports": [{
                "name": "src/app.vue",
                "stats": {
                    "code": 2,
                    "blobs": {
                        "JavaScript": {"code": 8, "blobs": {}},
                        "CSS": {"code": 3, "blobs": {}}
                    }
                }
            }]
        },
        "Markdown": {"reports": [{
            "name": "README.md",
            "stats": {"code": 0, "blobs": {"Rust": {"code": 500, "blobs": {}}}}
        }]},
        "Total": {"code": 523}
    });

    assert!(is_tokei_report(&output));
    assert!(!is_tokei_report(&json!({"Rust": {"code": 99}})));
    assert!(is_tokei_report(&json!({})));
    assert_eq!(classify_tokei_json(&output), (37, 30, 7));

    let custom = ClassificationConfig {
        test_paths: vec!["fixtures/unit".to_string()],
        test_patterns: vec!["*integration*.snap".to_string()],
        excluded_directories: Vec::new(),
    };
    assert!(is_test_path_with_config("src/fixtures/unit/case.ts", &custom));
    assert!(is_test_path_with_config("snapshots/api-integration.snap", &custom));
    assert!(!is_test_path_with_config("snapshots/api-integration.snap.bak", &custom));
}

#[cfg(unix)]
#[test]
fn worktree_scanner_counts_command_and_extensionless_shell_files_without_exclusions_or_comments() {
    use std::os::unix::fs::PermissionsExt;

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "codetally-shell-counts-{}-{suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("tests")).expect("test scripts directory");
    std::fs::create_dir_all(root.join("scripts")).expect("helper scripts directory");
    std::fs::create_dir_all(root.join("vendor")).expect("excluded scripts directory");
    std::fs::create_dir_all(root.join(".github/workflows")).expect("hidden workflow directory");
    std::fs::create_dir_all(root.join(".repowise")).expect("repowise directory");

    let command = root.join("build_image.command");
    std::fs::write(
        &command,
        "#!/bin/sh\n# command comment\n\necho source-one\necho source-two\n",
    )
    .expect("command script");
    let extensionless = root.join("deploy");
    std::fs::write(
        &extensionless,
        "#!/usr/bin/env bash\n# extensionless comment\nprintf '%s\\n' one\n\nprintf '%s\\n' two\n",
    )
    .expect("extensionless script");
    let multiline = root.join("multiline.command");
    std::fs::write(
        &multiline,
        "#!/bin/sh\nprintf '%s\\n' \"start\"\nmessage=\"first line\n# this line belongs to the quoted string\nlast line\"\nprintf '%s\\n' \"$message\"\n# actual comment\n",
    )
    .expect("multiline shell script");
    let build_script = root.join("build.sh");
    std::fs::write(&build_script, "#!/bin/sh\necho build\n")
        .expect("build shell script");
    let dependency_script = root.join("scripts/check.sh");
    std::fs::write(&dependency_script, "#!/bin/sh\necho dependency\n")
        .expect("dependency shell script");
    std::fs::write(root.join("scripts/common"), "COMMON_VALUE=1\n")
        .expect("common helper");
    std::fs::write(root.join("scripts/dependencies_check"), "DEPENDENCY_VALUE=1\n")
        .expect("dependency helper");
    let test_script = root.join("tests/check");
    std::fs::write(
        &test_script,
        "#!/bin/zsh\n# test comment\necho test-one\necho test-two\n",
    )
    .expect("test script");
    std::fs::write(
        root.join("vendor/ignored.command"),
        "#!/bin/sh\necho excluded\n",
    )
    .expect("excluded script");
    std::fs::write(root.join(".gitignore"), "ignored.command\n")
        .expect("gitignore fixture");
    std::fs::write(root.join(".tokeignore"), "tokei_ignored.command\n")
        .expect("tokeignore fixture");
    std::fs::write(root.join(".ignore"), "generic_ignored.command\n")
        .expect("ignore fixture");
    std::fs::write(
        root.join("ignored.command"),
        "#!/bin/sh\necho ignored-one\necho ignored-two\n",
    )
    .expect("ignored tracked script");
    std::fs::write(
        root.join("tokei_ignored.command"),
        "#!/bin/sh\necho ignored-by-tokei\n",
    )
    .expect("tokeignore script");
    std::fs::write(
        root.join("generic_ignored.command"),
        "#!/bin/sh\necho ignored-by-generic-ignore\n",
    )
    .expect("ignore script");
    std::fs::write(root.join("README.md"), "Documentation line\n")
        .expect("documentation fixture");
    std::fs::write(root.join("binary.bin"), [0_u8, 1, 2, 3])
        .expect("binary fixture");
    std::fs::write(root.join(".github/workflows/build.yml"), "name: Build\non: push\n")
        .expect("hidden workflow fixture");
    std::fs::write(root.join(".repowise/local.json"), "{\"cache\":true}\n")
        .expect("repowise fixture");

    for path in [
        &command,
        &extensionless,
        &multiline,
        &build_script,
        &dependency_script,
        &test_script,
    ] {
        let mut permissions = std::fs::metadata(path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("script permissions");
    }

    let git = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(&root)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git command failed: {args:?}");
    };
    git(&["init"]);
    git(&["config", "user.email", "portfolio@example.test"]);
    git(&["config", "user.name", "Portfolio Tests"]);
    git(&["add", "."]);
    // A tracked file can still match .gitignore. Tokei excludes it, so the
    // fallback must apply the same ignore semantics rather than counting it
    // merely because `git ls-files` reports it.
    git(&["add", "-f", "ignored.command"]);
    git(&["add", "-f", "tokei_ignored.command"]);
    git(&["add", "-f", "generic_ignored.command"]);
    git(&["commit", "-m", "shell fixtures"]);

    let scan = scan_worktree_with_config(&root, None).expect("shell-aware worktree scan");
    // The multiline command file contains a `#` line inside a quoted string;
    // Tokei counts that as code, so the fallback must preserve parser parity.
    assert_eq!(scan.total_loc, 17, "comments and blank lines must not count");
    assert_eq!(scan.source_loc, 15, "hidden workflow, shell commands, helpers, and extensionless source files must count");
    assert_eq!(scan.test_loc, 2, "test-directory shell files must remain test LOC");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn historical_sample_dates_are_monthly_and_cap_at_the_requested_now() {
    let end: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().expect("date");
    let dates = month_sample_dates(Some("2026-01-17"), end);
    assert_eq!(dates.len(), 9);
    assert_eq!(dates.first().expect("first month"), "2026-01-31T23:59:59Z");
    assert_eq!(dates.get(1).expect("second month"), "2026-02-28T23:59:59Z");
    assert_eq!(dates.last().expect("current month"), &end.to_rfc3339());
    assert!(dates.windows(2).all(|pair| pair[0] < pair[1]));

    let range = range_start(Some("3m"), end).expect("3 month range");
    assert_eq!(range, "2026-06-08T12:00:00Z");
    assert!(range_start(Some("all"), end).is_none());
}

#[test]
fn sqlite_snapshot_upserts_and_lookup_drive_net_growth() {
    let (database, path) = temp_database("snapshots");
    let repo_id = database
        .upsert_repository(&repository("repo-1", "portfolio"))
        .expect("repository upsert");
    let now = Utc::now();
    let old = now - Duration::days(45);
    let old_date = old.to_rfc3339();
    // Keep the current commit just behind the clock so a later reset
    // observation can be inserted deterministically without sleeping.
    let current_date = (now - Duration::days(1)).to_rfc3339();
    assert!(database
        .upsert_snapshot(&snapshot(repo_id, "old-sha", &old_date, 100))
        .expect("first snapshot"));
    assert!(database
        .upsert_snapshot(&snapshot(repo_id, "head-sha", &current_date, 150))
        .expect("current snapshot"));
    assert!(!database
        .upsert_snapshot(&snapshot(repo_id, "head-sha", &current_date, 999))
        .expect("duplicate snapshot is ignored"));

    assert_eq!(database.latest_snapshot(repo_id).expect("latest").expect("snapshot").total_loc, 150);
    assert_eq!(database.snapshot_at_or_before(repo_id, &current_date).expect("lookup").expect("snapshot").total_loc, 150);
    assert_eq!(database.snapshot_at_or_before(repo_id, &old_date).expect("lookup").expect("snapshot").total_loc, 100);

    let observation_date = (now - Duration::days(10)).to_rfc3339();
    let mut observation = snapshot(repo_id, "head-sha", &observation_date, 140);
    assert!(database.upsert_observation(&observation).expect("first daily observation"));
    observation.total_loc = 145;
    observation.source_loc = 125;
    assert!(database.upsert_observation(&observation).expect("same-day observation update"));
    assert_eq!(database.latest_observation(repo_id).expect("latest observation").expect("observation").total_loc, 145);
    let before_current_snapshot = (now - Duration::days(5)).to_rfc3339();
    assert_eq!(database.measurement_at_or_before(repo_id, &before_current_snapshot).expect("measurement lookup").expect("measurement").total_loc, 145);

    database.set_fetched_pushed_at(repo_id, Some("2026-09-08T00:00:00Z")).expect("fetch cursor");
    assert_eq!(database.repository(repo_id).expect("repository").expect("row").last_fetched_pushed_at.as_deref(), Some("2026-09-08T00:00:00Z"));

    let summary = database
        .summaries()
        .expect("summary")
        .into_iter()
        .find(|item| item.id == repo_id)
        .expect("summary row");
    assert_eq!(summary.total_loc, 150);
    assert_eq!(summary.loc_change_30d, 50);
    assert!((summary.loc_change_30d_percent - 50.0).abs() < 0.01);

    // A force-push/reset can make the current HEAD reuse an already cached
    // SHA. A newer daily observation must then become the current measurement,
    // while the older observation must not mask the newer snapshot above.
    let reset_date = now.to_rfc3339();
    let reset = snapshot(repo_id, "old-sha", &reset_date, 145);
    assert!(database.upsert_observation(&reset).expect("reset observation"));
    let reset_summary = database
        .summaries()
        .expect("summary after reset")
        .into_iter()
        .find(|item| item.id == repo_id)
        .expect("summary row after reset");
    assert_eq!(reset_summary.total_loc, 145);
    assert_eq!(reset_summary.loc_change_30d, 45);
    assert!((reset_summary.loc_change_30d_percent - 45.0).abs() < 0.01);
    remove_database(path);
}

#[test]
fn sqlite_upserts_preserve_one_repository_and_update_activity_rows() {
    let (database, path) = temp_database("upserts");
    let mut first = repository("repo-1", "portfolio");
    let repo_id = database.upsert_repository(&first).expect("first repository");
    first.id = repo_id;
    first.primary_language = Some("Rust".to_string());
    first.name = "portfolio-renamed".to_string();
    first.name_with_owner = "owner/portfolio-renamed".to_string();
    database.upsert_repository(&first).expect("repository update");
    let repositories = database.repositories().expect("repositories");
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0].primary_language.as_deref(), Some("Rust"));
    assert_eq!(repositories[0].name, "portfolio-renamed");

    let mut pull_request = PullRequest {
        repository_id: repo_id,
        repository: "owner/portfolio-renamed".to_string(),
        number: 3,
        title: "Old title".to_string(),
        state: "OPEN".to_string(),
        created_at: "2026-09-01T00:00:00Z".to_string(),
        updated_at: "2026-09-01T00:00:00Z".to_string(),
        url: "https://github.com/owner/portfolio/pull/3".to_string(),
        ..PullRequest::default()
    };
    database.upsert_pull_request(&pull_request).expect("PR insert");
    pull_request.title = "New title".to_string();
    pull_request.updated_at = "2026-09-02T00:00:00Z".to_string();
    database.upsert_pull_request(&pull_request).expect("PR update");
    let prs = database.pull_requests(Some(repo_id), None, 10).expect("PR feed");
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].title, "New title");

    let mut issue = Issue {
        repository_id: repo_id,
        repository: "owner/portfolio-renamed".to_string(),
        number: 4,
        title: "Cache issue".to_string(),
        state: "OPEN".to_string(),
        created_at: "2026-09-01T00:00:00Z".to_string(),
        updated_at: "2026-09-03T00:00:00Z".to_string(),
        url: "https://github.com/owner/portfolio/issues/4".to_string(),
        labels: vec!["bug".to_string()],
        assignees: vec!["maintainer".to_string()],
        ..Issue::default()
    };
    database.upsert_issue(&issue).expect("issue insert");
    issue.state = "CLOSED".to_string();
    database.upsert_issue(&issue).expect("issue update");
    let issues = database.issues(Some(repo_id), Some("CLOSED"), 10).expect("issue feed");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].labels, vec!["bug"]);
    assert_eq!(issues[0].assignees, vec!["maintainer"]);
    remove_database(path);
}

#[test]
fn activity_feeds_sort_newest_first_and_apply_state_and_repository_filters() {
    let (database, path) = temp_database("feeds");
    let repo_a = database.upsert_repository(&repository("repo-a", "alpha")).expect("repo a");
    let repo_b = database.upsert_repository(&repository("repo-b", "beta")).expect("repo b");
    let pr_a = PullRequest { repository_id: repo_a, repository: "owner/alpha".to_string(), number: 1, title: "Older PR".to_string(), state: "OPEN".to_string(), created_at: "2026-09-01T00:00:00Z".to_string(), updated_at: "2026-09-01T00:00:00Z".to_string(), ..PullRequest::default() };
    let pr_b = PullRequest { repository_id: repo_b, repository: "owner/beta".to_string(), number: 2, title: "Newest PR".to_string(), state: "MERGED".to_string(), created_at: "2026-09-01T00:00:00Z".to_string(), updated_at: "2026-09-03T00:00:00Z".to_string(), ..PullRequest::default() };
    database.upsert_pull_request(&pr_a).expect("PR a");
    database.upsert_pull_request(&pr_b).expect("PR b");
    let prs = database.pull_requests(None, None, 10).expect("all PRs");
    assert_eq!(prs.iter().map(|item| item.title.as_str()).collect::<Vec<_>>(), vec!["Newest PR", "Older PR"]);
    assert_eq!(database.pull_requests(Some(repo_a), Some("OPEN"), 10).expect("repo/state PR filter").len(), 1);
    assert!(database.pull_requests(Some(repo_a), Some("MERGED"), 10).expect("repo/state PR filter").is_empty());

    let issue_a = Issue { repository_id: repo_a, repository: "owner/alpha".to_string(), number: 5, title: "Open issue".to_string(), state: "OPEN".to_string(), created_at: "2026-09-01T00:00:00Z".to_string(), updated_at: "2026-09-04T00:00:00Z".to_string(), ..Issue::default() };
    let issue_b = Issue { repository_id: repo_b, repository: "owner/beta".to_string(), number: 6, title: "Closed issue".to_string(), state: "CLOSED".to_string(), created_at: "2026-09-01T00:00:00Z".to_string(), updated_at: "2026-09-05T00:00:00Z".to_string(), ..Issue::default() };
    database.upsert_issue(&issue_a).expect("issue a");
    database.upsert_issue(&issue_b).expect("issue b");
    let activity = database.all_activity("issues", Some(repo_b), Some("CLOSED"), 10).expect("filtered issue activity");
    assert!(matches!(&activity[..], [ActivityItem::Issue(item)] if item.title == "Closed issue"));
    remove_database(path);
}

#[test]
fn repository_star_and_fork_counts_round_trip_and_appear_in_summary() {
    let (database, path) = temp_database("repository-counts");
    let repo_id = database
        .upsert_repository(&Repository {
            star_count: 123,
            fork_count: 45,
            ..repository("repo-counts", "counts")
        })
        .expect("repository");

    let stored = database
        .repository(repo_id)
        .expect("repository lookup")
        .expect("stored repository");
    assert_eq!(stored.star_count, 123);
    assert_eq!(stored.fork_count, 45);

    let summary = database
        .summaries()
        .expect("repository summaries")
        .into_iter()
        .find(|item| item.id == repo_id)
        .expect("repository summary");
    assert_eq!(summary.star_count, 123);
    assert_eq!(summary.fork_count, 45);
    remove_database(path);
}

#[test]
fn sqlite_init_adds_social_counts_to_legacy_repositories_without_losing_history() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "codetally-legacy-counts-{}-{suffix}.sqlite3",
        std::process::id()
    ));
    let connection = Connection::open(&path).expect("legacy database");
    connection
        .execute_batch(
            r#"
            CREATE TABLE repositories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                github_id TEXT NOT NULL UNIQUE,
                owner TEXT NOT NULL,
                name TEXT NOT NULL,
                name_with_owner TEXT NOT NULL UNIQUE,
                url TEXT NOT NULL,
                ssh_url TEXT NOT NULL,
                default_branch TEXT NOT NULL DEFAULT 'main',
                primary_language TEXT,
                is_private INTEGER NOT NULL DEFAULT 0,
                is_fork INTEGER NOT NULL DEFAULT 0,
                is_archived INTEGER NOT NULL DEFAULT 0,
                created_at TEXT,
                github_updated_at TEXT,
                pushed_at TEXT,
                local_path TEXT,
                last_sync_at TEXT,
                last_error TEXT,
                loc_backfill_complete INTEGER NOT NULL DEFAULT 0,
                open_pr_count INTEGER NOT NULL DEFAULT 0,
                open_issue_count INTEGER NOT NULL DEFAULT 0,
                open_counts_synced INTEGER NOT NULL DEFAULT 0,
                last_fetched_pushed_at TEXT
            );
            CREATE TABLE code_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repository_id INTEGER NOT NULL,
                commit_sha TEXT NOT NULL,
                commit_date TEXT NOT NULL,
                snapshot_date TEXT NOT NULL,
                total_loc INTEGER NOT NULL DEFAULT 0,
                source_loc INTEGER NOT NULL DEFAULT 0,
                test_loc INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                UNIQUE(repository_id, commit_sha)
            );
            CREATE TABLE app_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO app_metadata(key, value) VALUES
                ('loc_analysis_version', '3'),
                ('history_sampling_version', '1');
            "#,
        )
        .expect("legacy schema");
    connection
        .execute(
            "INSERT INTO repositories (github_id,owner,name,name_with_owner,url,ssh_url,local_path,loc_backfill_complete) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                "legacy-counts",
                "owner",
                "counts",
                "owner/counts",
                "https://github.com/owner/counts",
                "git@github.com:owner/counts.git",
                "/managed/cache/counts",
                true,
            ],
        )
        .expect("legacy repository");
    let repository_id = connection.last_insert_rowid();
    connection
        .execute(
            "INSERT INTO code_snapshots (repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                repository_id,
                "legacy-sha",
                "2024-06-30T23:59:59Z",
                "2024-06-30T23:59:59Z",
                77,
                60,
                17,
                "2024-06-30T23:59:59Z",
            ],
        )
        .expect("legacy history snapshot");
    drop(connection);

    let database = Database::new(&path);
    database.init().expect("legacy migration");
    let migrated_repo = database
        .repository(repository_id)
        .expect("repository lookup")
        .expect("repository survives migration");
    assert_eq!(migrated_repo.star_count, 0);
    assert_eq!(migrated_repo.fork_count, 0);
    assert!(migrated_repo.loc_backfill_complete);
    assert_eq!(migrated_repo.local_path.as_deref(), Some("/managed/cache/counts"));
    let snapshot = database
        .snapshot_for_commit(repository_id, "legacy-sha")
        .expect("history lookup")
        .expect("history survives migration");
    assert_eq!(snapshot.total_loc, 77);
    assert_eq!(snapshot.source_loc, 60);
    assert_eq!(snapshot.test_loc, 17);
    remove_database(path);
}

#[test]
fn history_carries_forward_snapshots_and_totals_exclude_forks_and_archived_repositories() {
    let (database, path) = temp_database("history");
    let active = database.upsert_repository(&repository("active", "active")).expect("active");
    let fork = database.upsert_repository(&Repository { is_fork: true, github_id: "fork".into(), name: "fork".into(), name_with_owner: "owner/fork".into(), ..repository("fork", "fork") }).expect("fork");
    let archived = database.upsert_repository(&Repository { is_archived: true, github_id: "archived".into(), name: "archived".into(), name_with_owner: "owner/archived".into(), ..repository("archived", "archived") }).expect("archived");
    database.upsert_snapshot(&snapshot(active, "active-jan", "2026-01-31T23:59:59Z", 100)).expect("active Jan");
    database.upsert_snapshot(&snapshot(active, "active-feb", "2026-02-28T23:59:59Z", 125)).expect("active Feb");
    database.upsert_snapshot(&snapshot(fork, "fork-feb", "2026-02-28T23:59:59Z", 900)).expect("fork Feb");
    database.upsert_snapshot(&snapshot(archived, "archived-feb", "2026-02-28T23:59:59Z", 700)).expect("archived Feb");
    let history = database.history(None).expect("portfolio history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].total_loc, 100);
    assert_eq!(history[1].total_loc, 125);
    let summaries = database.summaries().expect("summaries");
    let totals = database.totals(&summaries);
    assert_eq!(totals.repositories, 2);
    assert_eq!(totals.total_loc, 125);
    // Daily observations take precedence over a monthly sample on that day,
    // and only eligible repository dates appear in portfolio history.
    database.upsert_observation(&snapshot(active, "observed", "2026-02-28T10:00:00Z", 140)).unwrap();
    database.upsert_snapshot(&snapshot(active, "later", "2026-03-01T23:59:59Z", 160)).unwrap();
    database.upsert_observation(&snapshot(active, "next", "2026-03-03T09:00:00Z", 155)).unwrap();
    database.upsert_snapshot(&snapshot(fork, "only-fork", "2026-03-02T23:59:59Z", 950)).unwrap();
    let history = database.history(None).unwrap();
    assert_eq!(history.iter().map(|p| p.total_loc).collect::<Vec<_>>(), vec![100, 140, 160, 155]);
    assert_eq!(database.history(Some(fork)).unwrap().last().unwrap().total_loc, 950);
    assert!(database.history(Some(i64::MAX)).unwrap().is_empty());
    remove_database(path);
}

#[test]
fn large_portfolio_history_and_summaries_preserve_daily_totals() {
    let (database, path) = temp_database("portfolio-scale");
    let mut conn = Connection::open(&path).expect("fixture connection");
    let tx = conn.transaction().expect("fixture transaction");
    for repo in 1..=1000 {
        tx.execute("INSERT INTO repositories(id,github_id,owner,name,name_with_owner,url,ssh_url) VALUES (?1,?1,'owner',?1,?1,'','')", [repo]).unwrap();
        for day in 0..180 {
            let date = (Utc::now() - Duration::days(180 - day)).format("%Y-%m-%d").to_string();
            tx.execute("INSERT INTO code_snapshots(repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at) VALUES (?1,?2,?2,?2,?3,?3,0,?2)", params![repo,date,100 + day]).unwrap();
        }
    }
    tx.commit().unwrap();
    let start = std::time::Instant::now();
    let history = database.history(None).expect("large history");
    eprintln!("1,000 repos / 180,000 samples: history {:?}", start.elapsed());
    assert_eq!(history.len(), 180);
    for (day, point) in history.iter().enumerate() {
        assert_eq!(point.total_loc, 1000 * (100 + day as i64));
        assert_eq!(point.source_loc, point.total_loc);
        assert_eq!(point.test_loc, 0);
    }
    let start = std::time::Instant::now();
    let summaries = database.summaries().expect("large summaries");
    eprintln!("1,000 repos: summaries {:?}", start.elapsed());
    assert_eq!(summaries.len(), 1000);
    assert!(summaries.iter().all(|repo| repo.total_loc == 279 && repo.loc_baseline_90d_available));
    assert_eq!(database.history(Some(1)).unwrap().last().unwrap().total_loc, 279);
    let mut cache = DashboardCache::default();
    let start = std::time::Instant::now();
    let dashboard = database.dashboard_cached(&mut cache).unwrap();
    eprintln!("1,000 repos: dashboard build {:?}", start.elapsed());
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let repeated = database.dashboard_cached(&mut cache).unwrap();
        assert!(Arc::ptr_eq(&dashboard, &repeated));
        assert_eq!(repeated.totals.total_loc, 279_000);
    }
    eprintln!("1,000 repos: ten unchanged dashboard reads {:?}", start.elapsed());
    drop(cache);
    drop(conn);
    remove_database(path);
}

#[test]
fn activity_pages_are_atomic_skip_unchanged_rows_and_update_ci_without_timestamp_changes() {
    let (database, path) = temp_database("activity-page");
    let id = database.upsert_repository(&repository("page", "page")).unwrap();
    let prs: Vec<_> = (1..=100).map(|number| PullRequest {
        repository_id: id, number, title: format!("PR {number}"), state: "OPEN".into(),
        created_at: "2026-01-01".into(), updated_at: "2026-01-02".into(), ci_state: Some("pending".into()),
        ..PullRequest::default()
    }).collect();
    let issues = vec![Issue { repository_id: id, number: 1, title: "Issue".into(), state: "OPEN".into(), ..Issue::default() }];
    let unbatched_id = database.upsert_repository(&repository("unbatched", "unbatched")).unwrap();
    let start = std::time::Instant::now();
    for pr in &prs { database.upsert_pull_request(&PullRequest { repository_id: unbatched_id, ..pr.clone() }).unwrap(); }
    eprintln!("100 PRs: individual commits {:?}", start.elapsed());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE updates(kind TEXT); CREATE TRIGGER track_pr AFTER UPDATE ON pull_requests BEGIN INSERT INTO updates VALUES ('pr'); END; CREATE TRIGGER track_issue AFTER UPDATE ON issues BEGIN INSERT INTO updates VALUES ('issue'); END;").unwrap();
    let start = std::time::Instant::now();
    database.apply_activity_page(&prs, &issues, id, 100, 1, "page_cursor", "first").unwrap();
    eprintln!("100 PRs + issue: transactional page {:?}", start.elapsed());
    database.apply_activity_page(&prs, &issues, id, 100, 1, "page_cursor", "first").unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM updates", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    let mut changed = prs[0].clone();
    changed.ci_state = Some("success".into());
    database.apply_activity_page(&[changed.clone()], &[], id, 100, 1, "page_cursor", "second").unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM updates", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(database.pull_requests(Some(id), None, 100).unwrap().iter().find(|pr| pr.number == 1).unwrap().ci_state.as_deref(), Some("success"));
    changed.title = "must roll back".into();
    let invalid = Issue { repository_id: id + 9999, ..issues[0].clone() };
    assert!(database.apply_activity_page(&[changed], &[invalid], id, 0, 0, "page_cursor", "failed").is_err());
    assert_eq!(database.metadata("page_cursor").unwrap().as_deref(), Some("second"));
    assert_eq!(database.repository(id).unwrap().unwrap().open_pr_count, 100);
    assert_eq!(database.pull_requests(Some(id), None, 100).unwrap().iter().find(|pr| pr.number == 1).unwrap().title, "PR 1");
    drop(conn);
    remove_database(path);
}

#[test]
fn classification_configuration_round_trips_and_invalidates_backfill() {
    let (database, path) = temp_database("classification");
    let repo_id = database.upsert_repository(&repository("repo-rules", "rules")).expect("repo");
    database.set_backfill_complete(repo_id, true).expect("mark complete");
    let config = ClassificationConfig {
        test_paths: vec!["checks".to_string()],
        test_patterns: vec!["*.fixture.ts".to_string()],
        excluded_directories: vec!["generated".to_string()],
    };
    database.set_classification_config(repo_id, &config).expect("set config");
    assert_eq!(database.classification_config(repo_id).expect("get config").test_paths, vec!["checks"]);
    assert!(!database.repository(repo_id).expect("repo").expect("row").loc_backfill_complete);
    remove_database(path);
}

#[test]
fn sqlite_init_migrates_legacy_loc_analysis_without_losing_repository_data() {
    let (database, path) = temp_database("migration");
    let repo_id = database
        .upsert_repository(&repository("repo-migration", "migration"))
        .expect("repository");
    database
        .set_local_path(repo_id, "/managed/cache/migration")
        .expect("local cache path");
    database.set_backfill_complete(repo_id, true).expect("backfill flag");
    database
        .upsert_snapshot(&snapshot(repo_id, "legacy-sha", "2026-01-31T23:59:59Z", 80))
        .expect("legacy snapshot");
    database
        .upsert_observation(&snapshot(repo_id, "legacy-sha", "2026-09-07T12:00:00Z", 80))
        .expect("legacy observation");
    database
        .upsert_pull_request(&PullRequest {
            repository_id: repo_id,
            repository: "owner/migration".to_string(),
            number: 11,
            title: "Keep this PR".to_string(),
            state: "OPEN".to_string(),
            created_at: "2026-09-01T00:00:00Z".to_string(),
            updated_at: "2026-09-02T00:00:00Z".to_string(),
            ..PullRequest::default()
        })
        .expect("PR");
    database
        .upsert_issue(&Issue {
            repository_id: repo_id,
            repository: "owner/migration".to_string(),
            number: 12,
            title: "Keep this issue".to_string(),
            state: "OPEN".to_string(),
            created_at: "2026-09-01T00:00:00Z".to_string(),
            updated_at: "2026-09-03T00:00:00Z".to_string(),
            ..Issue::default()
        })
        .expect("issue");

    database
        .set_metadata("loc_analysis_version", "1")
        .expect("legacy analysis version");
    database.init().expect("legacy migration");
    assert!(database.latest_snapshot(repo_id).expect("snapshot lookup").is_none());
    assert!(database.latest_observation(repo_id).expect("observation lookup").is_none());
    let migrated_repo = database
        .repository(repo_id)
        .expect("repository lookup")
        .expect("repository survives migration");
    assert_eq!(migrated_repo.local_path.as_deref(), Some("/managed/cache/migration"));
    assert!(!migrated_repo.loc_backfill_complete);
    assert_eq!(database.pull_requests(Some(repo_id), None, 10).expect("PR survives").len(), 1);
    assert_eq!(database.issues(Some(repo_id), None, 10).expect("issue survives").len(), 1);
    assert_eq!(database.metadata("loc_analysis_version").expect("version").as_deref(), Some("3"));

    database
        .upsert_snapshot(&snapshot(repo_id, "new-sha", "2026-09-08T12:00:00Z", 90))
        .expect("new snapshot");
    database.init().expect("current-version init");
    assert_eq!(database.latest_snapshot(repo_id).expect("new snapshot lookup").expect("snapshot").total_loc, 90);
    assert_eq!(database.pull_requests(Some(repo_id), None, 10).expect("PR after repeat init").len(), 1);
    assert_eq!(database.issues(Some(repo_id), None, 10).expect("issue after repeat init").len(), 1);
    remove_database(path);
}

#[test]
fn sqlite_init_invalidates_cached_loc_counts_when_shell_parser_version_changes() {
    let (database, path) = temp_database("shell-parser-version");
    let repo_id = database
        .upsert_repository(&repository("repo-shell-parser", "shell-parser"))
        .expect("repository");
    database.set_backfill_complete(repo_id, true).expect("backfill flag");
    database
        .upsert_snapshot(&snapshot(repo_id, "cached-sha", "2026-09-08T12:00:00Z", 973))
        .expect("cached LOC snapshot");
    database
        .upsert_observation(&snapshot(repo_id, "cached-sha", "2026-09-08T12:00:00Z", 973))
        .expect("cached LOC observation");

    // Version 2 is the pre-shell-fallback analysis. A later init must discard
    // both derived LOC tables so the same SHA is rescanned with the corrected
    // parser instead of reusing its incomplete count.
    database
        .set_metadata("loc_analysis_version", "2")
        .expect("pre-fallback analysis version");
    database.init().expect("shell parser migration");
    assert!(database.latest_snapshot(repo_id).expect("snapshot lookup").is_none());
    assert!(database.latest_observation(repo_id).expect("observation lookup").is_none());
    assert!(!database
        .repository(repo_id)
        .expect("repository lookup")
        .expect("repository")
        .loc_backfill_complete);
    assert_ne!(
        database
            .metadata("loc_analysis_version")
            .expect("analysis version")
            .as_deref(),
        Some("2")
    );
    remove_database(path);
}

#[test]
fn history_sampling_reuses_counts_for_earlier_sha_and_migrates_without_deleting_data() {
    let (database, path) = temp_database("history-sampling");
    let repo_id = database
        .upsert_repository(&repository("repo-history-sampling", "history-sampling"))
        .expect("repository");
    database
        .set_local_path(repo_id, "/managed/cache/history-sampling")
        .expect("local cache path");
    database
        .set_backfill_complete(repo_id, true)
        .expect("backfill flag");

    let original = snapshot(repo_id, "reused-sha", "2024-07-31T12:00:00Z", 90);
    database
        .upsert_snapshot(&original)
        .expect("initial historical snapshot");
    assert!(database
        .move_snapshot_date_earlier(repo_id, "reused-sha", "2024-06-30T23:59:59Z")
        .expect("move reused snapshot earlier"));

    let moved = database
        .snapshot_for_commit(repo_id, "reused-sha")
        .expect("moved snapshot lookup")
        .expect("moved snapshot");
    assert_eq!(moved.snapshot_date, "2024-06-30T23:59:59Z");
    assert_eq!(moved.commit_date, original.commit_date);
    assert_eq!(moved.total_loc, 90);
    assert_eq!(moved.source_loc, 70);
    assert_eq!(moved.test_loc, 20);
    assert!(!database
        .move_snapshot_date_earlier(repo_id, "reused-sha", "2024-08-31T23:59:59Z")
        .expect("later date must not move a snapshot"));

    database
        .set_metadata("history_sampling_version", "legacy")
        .expect("legacy history version");
    database.init().expect("history sampling migration");

    let migrated = database
        .snapshot_for_commit(repo_id, "reused-sha")
        .expect("snapshot after migration")
        .expect("snapshot survives history migration");
    assert_eq!(migrated.snapshot_date, "2024-06-30T23:59:59Z");
    assert_eq!(migrated.total_loc, 90);
    assert_eq!(migrated.source_loc, 70);
    assert_eq!(migrated.test_loc, 20);
    assert_eq!(
        database
            .repository(repo_id)
            .expect("repository after migration")
            .expect("repository survives migration")
            .local_path
            .as_deref(),
        Some("/managed/cache/history-sampling")
    );
    assert!(!database
        .repository(repo_id)
        .expect("repository flag lookup")
        .expect("repository")
        .loc_backfill_complete);
    assert_ne!(
        database
            .metadata("history_sampling_version")
            .expect("history version metadata")
            .as_deref(),
        Some("legacy")
    );
    remove_database(path);
}

#[test]
fn dashboard_cache_refreshes_after_commits_and_time_boundaries() {
    let (database, path) = temp_database("dashboard-cache");
    let id = database.upsert_repository(&repository("cache", "cache")).unwrap();
    let boundary = "2026-04-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    for (days, total) in [(90, 10), (30, 20), (7, 30), (1, 100), (-1, 200)] {
        let date = (boundary - Duration::days(days)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        database.upsert_snapshot(&Snapshot { repository_id: id, commit_sha: format!("sha-{days}"), commit_date: date.clone(), snapshot_date: date, total_loc: total, source_loc: total, ..Snapshot::default() }).unwrap();
    }
    let mut cache = DashboardCache::default();
    let before = database.dashboard_cached_at(&mut cache, boundary - Duration::seconds(1)).unwrap();
    assert_eq!(before.totals.total_loc, 100);
    assert_eq!(before.repositories[0].loc_change_7d, 80);
    assert_eq!(before.repositories[0].loc_change_30d, 90);
    assert!(!before.repositories[0].loc_baseline_90d_available);
    // Z sorts after the equivalent +00:00 cutoff in existing SQLite queries.
    let exact = database.dashboard_cached_at(&mut cache, boundary).unwrap();
    assert!(!exact.repositories[0].loc_baseline_90d_available);
    let now = boundary + Duration::seconds(1);
    let after = database.dashboard_cached_at(&mut cache, now).unwrap();
    assert_eq!(after.repositories[0].loc_change_7d, 70);
    assert_eq!(after.repositories[0].loc_change_30d, 80);
    assert_eq!(after.repositories[0].loc_change_90d, 90);
    assert!(Arc::ptr_eq(&after, &database.dashboard_cached_at(&mut cache, now).unwrap()));
    database.set_metadata("github_login", "changed-user").unwrap();
    database.set_metadata("org_discovery_errors", "retry owner").unwrap();
    database.apply_activity_page(&[PullRequest { repository_id: id, number: 1, state: "OPEN".into(), updated_at: "2026-04-01".into(), ..PullRequest::default() }], &[], id, 1, 0, "cursor", "partial").unwrap();
    let committed = database.dashboard_cached_at(&mut cache, now).unwrap();
    assert_eq!(committed.user.as_ref().unwrap().login, "changed-user");
    assert_eq!(committed.errors, vec!["retry owner"]);
    assert_eq!(committed.totals.open_prs, 1);
    database.mark_sync(id, None).unwrap();
    assert!(database.dashboard_cached_at(&mut cache, now).unwrap().last_sync_at.is_some());
    let now = boundary + Duration::days(1) + Duration::seconds(1);
    let future = database.dashboard_cached_at(&mut cache, now).unwrap();
    assert_eq!(future.totals.total_loc, 200);
    // Changes made outside Database must also invalidate the persistent observer.
    let external = Connection::open(&path).unwrap();
    external.execute("UPDATE repositories SET name='renamed' WHERE id=?1", [id]).unwrap();
    assert_eq!(database.dashboard_cached_at(&mut cache, now).unwrap().repositories[0].name, "renamed");
    database.set_classification_config(id, &ClassificationConfig::default()).unwrap();
    let invalidated = database.dashboard_cached_at(&mut cache, now).unwrap();
    assert_eq!(invalidated.totals.total_loc, 0);
    assert!(invalidated.history.is_empty());
    assert!(!invalidated.repositories[0].loc_available);
    external.execute("UPDATE repositories SET is_archived=1 WHERE id=?1", [id]).unwrap();
    assert!(database.dashboard_cached_at(&mut cache, now).unwrap().repositories.is_empty());
    // Reusing a cache for another database never carries records across files.
    let (other, other_path) = temp_database("dashboard-cache-other");
    assert!(other.dashboard_cached_at(&mut cache, now).unwrap().user.is_none());
    drop(cache);
    drop(external);
    remove_database(path);
    remove_database(other_path);
}
