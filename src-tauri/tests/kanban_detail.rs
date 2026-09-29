//! Read-only ticket discussion reads through a fake gh executable.
use codetally_lib::db::Database;
use codetally_lib::kanban;
use codetally_lib::kanban_detail;
use codetally_lib::models::{AppSettings, Issue, PullRequest, Repository};
use codetally_lib::sync;
use rusqlite::{params, Connection};
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn root() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("codetally-detail-{}-{suffix}", std::process::id()))
}

#[cfg(unix)]
#[test]
fn paginates_read_only_comments_and_checks_then_keeps_cache_offline_and_account_scoped() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let db_path = root.join("test.sqlite3");
    let database = Database::new(&db_path);
    database.init().unwrap();
    database.set_metadata("github_login", "alice").unwrap();
    sync::save_app_settings(
        &database,
        &AppSettings {
            kanban_enabled: true,
            ..AppSettings::default()
        },
    )
    .unwrap();
    let repo = Repository {
        github_id: "REPO_TEST".into(),
        owner: "example".into(),
        name: "repo".into(),
        name_with_owner: "example/repo".into(),
        url: "https://github.com/example/repo".into(),
        ..Repository::default()
    };
    let repo_id = database.upsert_repository(&repo).unwrap();
    kanban::remember_repository(&database, &repo.github_id).unwrap();
    database
        .upsert_pull_request(&PullRequest {
            repository_id: repo_id,
            number: 7,
            title: "Review".into(),
            state: "OPEN".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            url: "https://github.com/example/repo/pull/7".into(),
            ..PullRequest::default()
        })
        .unwrap();
    kanban::store_identity(
        &database,
        repo_id,
        &repo.github_id,
        "pr",
        7,
        Some("PR_TEST"),
        None,
    )
    .unwrap();
    database
        .upsert_issue(&Issue {
            repository_id: repo_id,
            number: 8,
            title: "Issue".into(),
            state: "OPEN".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            url: "https://github.com/example/repo/issues/8".into(),
            ..Issue::default()
        })
        .unwrap();
    kanban::store_identity(
        &database,
        repo_id,
        &repo.github_id,
        "issue",
        8,
        Some("ISSUE_TEST"),
        None,
    )
    .unwrap();

    let comment = |id: &str, number: i64| json!({"id":id,"author":{"login":"alice"},"bodyText":"Plain <script>text</script>","createdAt":"2026-09-28T00:00:00Z","updatedAt":"2026-09-28T00:00:00Z","url":format!("https://github.com/example/repo/pull/7#issuecomment-{number}")});
    let comments = |nodes: Vec<serde_json::Value>, more: bool, cursor: &str| json!({"data":{"rateLimit":{"remaining":5000},"repository":{"item":{"comments":{"totalCount":2,"nodes":nodes,"pageInfo":{"hasPreviousPage":more,"startCursor":cursor}}}}}});
    let check = |id: &str, name: &str, status: &str| json!({"__typename":"CheckRun","id":id,"name":name,"status":"COMPLETED","conclusion":status,"detailsUrl":"https://github.com/example/repo/actions/runs/1"});
    let checks = |nodes: Vec<serde_json::Value>, more: bool, cursor: &str| json!({"data":{"rateLimit":{"remaining":5000},"repository":{"item":{"commits":{"nodes":[{"commit":{"oid":"HEAD_ONE","statusCheckRollup":{"contexts":{"totalCount":2,"nodes":nodes,"pageInfo":{"hasNextPage":more,"endCursor":cursor}}}}}]}}}}});
    let commit = |oid: &str| json!({"commit":{"oid":oid,"messageHeadline":format!("Change {oid}"),"committedDate":"2026-09-28T00:00:00Z","url":format!("https://github.com/example/repo/commit/{oid}"),"author":{"user":{"login":"alice"},"name":"Alice"}}});
    let commits = |nodes: Vec<serde_json::Value>, more: bool, cursor: &str| json!({"data":{"rateLimit":{"remaining":5000},"repository":{"item":{"commits":{"totalCount":2,"nodes":nodes,"pageInfo":{"hasPreviousPage":more,"startCursor":cursor}}}}}});
    let mut combined = commits(vec![commit("HEAD_ONE")], true, "older-commit");
    combined["data"]["repository"]["item"]["head"] =
        checks(vec![check("CHECK_ONE", "Build", "SUCCESS")], true, "next")["data"]["repository"]
            ["item"]["commits"]
            .clone();
    for (name, body) in [
        (
            "comments-new.json",
            comments(vec![comment("COMMENT_NEW", 2)], true, "older"),
        ),
        (
            "comments-old.json",
            comments(vec![comment("COMMENT_OLD", 1)], false, ""),
        ),
        (
            "checks-first.json",
            checks(vec![check("CHECK_ONE", "Build", "SUCCESS")], true, "next"),
        ),
        (
            "checks-next.json",
            checks(vec![check("CHECK_TWO", "Tests", "FAILURE")], false, ""),
        ),
        (
            "commits-new.json",
            commits(vec![commit("HEAD_ONE")], true, "older-commit"),
        ),
        (
            "commits-old.json",
            commits(vec![commit("HEAD_ZERO")], false, ""),
        ),
        ("combined.json", combined),
    ] {
        std::fs::write(root.join(name), body.to_string()).unwrap();
    }
    let script = r#"#!/bin/sh
