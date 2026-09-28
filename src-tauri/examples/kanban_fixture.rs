//! Deterministic, fictional data for native screenshot capture only.
//! Run with a database path below /tmp, never with a user's application DB.
use codetally_lib::{
    db::Database,
    kanban,
    models::{AppSettings, Issue, KanbanMetadata, PullRequest, Repository},
    sync,
};
use rusqlite::Connection;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("provide a temporary database path")?,
    );
    if !path.starts_with(std::env::temp_dir()) || path.exists() {
        return Err("path must be a new database under the system temporary directory".into());
    }
    std::fs::create_dir_all(path.parent().ok_or("database has no parent")?)?;
    let db = Database::new(&path);
    db.init()?;
    db.set_metadata("github_login", "demo")?;
    db.set_metadata("github_discovered_at", "2026-09-27T12:00:00Z")?;
    db.set_metadata(sync::LAST_LOC_REFRESH_METADATA_KEY, "2026-09-27T09:30:00Z")?;
    db.set_metadata(sync::LAST_FULL_REFRESH_METADATA_KEY, "2026-09-27T09:30:00Z")?;
    db.set_metadata(
        sync::LAST_ACTIVITY_REFRESH_METADATA_KEY,
        "2026-09-27T12:00:00Z",
    )?;
    sync::save_app_settings(
        &db,
        &AppSettings {
            kanban_enabled: true,
            run_in_background: false,
            ..AppSettings::default()
        },
    )?;
    let repositories = [
        ("repo-story-planner", "storyforge", "planner"),
        ("repo-story-mobile", "storyforge", "mobile"),
        ("repo-orbit-api", "orbit-labs", "api"),
    ];
    let mut stored = Vec::new();
    for (github_id, owner, name) in repositories {
        let repo = Repository {
            github_id: github_id.into(),
            owner: owner.into(),
            name: name.into(),
            name_with_owner: format!("{owner}/{name}"),
            url: format!("https://github.com/{owner}/{name}"),
            ssh_url: format!("git@github.com:{owner}/{name}.git"),
            default_branch: "main".into(),
            primary_language: Some(if name == "api" { "Rust" } else { "TypeScript" }.into()),
            pushed_at: Some("2026-09-27T08:00:00Z".into()),
            github_updated_at: Some("2026-09-27T08:00:00Z".into()),
            ..Repository::default()
        };
        let id = db.upsert_repository(&repo)?;
        kanban::remember_repository(&db, github_id)?;
        kanban::record_activity(&db, github_id, true, None)?;
        stored.push((id, repo));
    }
    let date = "2026-09-27T09:30:00Z";
    for (repository_index, number, title, state, draft, ci) in [
        (
            0,
            42,
            "Add accessible planning timeline",
            "OPEN",
            false,
            "success",
        ),
        (
            1,
            87,
            "Polish offline sign-in flow",
            "OPEN",
            true,
            "pending",
        ),
        (
            2,
            16,
            "Reduce API response latency",
            "OPEN",
            false,
            "failure",
        ),
        (
            0,
            31,
            "Ship keyboard navigation",
            "MERGED",
            false,
            "success",
        ),
        (
            1,
            72,
            "Retire legacy notification",
            "CLOSED",
            false,
            "failure",
        ),
    ] {
        let (repository_id, repo) = &stored[repository_index];
        db.upsert_pull_request(&PullRequest {
            repository_id: *repository_id,
            repository: repo.name_with_owner.clone(),
            number,
            title: title.into(),
            state: state.into(),
            is_draft: draft,
            created_at: "2026-09-20T10:00:00Z".into(),
            updated_at: date.into(),
            merged_at: if state == "MERGED" {
                Some(date.into())
            } else {
                None
            },
            closed_at: if state != "OPEN" {
                Some(date.into())
            } else {
                None
            },
            url: format!("{}/pull/{number}", repo.url),
            author: Some(if number == 16 { "morgan" } else { "demo" }.into()),
            assignees: vec!["alex".into()],
            additions: 120,
            deletions: 36,
            changed_files: 5,
            ci_state: Some(ci.into()),
        })?;
        kanban::store_identity(
            &db,
            *repository_id,
            &repo.github_id,
            "pr",
            number,
            Some(&format!("PR_{repository_id}_{number}")),
            None,
        )?;
    }
    for (repository_index, number, title, state, reason) in [
        (0, 105, "Clarify weekly planning view", "OPEN", None),
        (2, 204, "Document API timeout behavior", "OPEN", None),
        (
            0,
            96,
            "Improve screen reader labels",
            "CLOSED",
            Some("COMPLETED"),
        ),
    ] {
        let (repository_id, repo) = &stored[repository_index];
        db.upsert_issue(&Issue {
            repository_id: *repository_id,
            repository: repo.name_with_owner.clone(),
            number,
            title: title.into(),
            state: state.into(),
            created_at: "2026-09-19T10:00:00Z".into(),
            updated_at: date.into(),
            closed_at: if state == "CLOSED" {
                Some(date.into())
            } else {
                None
            },
            url: format!("{}/issues/{number}", repo.url),
            author: Some("alex".into()),
            assignees: vec!["demo".into()],
            labels: vec!["accessibility".into()],
        })?;
        kanban::store_identity(
            &db,
            *repository_id,
            &repo.github_id,
            "issue",
            number,
            Some(&format!("ISSUE_{repository_id}_{number}")),
            reason,
        )?;
    }
    for (repo_index, kind, number, column, priority, notes, rank) in [
        (
            0,
            "pr",
            42,
            Some("In progress"),
            "High",
            "Check the keyboard journey before release.",
            -2048,
        ),
        (
            1,
            "pr",
            87,
            Some("Blocked"),
            "Medium",
            "Waiting for the copy review.",
            -1024,
        ),
        (
            2,
            "pr",
            16,
            Some("Review"),
            "High",
            "Review retry behavior on a slow connection.",
            0,
        ),
        (
            0,
            "issue",
            105,
            Some("Todo"),
            "Low",
            "Draft clear acceptance criteria.",
            0,
        ),
    ] {
        let (repository_id, repo) = &stored[repo_index];
        kanban::save_metadata(
            &db,
            KanbanMetadata {
                item_key: kanban::item_key(
                    &repo.github_id,
                    kind,
                    number,
                    Some(&format!(
                        "{}_{}_{}",
                        if kind == "pr" { "PR" } else { "ISSUE" },
                        repository_id,
                        number
                    )),
                ),
                manual_column: column.map(str::to_owned),
                priority: priority.into(),
                notes: notes.into(),
                sort_rank: rank,
                revision: 0,
            },
            0,
        )?;
    }
    let conn = Connection::open(&path)?;
    let (repository_id, repo) = &stored[0];
    let pr_key = kanban::item_key(
        &repo.github_id,
        "pr",
        42,
        Some(&format!("PR_{repository_id}_42")),
    );
    conn.execute("INSERT INTO kanban_links_cache(account_scope,item_key,links_json,partial,message,updated_at,next_cursor) VALUES ('github.com:demo',?1,?2,0,NULL,?3,NULL)",rusqlite::params![pr_key,r#"[{"kind":"issue","repository":"storyforge/planner","number":105,"title":"Clarify weekly planning view","state":"OPEN","url":"https://github.com/storyforge/planner/issues/105"}]"#,chrono::Utc::now().to_rfc3339()])?;
    println!("{}", path.display());
    Ok(())
}
