//! Read-only, on-demand GitHub conversation comments, PR commits, and checks.
//! Pages are cached separately from local planning metadata and scoped to the
//! signed-in account. No request in this module performs a GitHub mutation.
use crate::db::Database;
use crate::error::{AppError, AppResult};
use crate::models::{KanbanCheck, KanbanComment, KanbanCommit, KanbanDiscussion};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Default)]
struct Cached {
    comments: Vec<KanbanComment>,
    comment_count: i64,
    comments_cursor: Option<String>,
    checks: Vec<KanbanCheck>,
    check_count: i64,
    checks_cursor: Option<String>,
    checks_commit_oid: Option<String>,
    commits: Vec<KanbanCommit>,
    commit_count: i64,
    commits_cursor: Option<String>,
    refreshed_at: Option<String>,
    last_error: Option<String>,
}

fn read_cache(db: &Database, scope: &str, item_key: &str) -> AppResult<Option<Cached>> {
    let conn = db.connect()?;
    let row: Option<(String, i64, Option<String>, String, i64, Option<String>, Option<String>, Option<String>, String, i64, Option<String>, Option<String>)> = conn.query_row(
        "SELECT comments_json,comment_count,comments_cursor,checks_json,check_count,checks_cursor,checks_commit_oid,refreshed_at,commits_json,commit_count,commits_cursor,last_error
         FROM kanban_discussion_cache WHERE account_scope=?1 AND item_key=?2",
        params![scope, item_key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?)),
    ).optional()?;
    row.map(
        |(
            comments,
            comment_count,
            comments_cursor,
            checks,
            check_count,
            checks_cursor,
            checks_commit_oid,
            refreshed_at,
            commits,
            commit_count,
            commits_cursor,
            last_error,
        )| {
            Ok(Cached {
                comments: serde_json::from_str(&comments)?,
                comment_count,
                comments_cursor,
                checks: serde_json::from_str(&checks)?,
                check_count,
                checks_cursor,
                checks_commit_oid,
                refreshed_at,
                commits: serde_json::from_str(&commits)?,
                commit_count,
                commits_cursor,
                last_error,
            })
        },
    )
    .transpose()
}

fn result(cache: Cached, partial: bool, message: Option<String>) -> KanbanDiscussion {
    KanbanDiscussion {
        comments_has_more: cache.comments_cursor.is_some(),
        checks_has_more: cache.checks_cursor.is_some(),
        commits_has_more: cache.commits_cursor.is_some(),
        comments: cache.comments,
        comment_count: cache.comment_count,
        checks: cache.checks,
        check_count: cache.check_count,
        commits: cache.commits,
        commit_count: cache.commit_count,
        refreshed_at: cache.refreshed_at,
        partial,
        message,
    }
}

pub fn cached(db: &Database, item_key: &str, reason: Option<&str>) -> AppResult<KanbanDiscussion> {
    if !crate::sync::app_settings(db)?.kanban_enabled {
        return Err(AppError::InvalidArgument(
            "Enable Kanban in Settings first".into(),
        ));
    }
    let Some(scope) = crate::kanban::account_scope(db)? else {
        return Ok(result(
            Cached::default(),
            true,
            Some("No signed-in GitHub account".into()),
        ));
    };
    let row = read_cache(db, &scope, item_key)?;
    Ok(result(
        row.unwrap_or_default(),
        true,
        Some(
            reason
                .unwrap_or("Comments, commits, and checks have not been loaded yet")
                .into(),
        ),
    ))
}

fn origin(
    db: &Database,
    scope: &str,
    item_key: &str,
) -> AppResult<Option<(String, String, String, i64)>> {
    Ok(db.connect()?.query_row(
        "SELECT kind,owner,name,number FROM (
           SELECT 'pr' kind,r.owner,r.name,p.number,r.github_id,p.node_id FROM pull_requests p JOIN repositories r ON r.id=p.repository_id
           UNION ALL SELECT 'issue',r.owner,r.name,i.number,r.github_id,i.node_id FROM issues i JOIN repositories r ON r.id=i.repository_id
         ) w JOIN kanban_account_repositories a ON a.github_repository_id=w.github_id AND a.account_scope=?1
         WHERE 'github.com:' || CASE WHEN w.node_id IS NOT NULL AND w.node_id<>'' THEN 'node:'||w.node_id ELSE 'repo:'||w.github_id||':'||w.kind||':'||w.number END=?2",
        params![scope, item_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?)
}

fn github_url(value: Option<&str>) -> Option<String> {
    value
        .and_then(|raw| url::Url::parse(raw).ok())
        .filter(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("github.com")
                && url.username().is_empty()
                && url.password().is_none()
        })
        .map(|url| url.to_string())
}

