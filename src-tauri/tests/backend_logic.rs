use chrono::{DateTime, Duration, Utc};
use codetally_lib::migrate_legacy_app_data;
use codetally_lib::classify::{
    classify_tokei_json, is_test_path_with_config, is_tokei_report, month_sample_dates,
    range_start,
};
use codetally_lib::db::Database;
use codetally_lib::gitops::{earliest_commit, scan_worktree_with_config};
use codetally_lib::models::{
    ActivityItem, AppSettings, ClassificationConfig, GithubIssueJson, GithubPullRequestJson,
    DashboardTotals, GithubRepositoryJson, Issue, MenuBarMetric, PullRequest, Repository,
    Snapshot, SyncProgress, ThemeMode,
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

fn snapshot_with_counts(repository_id: i64, sha: &str, date: &str, total: i64, source: i64, tests: i64) -> Snapshot {
    Snapshot {
        repository_id,
        commit_sha: sha.to_string(),
        commit_date: date.to_string(),
        snapshot_date: date.to_string(),
        total_loc: total,
        source_loc: source,
        test_loc: tests,
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
fn automatic_loc_cadence_runs_at_the_120_minute_boundary_and_manual_sync_forces_fetch() {
    let now: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().expect("cadence instant");
    let repository = completed_repository("cadence-boundary", "boundary", "2026-09-08T10:00:00Z", "2026-09-08T10:00:00Z");
    let before_boundary = decide_loc_sync(&repository, now, Some("2026-09-08T10:01:00Z"), false);
    assert!(!before_boundary.run);
    assert!(!before_boundary.force_fetch);

    let at_boundary = decide_loc_sync(&repository, now, Some("2026-09-08T10:00:00Z"), false);
    assert!(at_boundary.run);
    assert!(at_boundary.force_fetch);

    let manual = decide_loc_sync(&repository, now, Some("2026-09-08T11:59:00Z"), true);
    assert!(manual.run);
    assert!(manual.force_fetch);
}

#[test]
fn cadence_settings_have_expected_defaults_and_validate_supported_intervals() {
    let defaults = AppSettings::default();
    assert_eq!(defaults.theme_mode, ThemeMode::System);
    assert_eq!(defaults.activity_refresh_minutes, 30);
    assert_eq!(defaults.lines_refresh_minutes, 120);
    assert!(defaults.refresh_lines_on_change);
    assert!(defaults.run_in_background);
    assert_eq!(defaults.menu_bar_metric, MenuBarMetric::TotalLines);
    assert!(defaults.menu_bar_metrics.is_empty());
    assert!(defaults.show_menu_bar);
    assert_eq!(
        defaults.effective_menu_bar_metrics(),
        vec![MenuBarMetric::TotalLines]
    );
    assert!(!defaults.include_forks_in_totals);
    assert!(defaults.include_personal_repositories);
    assert!(defaults.include_company_repositories);
    assert!(defaults.excluded_repository_ids.is_empty());
    assert!(sync::validate_app_settings(&defaults).is_ok());

    let custom_activity = AppSettings {
        activity_refresh_minutes: 3,
        ..defaults.clone()
    };
    assert!(sync::validate_app_settings(&custom_activity).is_ok());
    let custom_lines = AppSettings {
        lines_refresh_minutes: 15,
        ..defaults.clone()
    };
    assert!(sync::validate_app_settings(&custom_lines).is_ok());

    for invalid_activity in [0, -1, 1_441] {
        let settings = AppSettings {
            activity_refresh_minutes: invalid_activity,
            ..defaults.clone()
        };
        assert!(
            sync::validate_app_settings(&settings).is_err(),
            "activity interval {invalid_activity} must be rejected"
        );
    }
    for invalid_lines in [0, -1, 1_441] {
        let settings = AppSettings {
            lines_refresh_minutes: invalid_lines,
            ..defaults.clone()
        };
        assert!(
            sync::validate_app_settings(&settings).is_err(),
            "lines interval {invalid_lines} must be rejected"
        );
    }

    for (mode, encoded) in [
        (ThemeMode::Light, "light"),
        (ThemeMode::Dark, "dark"),
        (ThemeMode::System, "system"),
    ] {
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(encoded));
        assert_eq!(serde_json::from_value::<ThemeMode>(json!(encoded)).unwrap(), mode);
    }
}

#[test]
fn cadence_settings_round_trip_through_persisted_app_metadata() {
    let (database, path) = temp_database("cadence-settings");
    let configured = AppSettings {
        theme_mode: ThemeMode::System,
        activity_refresh_minutes: 5,
        lines_refresh_minutes: 60,
        refresh_lines_on_change: false,
        run_in_background: false,
        menu_bar_metric: MenuBarMetric::OpenPrs,
        menu_bar_metrics: Vec::new(),
        show_menu_bar: true,
        include_forks_in_totals: true,
        include_personal_repositories: false,
        include_company_repositories: true,
        excluded_repository_ids: vec!["repo-excluded".into()],
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
    assert!(!loaded.run_in_background);
    assert_eq!(loaded.menu_bar_metric, MenuBarMetric::OpenPrs);
    assert_eq!(loaded.effective_menu_bar_metrics(), vec![MenuBarMetric::OpenPrs]);
    assert!(loaded.include_forks_in_totals);
    assert!(!loaded.include_personal_repositories);
    assert!(loaded.include_company_repositories);
    assert_eq!(loaded.excluded_repository_ids, vec!["repo-excluded"]);
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
fn custom_cadence_minute_boundaries_persist_and_invalid_values_are_rejected() {
    let (database, path) = temp_database("custom-cadence-boundaries");
    let lower = AppSettings {
        activity_refresh_minutes: 1,
        lines_refresh_minutes: 1,
        ..AppSettings::default()
    };
    sync::save_app_settings(&database, &lower).expect("minimum intervals should persist");
    let loaded_lower = sync::app_settings(&database).expect("load minimum intervals");
    assert_eq!(loaded_lower.activity_refresh_minutes, 1);
    assert_eq!(loaded_lower.lines_refresh_minutes, 1);

    let upper = AppSettings {
        activity_refresh_minutes: 1_440,
        lines_refresh_minutes: 1_440,
        ..AppSettings::default()
    };
    sync::save_app_settings(&database, &upper).expect("maximum intervals should persist");
    let loaded_upper = sync::app_settings(&database).expect("load maximum intervals");
    assert_eq!(loaded_upper.activity_refresh_minutes, 1_440);
    assert_eq!(loaded_upper.lines_refresh_minutes, 1_440);

    for invalid_activity in [0, -1, 1_441] {
        let settings = AppSettings {
            activity_refresh_minutes: invalid_activity,
            ..AppSettings::default()
        };
        assert!(
            sync::save_app_settings(&database, &settings).is_err(),
            "activity interval {invalid_activity} must not persist"
        );
    }
    for invalid_lines in [0, -1, 1_441] {
        let settings = AppSettings {
            lines_refresh_minutes: invalid_lines,
            ..AppSettings::default()
        };
        assert!(
            sync::save_app_settings(&database, &settings).is_err(),
            "lines interval {invalid_lines} must not persist"
        );
    }
    let after_rejected_values = sync::app_settings(&database).expect("load settings after rejected values");
    assert_eq!(after_rejected_values.activity_refresh_minutes, 1_440);
    assert_eq!(after_rejected_values.lines_refresh_minutes, 1_440);
    remove_database(path);
}

#[test]
fn legacy_saved_app_settings_keep_cadence_and_receive_new_defaults() {
    let (database, path) = temp_database("legacy-settings");
    database
        .set_metadata(
            sync::APP_SETTINGS_METADATA_KEY,
            r#"{"activity_refresh_minutes":5,"lines_refresh_minutes":60,"refresh_lines_on_change":false,"menu_bar_metric":"open_prs"}"#,
        )
        .expect("save legacy settings JSON");

    let loaded = sync::app_settings(&database).expect("load legacy settings");
    assert_eq!(loaded.activity_refresh_minutes, 5);
    assert_eq!(loaded.lines_refresh_minutes, 60);
    assert!(!loaded.refresh_lines_on_change);
    assert!(loaded.run_in_background);
    assert_eq!(loaded.theme_mode, ThemeMode::System);
    assert_eq!(loaded.menu_bar_metric, MenuBarMetric::OpenPrs);
    assert!(loaded.menu_bar_metrics.is_empty());
    assert!(loaded.show_menu_bar);
    assert_eq!(loaded.effective_menu_bar_metrics(), vec![MenuBarMetric::OpenPrs]);
    assert!(!loaded.include_forks_in_totals);
    assert!(loaded.include_personal_repositories);
    assert!(loaded.include_company_repositories);
    assert!(loaded.excluded_repository_ids.is_empty());
    remove_database(path);
}

#[test]
fn previous_refresh_defaults_migrate_once_without_overriding_later_custom_values() {
    let (database, path) = temp_database("refresh-default-migration");
    database
        .set_metadata(
            sync::APP_SETTINGS_METADATA_KEY,
            r#"{"activity_refresh_minutes":10,"lines_refresh_minutes":45,"refresh_lines_on_change":true}"#,
        )
        .expect("save previous defaults");

    let migrated = sync::app_settings(&database).expect("migrate previous defaults");
    assert_eq!(migrated.activity_refresh_minutes, 30);
    assert_eq!(migrated.lines_refresh_minutes, 120);

    let custom = AppSettings {
        activity_refresh_minutes: 10,
        lines_refresh_minutes: 45,
        ..migrated
    };
    sync::save_app_settings(&database, &custom).expect("save later custom cadence");
    let reloaded = sync::app_settings(&database).expect("reload later custom cadence");
    assert_eq!(reloaded.activity_refresh_minutes, 10);
    assert_eq!(reloaded.lines_refresh_minutes, 45);
    remove_database(path);
}

#[test]
fn all_menu_bar_metrics_round_trip_through_saved_app_settings() {
    let (database, path) = temp_database("menu-bar-metrics");
    for metric in [
        MenuBarMetric::TotalLines,
        MenuBarMetric::TestLines,
        MenuBarMetric::SourceLines,
        MenuBarMetric::OpenPrs,
        MenuBarMetric::OpenIssues,
    ] {
        let settings = AppSettings {
            menu_bar_metric: metric,
            ..AppSettings::default()
        };
        sync::save_app_settings(&database, &settings).expect("save menu bar metric");
        let loaded = sync::app_settings(&database).expect("load menu bar metric");
        assert_eq!(loaded.menu_bar_metric, metric);
        assert_eq!(loaded.menu_bar_metrics, vec![metric]);
    }
    remove_database(path);
}

#[test]
fn multiple_menu_bar_metrics_round_trip_in_canonical_order_without_duplicates() {
    let (database, path) = temp_database("multiple-menu-bar-metrics");
    let settings = AppSettings {
        menu_bar_metric: MenuBarMetric::OpenPrs,
        menu_bar_metrics: vec![
            MenuBarMetric::OpenIssues,
            MenuBarMetric::TotalLines,
            MenuBarMetric::OpenIssues,
            MenuBarMetric::SourceLines,
        ],
        ..AppSettings::default()
    };
    sync::save_app_settings(&database, &settings).expect("save multiple menu bar metrics");
    let loaded = sync::app_settings(&database).expect("load multiple menu bar metrics");
    assert_eq!(
        loaded.menu_bar_metrics,
        vec![
            MenuBarMetric::TotalLines,
            MenuBarMetric::SourceLines,
            MenuBarMetric::OpenIssues,
        ]
    );
    assert_eq!(loaded.menu_bar_metric, MenuBarMetric::TotalLines);
    assert_eq!(
        loaded.effective_menu_bar_metrics(),
        loaded.menu_bar_metrics.clone()
    );
    remove_database(path);
}

#[test]
fn menu_bar_metric_titles_select_the_matching_dashboard_total() {
    let totals = DashboardTotals {
        total_loc: 12_345,
        source_loc: 8_765,
        test_loc: 3_580,
        open_prs: 23,
        open_issues: 41,
        ..DashboardTotals::default()
    };

    assert_eq!(codetally_lib::native::metric_title(MenuBarMetric::TotalLines, &totals), "12.3K lines");
    assert_eq!(codetally_lib::native::metric_title(MenuBarMetric::TestLines, &totals), "3.6K tests");
    assert_eq!(codetally_lib::native::metric_title(MenuBarMetric::SourceLines, &totals), "8.8K source");
    assert_eq!(codetally_lib::native::metric_title(MenuBarMetric::OpenPrs, &totals), "23 PRs");
    assert_eq!(codetally_lib::native::metric_title(MenuBarMetric::OpenIssues, &totals), "41 issues");
}

#[test]
fn menu_bar_titles_are_split_so_each_native_item_can_use_an_aligned_icon() {
    let totals = DashboardTotals {
        total_loc: 12_345,
        source_loc: 8_765,
        test_loc: 3_580,
        open_prs: 23,
        open_issues: 41,
        ..DashboardTotals::default()
    };

    assert_eq!(
        codetally_lib::native::menu_titles(
            &[
                MenuBarMetric::TotalLines,
                MenuBarMetric::SourceLines,
                MenuBarMetric::TestLines,
                MenuBarMetric::OpenPrs,
                MenuBarMetric::OpenIssues,
            ],
            &totals,
        ),
        vec!["12.3K lines", "8.8K source", "3.6K tests", "23 PRs", "41 issues"]
    );
    assert_eq!(
        codetally_lib::native::menu_titles(
            &[MenuBarMetric::OpenPrs, MenuBarMetric::TotalLines],
            &totals,
        ),
        vec!["23 PRs", "12.3K lines"]
    );
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
repo_json='[{"id":"owned-id","name":"portfolio","nameWithOwner":"me/portfolio","url":"https://github.com/me/portfolio","sshUrl":"git@github.com:me/portfolio.git","isPrivate":false,"isFork":false,"isArchived":false,"defaultBranchRef":{"name":"main"},"primaryLanguage":{"name":"Rust"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","pushedAt":"2026-09-02T00:00:00Z"}]'
org_json='[{"id":"org-id","name":"shared","nameWithOwner":"org-one/shared","url":"https://github.com/org-one/shared","sshUrl":"git@github.com:org-one/shared.git","isPrivate":true,"isFork":false,"isArchived":false,"defaultBranchRef":{"name":"main"},"primaryLanguage":{"name":"TypeScript"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","pushedAt":"2026-09-02T00:00:00Z"},{"id":"owned-id","name":"duplicate","nameWithOwner":"org-one/duplicate","url":"https://github.com/org-one/duplicate","sshUrl":"git@github.com:org-one/duplicate.git","isPrivate":false,"isFork":false,"isArchived":false,"defaultBranchRef":{"name":"main"},"primaryLanguage":{"name":"Rust"},"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","pushedAt":"2026-09-02T00:00:00Z"}]'
if [ "$1" = "api" ] && [ "$2" = "user" ]; then printf 'me\n'; exit 0; fi
if [ "$1" = "api" ] && [ "$2" = "graphql" ]; then printf '{"data":{"rateLimit":{"remaining":5000,"resetAt":"2099-01-01T00:00:00Z"}}}\n'; exit 0; fi
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
        job_lock: Arc::new(Mutex::new(())),
    };

    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(&original_path));
    std::env::set_var("PATH", std::env::join_paths(paths).expect("PATH").to_string_lossy().to_string());
    let discovered = sync::discover(&state);
    std::env::set_var("PATH", original_path);

    let repositories = discovered.expect("discovery should retain usable owners");
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
fn repository_selection_keeps_all_nonarchived_inventory_for_reselection() {
    let (database, path) = temp_database("repository-selection");
    database
        .set_metadata("github_login", "sam")
        .expect("current GitHub login");
    let personal_id = database
        .upsert_repository(&Repository {
            github_id: "personal-id".into(),
            owner: "sam".into(),
            name: "alpha".into(),
            name_with_owner: "sam/alpha".into(),
            ..repository("personal-id", "alpha")
        })
        .expect("personal repository");
    let company_id = database
        .upsert_repository(&Repository {
            github_id: "company-id".into(),
            owner: "acme".into(),
            name: "company".into(),
            name_with_owner: "acme/company".into(),
            ..repository("company-id", "company")
        })
        .expect("company repository");
    let excluded_id = database
        .upsert_repository(&Repository {
            github_id: "excluded-id".into(),
            owner: "sam".into(),
            name: "disabled".into(),
            name_with_owner: "sam/disabled".into(),
            ..repository("excluded-id", "disabled")
        })
        .expect("excluded repository");
    database
        .upsert_repository(&Repository {
            github_id: "archived-id".into(),
            owner: "sam".into(),
            name: "archived".into(),
            name_with_owner: "sam/archived".into(),
            is_archived: true,
            ..repository("archived-id", "archived")
        })
        .expect("archived repository");

    sync::save_app_settings(
        &database,
        &AppSettings {
            include_personal_repositories: false,
            include_company_repositories: false,
            excluded_repository_ids: vec!["excluded-id".into()],
            ..AppSettings::default()
        },
    )
    .expect("save repository selection");

    let inventory = database
        .repository_selection()
        .expect("repository inventory");
    assert_eq!(
        inventory
            .iter()
            .map(|item| (item.github_id.as_str(), item.name_with_owner.as_str(), item.owner.as_str(), item.group.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("company-id", "acme/company", "acme", "company"),
            ("personal-id", "sam/alpha", "sam", "personal"),
            ("excluded-id", "sam/disabled", "sam", "personal"),
        ]
    );
    assert!(inventory.iter().any(|item| item.github_id == "excluded-id"));
    assert!(inventory.iter().all(|item| item.github_id != "archived-id"));

    let selected = database
        .selected_repositories()
        .expect("selected repositories");
    assert!(selected.is_empty(), "both groups are disabled");
    let personal = database
        .repository(personal_id)
        .expect("personal lookup")
        .expect("personal row");
    let company = database
        .repository(company_id)
        .expect("company lookup")
        .expect("company row");
    let excluded = database
        .repository(excluded_id)
        .expect("excluded lookup")
        .expect("excluded row");
    assert!(!database.repository_enabled(&personal).expect("personal enabled state"));
    assert!(!database.repository_enabled(&company).expect("company enabled state"));
    assert!(!database.repository_enabled(&excluded).expect("excluded enabled state"));
    remove_database(path);
}

#[test]
fn dashboard_history_and_activity_filter_disabled_repositories_before_limits() {
    let (database, path) = temp_database("repository-filtering");
    database
        .set_metadata("github_login", "sam")
        .expect("current GitHub login");
    let selected_id = database
        .upsert_repository(&Repository {
            github_id: "selected-id".into(),
            owner: "sam".into(),
            name: "selected".into(),
            name_with_owner: "sam/selected".into(),
            ..repository("selected-id", "selected")
        })
        .expect("selected repository");
    let disabled_id = database
        .upsert_repository(&Repository {
            github_id: "disabled-id".into(),
            owner: "sam".into(),
            name: "disabled".into(),
            name_with_owner: "sam/disabled".into(),
            ..repository("disabled-id", "disabled")
        })
        .expect("disabled repository");
    sync::save_app_settings(
        &database,
        &AppSettings {
            excluded_repository_ids: vec!["disabled-id".into()],
            ..AppSettings::default()
        },
    )
    .expect("save disabled repository");

    database
        .upsert_snapshot(&snapshot(selected_id, "selected-sha", "2026-09-01T00:00:00Z", 100))
        .expect("selected snapshot");
    database
        .upsert_snapshot(&snapshot(disabled_id, "disabled-sha", "2026-09-02T00:00:00Z", 900))
        .expect("disabled snapshot");
    database
        .upsert_pull_request(&PullRequest {
            repository_id: selected_id,
            repository: "sam/selected".into(),
            number: 1,
            title: "Selected PR".into(),
            state: "OPEN".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            ..PullRequest::default()
        })
        .expect("selected pull request");
    database
        .upsert_pull_request(&PullRequest {
            repository_id: disabled_id,
            repository: "sam/disabled".into(),
            number: 2,
            title: "Disabled PR".into(),
            state: "OPEN".into(),
            created_at: "2026-09-02T00:00:00Z".into(),
            updated_at: "2026-09-02T00:00:00Z".into(),
            ..PullRequest::default()
        })
        .expect("disabled pull request");
    database
        .upsert_issue(&Issue {
            repository_id: selected_id,
            repository: "sam/selected".into(),
            number: 3,
            title: "Selected issue".into(),
            state: "OPEN".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            ..Issue::default()
        })
        .expect("selected issue");
    database
        .upsert_issue(&Issue {
            repository_id: disabled_id,
            repository: "sam/disabled".into(),
            number: 4,
            title: "Disabled issue".into(),
            state: "OPEN".into(),
            created_at: "2026-09-02T00:00:00Z".into(),
            updated_at: "2026-09-02T00:00:00Z".into(),
            ..Issue::default()
        })
        .expect("disabled issue");

    let summaries = database.summaries().expect("filtered summaries");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].github_id, "selected-id");
    let totals = database.totals(&summaries).expect("filtered totals");
    assert_eq!(totals.repositories, 1);
    assert_eq!(totals.total_loc, 100);
    assert_eq!(database.history(None).expect("filtered history").len(), 1);

    let prs = database
        .pull_requests(None, None, 1)
        .expect("limited pull request feed");
    assert!(matches!(&prs[..], [item] if item.title == "Selected PR"));
    let issues = database
        .issues(None, None, 1)
        .expect("limited issue feed");
    assert!(matches!(&issues[..], [item] if item.title == "Selected issue"));
    assert!(database
        .pull_requests(Some(disabled_id), None, 100)
        .expect("disabled pull request feed")
        .is_empty());
    assert!(database
        .issues(Some(disabled_id), None, 100)
        .expect("disabled issue feed")
        .is_empty());
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
fn activity_group_scopes_apply_before_limit_and_empty_scope_returns_nothing() {
    let (database, path) = temp_database("activity-group-scopes");
    let personal = database.upsert_repository(&repository("personal", "personal")).unwrap();
    let company = database.upsert_repository(&repository("company", "company")).unwrap();
    for (id, date) in [(personal, "2026-09-01T00:00:00Z"), (company, "2026-09-02T00:00:00Z")] {
        database.upsert_pull_request(&PullRequest { repository_id: id, number: 1, title: "Open PR".into(), state: "OPEN".into(), updated_at: date.into(), ..PullRequest::default() }).unwrap();
        database.upsert_issue(&Issue { repository_id: id, number: 2, title: "Open issue".into(), state: "OPEN".into(), updated_at: date.into(), ..Issue::default() }).unwrap();
    }
    for kind in ["prs", "issues"] {
        let scoped = database.scoped_activity(kind, None, Some(&[personal]), Some("open"), 1).unwrap();
        assert_eq!(scoped.len(), 1);
        let id = match &scoped[0] { codetally_lib::models::ActivityItem::PullRequest(item) => item.repository_id, codetally_lib::models::ActivityItem::Issue(item) => item.repository_id };
        assert_eq!(id, personal);
        assert!(database.scoped_activity(kind, None, Some(&[]), Some("open"), 100).unwrap().is_empty());
        assert!(database.scoped_activity(kind, Some(company), Some(&[personal]), Some("open"), 100).unwrap().is_empty());
        assert_eq!(database.scoped_activity(kind, None, Some(&[personal, company]), Some("open"), 100).unwrap().len(), 2);
    }
    sync::save_app_settings(&database, &AppSettings { excluded_repository_ids: vec!["company".into()], ..AppSettings::default() }).unwrap();
    assert!(database.scoped_activity("prs", None, Some(&[company]), Some("open"), 100).unwrap().is_empty());
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
    let totals = database.totals(&summaries).expect("summaries totals");
    assert_eq!(totals.repositories, 2);
    assert_eq!(totals.total_loc, 125);
    remove_database(path);
}

#[test]
fn fork_line_totals_and_history_follow_opt_in_setting_and_exclusions() {
    let (database, path) = temp_database("fork-line-totals");
    let active_id = database
        .upsert_repository(&repository("active", "active"))
        .expect("active repository");
    let included_fork_id = database
        .upsert_repository(&Repository {
            github_id: "included-fork".into(),
            name: "included-fork".into(),
            name_with_owner: "owner/included-fork".into(),
            is_fork: true,
            ..repository("included-fork", "included-fork")
        })
        .expect("included fork");
    let excluded_fork_id = database
        .upsert_repository(&Repository {
            github_id: "excluded-fork".into(),
            name: "excluded-fork".into(),
            name_with_owner: "owner/excluded-fork".into(),
            is_fork: true,
            ..repository("excluded-fork", "excluded-fork")
        })
        .expect("excluded fork");

    let current_date = (Utc::now() - Duration::days(1)).to_rfc3339();
    let baseline_date = (Utc::now() - Duration::days(31)).to_rfc3339();
    database
        .upsert_snapshot(&snapshot_with_counts(active_id, "active-baseline", &baseline_date, 100, 70, 30))
        .expect("active baseline");
    database
        .upsert_snapshot(&snapshot_with_counts(active_id, "active-current", &current_date, 140, 100, 40))
        .expect("active current");
    database
        .upsert_snapshot(&snapshot_with_counts(included_fork_id, "included-baseline", &baseline_date, 500, 300, 200))
        .expect("included fork baseline");
    database
        .upsert_snapshot(&snapshot_with_counts(included_fork_id, "included-current", &current_date, 900, 600, 300))
        .expect("included fork current");
    database
        .upsert_snapshot(&snapshot_with_counts(excluded_fork_id, "excluded-baseline", &baseline_date, 700, 400, 300))
        .expect("excluded fork baseline");
    database
        .upsert_snapshot(&snapshot_with_counts(excluded_fork_id, "excluded-current", &current_date, 1_200, 800, 400))
        .expect("excluded fork current");

    let default_summaries = database.summaries().expect("default summaries");
    let default_totals = database.totals(&default_summaries).expect("default totals");
    assert_eq!(default_totals.repositories, 3);
    assert_eq!(default_totals.total_loc, 140);
    assert_eq!(default_totals.source_loc, 100);
    assert_eq!(default_totals.test_loc, 40);
    assert_eq!(default_totals.loc_change_30d, 40);
    assert_eq!(
        database
            .history(None)
            .expect("default history")
            .iter()
            .map(|point| point.total_loc)
            .collect::<Vec<_>>(),
        vec![100, 140]
    );

    sync::save_app_settings(
        &database,
        &AppSettings {
            include_forks_in_totals: true,
            excluded_repository_ids: vec!["excluded-fork".into()],
            ..AppSettings::default()
        },
    )
    .expect("save fork total settings");

    let opted_in_summaries = database.summaries().expect("opted-in summaries");
    assert_eq!(opted_in_summaries.len(), 2);
    assert!(opted_in_summaries.iter().any(|summary| summary.id == included_fork_id));
    assert!(opted_in_summaries.iter().all(|summary| summary.id != excluded_fork_id));
    let opted_in_totals = database.totals(&opted_in_summaries).expect("opted-in totals");
    assert_eq!(opted_in_totals.repositories, 2);
    assert_eq!(opted_in_totals.total_loc, 1_040);
    assert_eq!(opted_in_totals.source_loc, 700);
    assert_eq!(opted_in_totals.test_loc, 340);
    assert_eq!(opted_in_totals.loc_change_30d, 440);
    assert_eq!(
        database
            .history(None)
            .expect("opted-in history")
            .iter()
            .map(|point| point.total_loc)
            .collect::<Vec<_>>(),
        vec![600, 1_040]
    );
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