printf '%s\n' "$*" >> "$CODETALLY_FAKE_GH_LOG"
if [ "${CODETALLY_FAKE_GH_FAIL:-0}" = 1 ]; then echo offline >&2; exit 1; fi
case "$*" in
  *"comments(last:50"*)
    case "$*" in *"before=older"*) cat "$CODETALLY_FAKE_GH_ROOT/comments-old.json";; *) cat "$CODETALLY_FAKE_GH_ROOT/comments-new.json";; esac ;;
  *"head:commits(last:1"*) cat "$CODETALLY_FAKE_GH_ROOT/combined.json" ;;
  *"commits(last:30"*)
    case "$*" in *"before=older-commit"*) cat "$CODETALLY_FAKE_GH_ROOT/commits-old.json";; *) cat "$CODETALLY_FAKE_GH_ROOT/commits-new.json";; esac ;;
  *"statusCheckRollup"*)
    case "$*" in *"after=next"*) cat "$CODETALLY_FAKE_GH_ROOT/checks-next.json";; *) cat "$CODETALLY_FAKE_GH_ROOT/checks-first.json";; esac ;;
  *) echo unexpected-gh-command >&2; exit 1 ;;
esac
"#;
    let gh = bin.join("gh");
    std::fs::write(&gh, script).unwrap();
    let mut permissions = std::fs::metadata(&gh).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&gh, permissions).unwrap();
    let log = root.join("gh.log");
    std::fs::write(&log, "").unwrap();
    let original_path = std::env::var_os("PATH");
    let mut paths = vec![bin];
    if let Some(path) = &original_path {
        paths.extend(std::env::split_paths(path));
    }
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    std::env::set_var("CODETALLY_FAKE_GH_ROOT", &root);
    std::env::set_var("CODETALLY_FAKE_GH_LOG", &log);

    let key = "github.com:node:PR_TEST";
    let first = kanban_detail::load(&database, key, None, false).unwrap();
    assert_eq!(
        (
            first.comments.len(),
            first.comment_count,
            first.checks.len(),
            first.check_count
        ),
        (1, 2, 1, 2)
    );
    assert!(first.comments_has_more && first.checks_has_more && !first.partial);
    assert_eq!(
        (
            first.commits.len(),
            first.commit_count,
            first.commits_has_more
        ),
        (1, 2, true)
    );
    assert_eq!(first.comments[0].body_text, "Plain <script>text</script>");
    let calls_after_first = std::fs::read_to_string(&log).unwrap().lines().count();
    assert_eq!(calls_after_first, 2);
    assert_eq!(
        kanban_detail::load(&database, key, None, false)
            .unwrap()
            .comments
            .len(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        calls_after_first,
        "fresh cache should avoid more gh calls"
    );
    assert_eq!(
        kanban_detail::load(&database, key, None, true)
            .unwrap()
            .comments
            .len(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        4,
        "explicit discussion refresh bypasses the cache window"
    );
    let comments = kanban_detail::load(&database, key, Some("comments"), false).unwrap();
    assert_eq!(
        comments
            .comments
            .iter()
            .map(|comment| comment.id.as_str())
            .collect::<Vec<_>>(),
        vec!["COMMENT_OLD", "COMMENT_NEW"]
    );
    assert!(!comments.comments_has_more);
    let checks = kanban_detail::load(&database, key, Some("checks"), false).unwrap();
    assert_eq!(
        checks
            .checks
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Build", "Tests"]
    );
    assert!(!checks.checks_has_more);
    let commits = kanban_detail::load(&database, key, Some("commits"), false).unwrap();
    assert_eq!(
        commits
            .commits
            .iter()
            .map(|commit| commit.oid.as_str())
            .collect::<Vec<_>>(),
        vec!["HEAD_ZERO", "HEAD_ONE"]
    );
    assert!(!commits.commits_has_more);
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.lines().count(), 7);
    assert!(calls
        .lines()
        .all(|call| call.starts_with("api graphql ") && call.contains("query=query(")));
    assert!(
        !calls.contains("mutation")
            && !calls.contains("issue comment")
            && !calls.contains("pr comment")
    );

    let issue = kanban_detail::load(&database, "github.com:node:ISSUE_TEST", None, false).unwrap();
    assert!(
        issue.checks.is_empty() && issue.commits.is_empty(),
        "issues have no PR checks or commits"
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 8);
    Connection::open(&db_path).unwrap().execute("UPDATE kanban_discussion_cache SET refreshed_at=?1 WHERE account_scope='github.com:alice' AND item_key=?2", params!["2020-01-01T00:00:00Z", key]).unwrap();
    std::env::set_var("CODETALLY_FAKE_GH_FAIL", "1");
    let offline = kanban_detail::load(&database, key, None, false).unwrap();
    assert!(offline.partial && offline.message.unwrap().contains("could not be refreshed"));
    assert_eq!(
        (
            offline.comments.len(),
            offline.checks.len(),
            offline.commits.len()
        ),
        (2, 2, 2),
        "offline reads preserve saved data"
    );
    std::env::remove_var("CODETALLY_FAKE_GH_FAIL");
    let recovered = kanban_detail::load(&database, key, None, false).unwrap();
    assert!(
        !recovered.partial && recovered.message.is_none(),
        "a failed refresh must not make the old success timestamp hide a retry"
    );
    assert_eq!(recovered.check_count, 2);

    database.set_metadata("github_login", "bob").unwrap();
    kanban::remember_repository(&database, &repo.github_id).unwrap();
    let bob = kanban_detail::cached(&database, key, None).unwrap();
    assert!(
        bob.comments.is_empty() && bob.checks.is_empty() && bob.commits.is_empty(),
        "new account cannot see Alice's discussion cache"
    );
    match original_path {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }
    for key in [
        "CODETALLY_FAKE_GH_ROOT",
        "CODETALLY_FAKE_GH_LOG",
        "CODETALLY_FAKE_GH_FAIL",
    ] {
        std::env::remove_var(key);
    }
    let _ = std::fs::remove_dir_all(root);
}