fn safe_check_url(value: Option<&str>) -> Option<String> {
    value
        .and_then(|raw| url::Url::parse(raw).ok())
        .filter(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
        .map(|url| url.to_string())
}

fn comments_page(response: &Value) -> AppResult<(Vec<KanbanComment>, i64, Option<String>)> {
    let connection = &response["data"]["repository"]["item"]["comments"];
    let nodes = connection["nodes"].as_array().ok_or_else(|| {
        AppError::InvalidArgument("GitHub comments response is incomplete".into())
    })?;
    let count = connection["totalCount"]
        .as_i64()
        .ok_or_else(|| AppError::InvalidArgument("GitHub comment count is missing".into()))?;
    let more = connection["pageInfo"]["hasPreviousPage"]
        .as_bool()
        .ok_or_else(|| {
            AppError::InvalidArgument("GitHub comment page information is missing".into())
        })?;
    let cursor = if more {
        Some(
            connection["pageInfo"]["startCursor"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    AppError::InvalidArgument("GitHub comment cursor is missing".into())
                })?
                .to_owned(),
        )
    } else {
        None
    };
    let mut comments = Vec::with_capacity(nodes.len());
    for node in nodes {
        comments.push(KanbanComment {
            id: node["id"]
                .as_str()
                .ok_or_else(|| AppError::InvalidArgument("GitHub comment ID is missing".into()))?
                .into(),
            author: node["author"]["login"].as_str().map(str::to_owned),
            body_text: node["bodyText"]
                .as_str()
                .ok_or_else(|| AppError::InvalidArgument("GitHub comment text is missing".into()))?
                .into(),
            body_markdown: node["body"].as_str().map(str::to_owned),
            created_at: node["createdAt"]
                .as_str()
                .ok_or_else(|| AppError::InvalidArgument("GitHub comment date is missing".into()))?
                .into(),
            updated_at: node["updatedAt"]
                .as_str()
                .ok_or_else(|| {
                    AppError::InvalidArgument("GitHub comment update date is missing".into())
                })?
                .into(),
            url: github_url(node["url"].as_str()).unwrap_or_default(),
        });
    }
    Ok((comments, count, cursor))
}

fn checks_page(
    response: &Value,
) -> AppResult<(Vec<KanbanCheck>, i64, Option<String>, Option<String>)> {
    if response["data"]["repository"]["item"].is_null() {
        return Err(AppError::InvalidArgument("GitHub PR is unavailable".into()));
    }
    let item = &response["data"]["repository"]["item"];
    let head = if item["head"].is_null() {
        &item["commits"]
    } else {
        &item["head"]
    };
    let commit = &head["nodes"][0]["commit"];
    if commit.is_null() {
        return Ok((Vec::new(), 0, None, None));
    }
    let oid = commit["oid"]
        .as_str()
        .ok_or_else(|| AppError::InvalidArgument("GitHub PR commit ID is missing".into()))?
        .to_owned();
    let rollup = &commit["statusCheckRollup"];
    if rollup.is_null() {
        return Ok((Vec::new(), 0, None, Some(oid)));
    }
    let connection = &rollup["contexts"];
    let nodes = connection["nodes"]
        .as_array()
        .ok_or_else(|| AppError::InvalidArgument("GitHub checks response is incomplete".into()))?;
    let count = connection["totalCount"]
        .as_i64()
        .ok_or_else(|| AppError::InvalidArgument("GitHub check count is missing".into()))?;
    let more = connection["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or_else(|| {
            AppError::InvalidArgument("GitHub check page information is missing".into())
        })?;
    let cursor = if more {
        Some(
            connection["pageInfo"]["endCursor"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| AppError::InvalidArgument("GitHub check cursor is missing".into()))?
                .to_owned(),
        )
    } else {
        None
    };
    let mut checks = Vec::with_capacity(nodes.len());
    for node in nodes {
        let kind = node["__typename"].as_str().unwrap_or("");
        let (name, status, details_url) = match kind {
            "CheckRun" => (
                node["name"].as_str(),
                node["conclusion"]
                    .as_str()
                    .or_else(|| node["status"].as_str()),
                safe_check_url(node["detailsUrl"].as_str()),
            ),
            "StatusContext" => (
                node["context"].as_str(),
                node["state"].as_str(),
                safe_check_url(node["targetUrl"].as_str()),
            ),
            _ => {
                return Err(AppError::InvalidArgument(
                    "Unknown GitHub check type".into(),
                ))
            }
        };
        checks.push(KanbanCheck {
            id: node["id"]
                .as_str()
                .ok_or_else(|| AppError::InvalidArgument("GitHub check ID is missing".into()))?
                .into(),
            name: name
                .ok_or_else(|| AppError::InvalidArgument("GitHub check name is missing".into()))?
                .into(),
            status: status.unwrap_or("UNKNOWN").into(),
            details_url,
        });
    }
    Ok((checks, count, cursor, Some(oid)))
}

