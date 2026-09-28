use codetally_lib::db::Database;
use codetally_lib::kanban::{self, KanbanQuery};
use codetally_lib::models::{AppSettings, Issue, KanbanMetadata, PullRequest, Repository};
use codetally_lib::sync;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_database(label: &str) -> (Database, PathBuf) {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "codetally-kanban-{label}-{}-{suffix}.sqlite3",
        std::process::id()
    ));
    let database = Database::new(&path);
    database.init().expect("initialize database");
    (database, path)
}

fn remove_database(path: PathBuf) {
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

fn repository(github_id: &str, owner: &str, name: &str) -> Repository {
    Repository {
        github_id: github_id.into(),
        owner: owner.into(),
        name: name.into(),
        name_with_owner: format!("{owner}/{name}"),
        url: format!("https://github.com/{owner}/{name}"),
        ssh_url: format!("git@github.com:{owner}/{name}.git"),
        default_branch: "main".into(),
        ..Repository::default()
    }
}

fn board_query(kind: &str, show_completed: bool, offset: i64, limit: i64) -> KanbanQuery {
    KanbanQuery {
        kind: kind.into(),
        repository_ids: None,
        repository_id: None,
        relationship: "everyone".into(),
        search: String::new(),
        show_completed,
        offset,
        limit,
    }
}

fn enable_board(database: &Database, login: &str) {
    database
        .set_metadata("github_login", login)
        .expect("set account");
    sync::save_app_settings(
        database,
        &AppSettings {
            kanban_enabled: true,
            ..AppSettings::default()
        },
    )
    .expect("enable board");
}

fn add_pull_request(database: &Database, repository_id: i64, number: i64, state: &str) {
    database
        .upsert_pull_request(&PullRequest {
            repository_id,
            number,
            title: format!("PR {number}"),
            state: state.into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            closed_at: (state != "OPEN").then(|| "2026-09-01T00:00:00Z".into()),
            url: format!("https://github.com/example/repo/pull/{number}"),
            author: Some("alice".into()),
            ..PullRequest::default()
        })
        .expect("store PR");
}

#[test]
fn kanban_defaults_on_but_explicit_disable_survives_legacy_cache_upgrade() {
    let (database, path) = temporary_database("legacy-upgrade");
    assert!(
        sync::app_settings(&database)
            .expect("default settings")
            .kanban_enabled
    );
    sync::save_app_settings(&database, &AppSettings { kanban_enabled: false, ..AppSettings::default() }).expect("disable board");
    assert!(kanban::page(&database, board_query("prs", false, 0, 50))
        .expect_err("board reads are disabled by user setting")
        .to_string()
        .contains("Enable Kanban"));

    database
        .set_metadata("github_login", "alice")
        .expect("seed account");
    let repo_id = database
        .upsert_repository(&repository("legacy-repo", "alice", "legacy"))
        .expect("seed repository");
    add_pull_request(&database, repo_id, 13, "OPEN");
    database
        .upsert_issue(&Issue {
            repository_id: repo_id,
            number: 21,
            title: "Legacy issue".into(),
            state: "OPEN".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            url: "https://github.com/alice/legacy/issues/21".into(),
            author: Some("alice".into()),
            ..Issue::default()
        })
        .expect("seed issue");

    // Recreate a pre-Kanban schema by removing only the additive board
    // columns/tables and lowering user_version. Reopening must add them back
    // without discarding the GitHub cache already in use.
    let conn = Connection::open(&path).expect("legacy database connection");
    conn.execute_batch(
        "DROP TABLE kanban_preferences;
         DROP TABLE kanban_links_cache;
         DROP TABLE kanban_activity_status;
         DROP TABLE kanban_account_repositories;
         DROP TABLE kanban_item_metadata;
         ALTER TABLE pull_requests DROP COLUMN node_id;
         ALTER TABLE issues DROP COLUMN node_id;
         ALTER TABLE issues DROP COLUMN completion_reason;
         PRAGMA user_version = 0;",
    )
    .expect("prepare legacy schema");
    drop(conn);

    let migrated = Database::new(&path);
    migrated.init().expect("upgrade legacy schema");
    assert_eq!(
        migrated
            .pull_requests(Some(repo_id), None, 20)
            .expect("PR cache")
            .len(),
        1
    );
    assert_eq!(
        migrated
            .issues(Some(repo_id), None, 20)
            .expect("issue cache")
            .len(),
        1
    );
    assert_eq!(
        kanban::account_repository_ids(&migrated).expect("migrated repository scope"),
        ["legacy-repo".to_string()].into_iter().collect()
    );
    assert!(
        !sync::app_settings(&migrated)
            .expect("default settings after migration")
            .kanban_enabled
    );

    remove_database(path);
}

#[test]
fn local_metadata_survives_node_promotion_restart_and_remains_account_scoped() {
    let (database, path) = temporary_database("metadata");
    enable_board(&database, "alice");
    let repo_id = database
        .upsert_repository(&repository("repo-one", "alice", "one"))
        .expect("seed repository");
    add_pull_request(&database, repo_id, 17, "OPEN");
    database
        .upsert_issue(&Issue {
            repository_id: repo_id,
            number: 18,
            title: "Shipped enhancement".into(),
            state: "CLOSED".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            updated_at: "2026-09-02T00:00:00Z".into(),
            closed_at: Some("2026-09-02T00:00:00Z".into()),
            url: "https://github.com/alice/one/issues/18".into(),
            author: Some("alice".into()),
            ..Issue::default()
        })
        .expect("seed issue");
    kanban::remember_repository(&database, "repo-one").expect("remember repository");
    kanban::record_activity(&database, "repo-one", true, None).expect("record refresh");
    kanban::store_identity(
        &database,
        repo_id,
        "repo-one",
        "issue",
        18,
        Some("Issue_node_18"),
        Some("COMPLETED"),
    )
    .expect("promote issue to stable node identity");

    let fallback_key = kanban::item_key("repo-one", "pr", 17, None);
    let initial = kanban::save_metadata(
        &database,
        KanbanMetadata {
            item_key: fallback_key.clone(),
            manual_column: Some("In progress".into()),
            priority: "High".into(),
            notes: "Keep the local review context".into(),
            sort_rank: 45,
            ..KanbanMetadata::default()
        },
        0,
    )
    .expect("create local metadata");
    assert_eq!(initial.revision, 1);
    assert!(
        kanban::save_metadata(
            &database,
            KanbanMetadata {
                item_key: fallback_key.clone(),
                priority: "Low".into(),
                ..KanbanMetadata::default()
            },
            0,
        )
        .is_err(),
        "an outdated create must not overwrite a saved item"
    );

    kanban::store_identity(
        &database,
        repo_id,
        "repo-one",
        "pr",
        17,
        Some("PR_node_17"),
        None,
    )
    .expect("promote PR to stable node identity");
    let stable_key = kanban::item_key("repo-one", "pr", 17, Some("PR_node_17"));
    let changed = kanban::save_metadata(
        &database,
        KanbanMetadata {
            item_key: stable_key.clone(),
            manual_column: Some("Review".into()),
            priority: "Medium".into(),
            notes: "Metadata follows the stable GitHub identity".into(),
            sort_rank: 12,
            ..KanbanMetadata::default()
        },
        1,
    )
    .expect("update with current revision");
    assert_eq!(changed.revision, 2);
    assert!(
        kanban::save_metadata(
            &database,
            KanbanMetadata {
                item_key: stable_key.clone(),
                priority: "Low".into(),
                ..KanbanMetadata::default()
            },
            1,
        )
        .is_err(),
        "stale compare-and-swap writes must fail"
    );

    let page = kanban::page(&database, board_query("both", true, 0, 50)).expect("board page");
    assert_eq!(
        (page.total, page.active_count, page.completed_count),
        (2, 1, 1)
    );
    assert!(
        !page.partial,
        "a successful activity refresh should complete the page status"
    );
    let pr = page
        .items
        .iter()
        .find(|item| item.kind == "pr")
        .expect("PR row");
    assert_eq!(pr.item_key, stable_key);
    assert_eq!(pr.manual_column.as_deref(), Some("Review"));
    assert_eq!(pr.priority, "Medium");
    assert_eq!(pr.notes, "Metadata follows the stable GitHub identity");
    assert_eq!(pr.sort_rank, 12);
    assert_eq!(pr.revision, 2);
    let issue = page
        .items
        .iter()
        .find(|item| item.kind == "issue")
        .expect("issue row");
    assert_eq!(
        issue.item_key,
        kanban::item_key("repo-one", "issue", 18, Some("Issue_node_18"))
    );
    assert_eq!(issue.completion_reason.as_deref(), Some("COMPLETED"));

    let reopened = Database::new(&path);
    reopened.init().expect("reopen database");
    let persisted =
        kanban::page(&reopened, board_query("prs", false, 0, 50)).expect("persisted board page");
    assert_eq!(persisted.items[0].item_key, stable_key);
    assert_eq!(
        persisted.items[0].notes,
        "Metadata follows the stable GitHub identity"
    );
    assert_eq!(persisted.items[0].revision, 2);
    kanban::record_activity(&reopened, "repo-one", false, Some("Repository unavailable"))
        .expect("mark inaccessible repository");
    let unavailable =
        kanban::page(&reopened, board_query("prs", false, 0, 50)).expect("cached unavailable work");
    assert!(unavailable.partial);
    assert!(unavailable.items[0].unavailable);
    assert_eq!(
        unavailable.active_count, 1,
        "an unavailable open item is not closed"
    );

    reopened
        .set_metadata("github_login", "bob")
        .expect("switch account");
    kanban::remember_repository(&reopened, "repo-one").expect("remember repository for Bob");
    let bob_page =
        kanban::page(&reopened, board_query("prs", false, 0, 50)).expect("Bob board page");
    assert_eq!(
        bob_page.items[0].priority, "None",
        "metadata from Alice's account must not leak"
    );
    assert_eq!(bob_page.items[0].notes, "");
    kanban::save_metadata(
        &reopened,
        KanbanMetadata {
            item_key: stable_key.clone(),
            manual_column: Some("Blocked".into()),
            priority: "Low".into(),
            notes: "Bob's local note".into(),
            sort_rank: 8,
            ..KanbanMetadata::default()
        },
        0,
    )
    .expect("save Bob's independent metadata");
    reopened
        .set_metadata("github_login", "alice")
        .expect("switch back to Alice");
    let alice_page =
        kanban::page(&reopened, board_query("prs", false, 0, 50)).expect("Alice board page");
    assert_eq!(
        alice_page.items[0].notes,
        "Metadata follows the stable GitHub identity"
    );

    remove_database(path);
}

#[test]
fn board_counts_selection_and_pagination_work_across_one_hundred_repositories() {
    const REPOSITORIES: usize = 100;
    const OPEN_PER_REPOSITORY: usize = 11;
    let (database, path) = temporary_database("many-repositories");
    enable_board(&database, "alice");

    let mut repository_ids = Vec::with_capacity(REPOSITORIES);
    for index in 0..REPOSITORIES {
        let repo_id = database
            .upsert_repository(&repository(
                &format!("repo-{index:03}"),
                "alice",
                &format!("project-{index:03}"),
            ))
            .expect("seed repository");
        kanban::remember_repository(&database, &format!("repo-{index:03}"))
            .expect("remember repository");
        repository_ids.push(repo_id);
    }

    let mut conn = Connection::open(&path).expect("database connection");
    let tx = conn.transaction().expect("seed activity transaction");
    for (repo_index, repo_id) in repository_ids.iter().enumerate() {
        for number in 1..=OPEN_PER_REPOSITORY {
            tx.execute(
                "INSERT INTO pull_requests(repository_id,number,title,state,is_draft,created_at,updated_at,closed_at,url,author,assignees_json,node_id) VALUES (?1,?2,?3,'OPEN',0,?4,?4,NULL,?5,'alice','[]',NULL)",
                params![
                    repo_id,
                    number as i64,
                    format!("Open {number} in repository {repo_index}"),
                    "2026-09-01T00:00:00Z",
                    format!("https://github.com/alice/project-{repo_index:03}/pull/{number}")
                ],
            )
            .expect("insert open PR");
        }
        tx.execute(
            "INSERT INTO pull_requests(repository_id,number,title,state,is_draft,created_at,updated_at,closed_at,url,author,assignees_json,node_id) VALUES (?1,99,?2,'CLOSED',0,?3,?3,?3,?4,'alice','[]',NULL)",
            params![
                repo_id,
                format!("Closed in repository {repo_index}"),
                "2026-09-02T00:00:00Z",
                format!("https://github.com/alice/project-{repo_index:03}/pull/99")
            ],
        )
        .expect("insert closed PR");
    }
    tx.commit().expect("commit activity fixtures");
    drop(conn);

    let first_page =
        kanban::page(&database, board_query("prs", false, 0, 200)).expect("first page");
    assert_eq!(first_page.total, 1_100);
    assert_eq!(first_page.active_count, 1_100);
    assert_eq!(first_page.completed_count, 100);
    assert_eq!(first_page.items.len(), 200);
    let none = kanban::page(&database, board_query("none", true, 0, 200)).expect("no ticket types selected");
    assert_eq!((none.active_count, none.completed_count, none.total), (0, 0, 0));
    assert!(none.items.is_empty());

    let late_page = kanban::page(&database, board_query("prs", false, 1_000, 200))
        .expect("page after one thousand");
    assert_eq!(late_page.items.len(), 100);
    assert!(late_page.items.iter().all(|item| item.state == "OPEN"));
    let early_keys: std::collections::BTreeSet<_> = first_page
        .items
        .iter()
        .map(|item| item.item_key.as_str())
        .collect();
    assert!(
        late_page
            .items
            .iter()
            .all(|item| !early_keys.contains(item.item_key.as_str())),
        "pages must not repeat items"
    );

    let completed_tail = kanban::page(&database, board_query("prs", true, 1_100, 200))
        .expect("completed items page");
    assert_eq!(completed_tail.total, 1_200);
    assert_eq!(completed_tail.items.len(), 100);
    assert!(completed_tail
        .items
        .iter()
        .all(|item| item.state == "CLOSED"));

    let selected: Vec<i64> = repository_ids.into_iter().take(30).collect();
    let selected_page = kanban::page(
        &database,
        KanbanQuery {
            repository_ids: Some(selected),
            ..board_query("prs", true, 0, 200)
        },
    )
    .expect("selected-repository page");
    assert_eq!(selected_page.total, 360);
    assert_eq!(selected_page.active_count, 330);
    assert_eq!(selected_page.completed_count, 30);
    assert_eq!(selected_page.items.len(), 200);

    remove_database(path);
}