fn commits_page(response: &Value) -> AppResult<(Vec<KanbanCommit>, i64, Option<String>)> {
    let connection = &response["data"]["repository"]["item"]["commits"];
    let nodes = connection["nodes"]
        .as_array()
        .ok_or_else(|| AppError::InvalidArgument("GitHub commits response is incomplete".into()))?;
    let count = connection["totalCount"]
        .as_i64()
        .ok_or_else(|| AppError::InvalidArgument("GitHub commit count is missing".into()))?;
    let more = connection["pageInfo"]["hasPreviousPage"]
        .as_bool()
        .ok_or_else(|| {
            AppError::InvalidArgument("GitHub commit page information is missing".into())
        })?;
    let cursor = if more {
        Some(
            connection["pageInfo"]["startCursor"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| AppError::InvalidArgument("GitHub commit cursor is missing".into()))?
                .to_owned(),
        )
    } else {
        None
    };
    let mut commits = Vec::with_capacity(nodes.len());
    for node in nodes {
        let commit = &node["commit"];
        commits.push(KanbanCommit {
            oid: commit["oid"]
                .as_str()
                .ok_or_else(|| AppError::InvalidArgument("GitHub commit ID is missing".into()))?
                .into(),
            headline: commit["messageHeadline"]
                .as_str()
                .ok_or_else(|| {
                    AppError::InvalidArgument("GitHub commit headline is missing".into())
                })?
                .into(),
            committed_at: commit["committedDate"]
                .as_str()
                .ok_or_else(|| AppError::InvalidArgument("GitHub commit date is missing".into()))?
                .into(),
            author: commit["author"]["user"]["login"]
                .as_str()
                .or_else(|| commit["author"]["name"].as_str())
                .map(str::to_owned),
            url: github_url(commit["url"].as_str()).unwrap_or_default(),
        });
    }
    Ok((commits, count, cursor))
}

fn save_cache(db: &Database, scope: &str, item_key: &str, cache: &Cached) -> AppResult<()> {
    db.connect()?.execute(
        "INSERT INTO kanban_discussion_cache(account_scope,item_key,comments_json,comment_count,comments_cursor,checks_json,check_count,checks_cursor,checks_commit_oid,refreshed_at,commits_json,commit_count,commits_cursor,last_error)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(account_scope,item_key) DO UPDATE SET comments_json=excluded.comments_json,comment_count=excluded.comment_count,
         comments_cursor=excluded.comments_cursor,checks_json=excluded.checks_json,check_count=excluded.check_count,
         checks_cursor=excluded.checks_cursor,checks_commit_oid=excluded.checks_commit_oid,refreshed_at=excluded.refreshed_at,
         commits_json=excluded.commits_json,commit_count=excluded.commit_count,commits_cursor=excluded.commits_cursor,last_error=excluded.last_error",
        params![scope,item_key,serde_json::to_string(&cache.comments)?,cache.comment_count,cache.comments_cursor,
            serde_json::to_string(&cache.checks)?,cache.check_count,cache.checks_cursor,cache.checks_commit_oid,cache.refreshed_at,
            serde_json::to_string(&cache.commits)?,cache.commit_count,cache.commits_cursor,cache.last_error],
    )?;
    Ok(())
}

/// Fetch the most recent conversation comments, commits, and current-head checks. Older
/// comments/commits and remaining checks are fetched one bounded page per explicit
/// "load more" action. Only queries are issued through the shared gh wrapper.
pub fn load(
    db: &Database,
    item_key: &str,
    load_more: Option<&str>,
    force_refresh: bool,
) -> AppResult<KanbanDiscussion> {
    if !crate::sync::app_settings(db)?.kanban_enabled {
        return Err(AppError::InvalidArgument(
            "Enable Kanban in Settings first".into(),
        ));
    }
    if !matches!(load_more, None | Some("comments" | "checks" | "commits")) {
        return Err(AppError::InvalidArgument(
            "Unknown discussion section".into(),
        ));
    }
    let Some(scope) = crate::kanban::account_scope(db)? else {
        return Ok(result(
            Cached::default(),
            true,
            Some("No signed-in GitHub account".into()),
        ));
    };
    let Some((kind, owner, name, number)) = origin(db, &scope, item_key)? else {
        return Ok(result(
            Cached::default(),
            true,
            Some("Item is unavailable for this account".into()),
        ));
    };
    let previous = read_cache(db, &scope, item_key)?;
    if load_more.is_none()
        && !force_refresh
        && previous
            .as_ref()
            .is_some_and(|row| row.last_error.is_none() && row.comments.iter().all(|comment| comment.body_markdown.is_some()))
        && previous
            .as_ref()
            .and_then(|row| row.refreshed_at.as_deref())
            .and_then(|date| date.parse::<DateTime<Utc>>().ok())
            .is_some_and(|date| Utc::now() - date < Duration::minutes(5))
    {
        return Ok(result(previous.expect("fresh cache exists"), false, None));
    }
    let mut cache = previous.unwrap_or_default();
    let mut errors = Vec::new();
    let mut attempted = 0;
    let mut successful = 0;
    if load_more.is_none() || load_more == Some("comments") && cache.comments_cursor.is_some() {
        attempted += 1;
        let query = format!("query($owner:String!,$name:String!,$number:Int!,$before:String){{rateLimit{{cost remaining resetAt}} repository(owner:$owner,name:$name){{item:{}(number:$number){{comments(last:50,before:$before){{totalCount nodes{{id author{{login}} bodyText body createdAt updatedAt url}} pageInfo{{hasPreviousPage startCursor}}}}}}}}}}", if kind == "pr" { "pullRequest" } else { "issue" });
        let before = if load_more == Some("comments") {
            cache.comments_cursor.clone()
        } else {
            None
        };
        match crate::github_sync::graphql(
            db,
            &query,
            json!({"owner":owner,"name":name,"number":number,"before":before}),
        )
        .and_then(|response| comments_page(&response))
        {
            Ok((page, count, cursor)) => {
                if load_more == Some("comments") && cursor == before {
                    errors.push("GitHub comment cursor did not advance".to_owned());
                } else {
                    if load_more.is_none() {
                        cache.comments = page;
                    } else {
                        let mut combined = page;
                        combined.extend(cache.comments);
                        let mut seen = HashSet::new();
                        combined.retain(|comment| seen.insert(comment.id.clone()));
                        cache.comments = combined;
                    }
                    cache.comment_count = count;
                    cache.comments_cursor = cursor;
                    successful += 1;
                }
            }
            Err(error) => errors.push(format!("Comments could not be refreshed: {error}")),
        }
    }
    if kind == "pr" && load_more.is_none() {
        attempted += 2;
        let query = "query($owner:String!,$name:String!,$number:Int!){rateLimit{cost remaining resetAt} repository(owner:$owner,name:$name){item:pullRequest(number:$number){commits(last:30){totalCount nodes{commit{oid messageHeadline committedDate url author{user{login} name}}} pageInfo{hasPreviousPage startCursor}} head:commits(last:1){nodes{commit{oid statusCheckRollup{contexts(first:100){totalCount nodes{__typename ... on CheckRun{id name status conclusion detailsUrl} ... on StatusContext{id context state targetUrl}} pageInfo{hasNextPage endCursor}}}}}}}}}";
        match crate::github_sync::graphql(
            db,
            query,
            json!({"owner":owner,"name":name,"number":number}),
        ) {
            Ok(response) => {
                match commits_page(&response) {
                    Ok((page, count, cursor)) => {
                        cache.commits = page;
                        cache.commit_count = count;
                        cache.commits_cursor = cursor;
                        successful += 1;
                    }
                    Err(error) => errors.push(format!("Commits could not be refreshed: {error}")),
                }
                match checks_page(&response) {
                    Ok((page, count, cursor, oid)) => {
                        cache.checks = page;
                        cache.check_count = count;
                        cache.checks_cursor = cursor;
                        cache.checks_commit_oid = oid;
                        successful += 1;
                    }
                    Err(error) => errors.push(format!("Checks could not be refreshed: {error}")),
                }
            }
            Err(error) => errors.push(format!(
                "Commits and checks could not be refreshed: {error}"
            )),
        }
    }
    if kind == "pr" && load_more == Some("commits") && cache.commits_cursor.is_some() {
        attempted += 1;
        let before = if load_more == Some("commits") {
            cache.commits_cursor.clone()
        } else {
            None
        };
        let query = "query($owner:String!,$name:String!,$number:Int!,$before:String){rateLimit{cost remaining resetAt} repository(owner:$owner,name:$name){item:pullRequest(number:$number){commits(last:30,before:$before){totalCount nodes{commit{oid messageHeadline committedDate url author{user{login} name}}} pageInfo{hasPreviousPage startCursor}}}}}";
        match crate::github_sync::graphql(
            db,
            query,
            json!({"owner":owner,"name":name,"number":number,"before":before}),
        )
        .and_then(|response| commits_page(&response))
        {
            Ok((page, count, cursor)) => {
                if load_more == Some("commits") && cursor == before {
                    errors.push("GitHub commit cursor did not advance".to_owned());
                } else {
                    if load_more.is_none() {
                        cache.commits = page;
                    } else {
                        let mut combined = page;
                        combined.extend(cache.commits);
                        let mut seen = HashSet::new();
                        combined.retain(|commit| seen.insert(commit.oid.clone()));
                        cache.commits = combined;
                    }
                    cache.commit_count = count;
                    cache.commits_cursor = cursor;
                    successful += 1;
                }
            }
            Err(error) => errors.push(format!("Commits could not be refreshed: {error}")),
        }
    }
    if kind == "pr" && load_more == Some("checks") && cache.checks_cursor.is_some() {
        attempted += 1;
        let after = if load_more == Some("checks") {
            cache.checks_cursor.clone()
        } else {
            None
        };
        let query = "query($owner:String!,$name:String!,$number:Int!,$after:String){rateLimit{cost remaining resetAt} repository(owner:$owner,name:$name){item:pullRequest(number:$number){commits(last:1){nodes{commit{oid statusCheckRollup{contexts(first:100,after:$after){totalCount nodes{__typename ... on CheckRun{id name status conclusion detailsUrl} ... on StatusContext{id context state targetUrl}} pageInfo{hasNextPage endCursor}}}}}}}}}";
        match crate::github_sync::graphql(
            db,
            query,
            json!({"owner":owner,"name":name,"number":number,"after":after}),
        )
        .and_then(|response| checks_page(&response))
        {
            Ok((page, count, cursor, oid)) => {
                if load_more == Some("checks") && oid == cache.checks_commit_oid && cursor == after
                {
                    errors.push("GitHub check cursor did not advance".to_owned());
                } else {
                    if load_more.is_none() || oid != cache.checks_commit_oid {
                        cache.checks = page;
                    } else {
                        for check in page {
                            if !cache.checks.iter().any(|old| old.id == check.id) {
                                cache.checks.push(check);
                            }
                        }
                    }
                    cache.check_count = count;
                    cache.checks_cursor = cursor;
                    cache.checks_commit_oid = oid;
                    successful += 1;
                }
            }
            Err(error) => errors.push(format!("Checks could not be refreshed: {error}")),
        }
    }
    if load_more.is_none() && attempted == successful {
        cache.refreshed_at = Some(Utc::now().to_rfc3339());
        cache.last_error = None;
    }
    if !errors.is_empty() {
        cache.last_error = Some(errors.join(" "));
    }
    if successful > 0 || !errors.is_empty() {
        save_cache(db, &scope, item_key, &cache)?;
    }
    let partial = cache.last_error.is_some();
    let message = cache.last_error.clone();
    Ok(result(cache, partial, message))
}
