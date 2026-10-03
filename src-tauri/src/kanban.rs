//! Local planning data and paginated reads over the existing GitHub cache.
use crate::db::Database;
use crate::error::{AppError, AppResult};
use crate::models::{
    KanbanItem, KanbanLink, KanbanLinks, KanbanMetadata, KanbanPage, KanbanPreferences,
};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use std::collections::BTreeSet;

pub struct KanbanQuery {
    pub kind: String,
    pub repository_ids: Option<Vec<i64>>,
    pub repository_id: Option<i64>,
    pub relationship: String,
    pub search: String,
    pub show_completed: bool,
    pub offset: i64,
    pub limit: i64,
}

pub fn account_scope(db: &Database) -> AppResult<Option<String>> {
    Ok(db
        .metadata("github_login")?
        .filter(|login| !login.is_empty())
        .map(|login| format!("github.com:{}", login.to_ascii_lowercase())))
}

pub fn remember_repository(db: &Database, github_repository_id: &str) -> AppResult<()> {
    let Some(scope) = account_scope(db)? else {
        return Ok(());
    };
    db.connect()?.execute(
        "INSERT OR IGNORE INTO kanban_account_repositories(account_scope,github_repository_id) VALUES (?1,?2)",
        params![scope, github_repository_id],
    )?;
    Ok(())
}

pub fn account_repository_ids(db: &Database) -> AppResult<BTreeSet<String>> {
    let Some(scope) = account_scope(db)? else {
        return Ok(BTreeSet::new());
    };
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT github_repository_id FROM kanban_account_repositories WHERE account_scope=?1",
    )?;
    let ids = stmt
        .query_map([scope], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

pub fn record_activity(
    db: &Database,
    github_repository_id: &str,
    complete: bool,
    error: Option<&str>,
) -> AppResult<()> {
    let Some(scope) = account_scope(db)? else {
        return Ok(());
    };
    db.connect()?.execute(
        "INSERT INTO kanban_activity_status(account_scope,github_repository_id,last_successful_at,partial,error)
         VALUES (?1,?2,?3,?4,?5) ON CONFLICT(account_scope,github_repository_id) DO UPDATE SET
         last_successful_at=COALESCE(excluded.last_successful_at,kanban_activity_status.last_successful_at),
         partial=excluded.partial,error=excluded.error",
        params![scope, github_repository_id, if complete { Some(Utc::now().to_rfc3339()) } else { None }, !complete, error],
    )?;
    Ok(())
}

pub fn item_key(
    github_repository_id: &str,
    kind: &str,
    number: i64,
    node_id: Option<&str>,
) -> String {
    match node_id.filter(|id| !id.is_empty()) {
        Some(id) => format!("github.com:node:{id}"),
        None => format!("github.com:repo:{github_repository_id}:{kind}:{number}"),
    }
}

/// Promote fallback metadata atomically when a GitHub node ID first arrives.
pub fn store_identity(
    db: &Database,
    repository_id: i64,
    github_repository_id: &str,
    kind: &str,
    number: i64,
    node_id: Option<&str>,
    reason: Option<&str>,
) -> AppResult<()> {
    let mut conn = db.connect()?;
    let tx = conn.transaction()?;
    store_identity_on(&tx, repository_id, github_repository_id, kind, number, node_id, reason)?;
    tx.commit()?;
    Ok(())
}

/// Update identity alongside the activity rows and checkpoint in a caller-owned transaction.
pub(crate) fn store_identity_on(
    conn: &rusqlite::Connection,
    repository_id: i64,
    github_repository_id: &str,
    kind: &str,
    number: i64,
    node_id: Option<&str>,
    reason: Option<&str>,
) -> AppResult<()> {
    match kind {
        "pr" => {
            conn.execute(
                "UPDATE pull_requests SET node_id=?1 WHERE repository_id=?2 AND number=?3",
                params![node_id, repository_id, number],
            )?;
        }
        "issue" => {
            conn.execute("UPDATE issues SET node_id=?1,completion_reason=?4 WHERE repository_id=?2 AND number=?3", params![node_id, repository_id, number, reason])?;
        }
        _ => return Err(AppError::InvalidArgument("Unknown work item kind".into())),
    }
    if node_id.is_some_and(|id| !id.is_empty()) {
        let fallback = item_key(github_repository_id, kind, number, None);
        let stable = item_key(github_repository_id, kind, number, node_id);
        conn.execute(
            "INSERT OR IGNORE INTO kanban_item_metadata(account_scope,item_key,manual_column,priority,notes,sort_rank,revision,created_at,updated_at)
             SELECT account_scope,?2,manual_column,priority,notes,sort_rank,revision,created_at,updated_at
             FROM kanban_item_metadata WHERE item_key=?1",
            params![fallback, stable],
        )?;
        conn.execute(
            "DELETE FROM kanban_item_metadata WHERE item_key=?1",
            params![fallback],
        )?;
    }
    Ok(())
}

const BOARD_BASE: &str = r#"
WITH work AS (
 SELECT 'pr' kind,p.repository_id,r.name_with_owner repository,r.github_id,p.number,p.title,p.state,p.is_draft,p.updated_at,p.url,p.author,p.assignees_json assignees,p.ci_state,p.closed_at,p.merged_at,NULL completion_reason,
        'github.com:' || CASE WHEN p.node_id IS NOT NULL AND p.node_id<>'' THEN 'node:'||p.node_id ELSE 'repo:'||r.github_id||':pr:'||p.number END item_key
 FROM pull_requests p JOIN repositories r ON r.id=p.repository_id
 UNION ALL
 SELECT 'issue',i.repository_id,r.name_with_owner,r.github_id,i.number,i.title,i.state,0,i.updated_at,i.url,i.author,i.assignees_json,NULL,i.closed_at,NULL,i.completion_reason,
        'github.com:' || CASE WHEN i.node_id IS NOT NULL AND i.node_id<>'' THEN 'node:'||i.node_id ELSE 'repo:'||r.github_id||':issue:'||i.number END
 FROM issues i JOIN repositories r ON r.id=i.repository_id
), filtered AS (
 SELECT w.*,m.manual_column,COALESCE(m.priority,'None') priority,COALESCE(m.notes,'') notes,
        CASE WHEN s.error='Repository unavailable' THEN 1 ELSE 0 END unavailable,
        COALESCE(m.sort_rank,0) sort_rank,COALESCE(m.revision,0) revision
 FROM work w JOIN kanban_account_repositories a ON a.github_repository_id=w.github_id AND a.account_scope=?1
 LEFT JOIN kanban_item_metadata m ON m.account_scope=?1 AND m.item_key=w.item_key
 LEFT JOIN kanban_activity_status s ON s.account_scope=?1 AND s.github_repository_id=w.github_id
 WHERE w.repository_id IN (SELECT value FROM json_each(?2))
   AND (?3='both' OR (?3='prs' AND w.kind='pr') OR (?3='issues' AND w.kind='issue'))
   AND (?4 IS NULL OR w.repository_id=?4)
   AND (?5='everyone' OR (?5='author' AND lower(COALESCE(w.author,''))=lower(?6))
     OR (?5='assignee' AND EXISTS (SELECT 1 FROM json_each(w.assignees) WHERE lower(value)=lower(?6)))
     OR (?5='author_or_assignee' AND (lower(COALESCE(w.author,''))=lower(?6)
       OR EXISTS (SELECT 1 FROM json_each(w.assignees) WHERE lower(value)=lower(?6)))))
   AND (?7='' OR lower(w.title) LIKE '%'||lower(?7)||'%' OR lower(w.repository) LIKE '%'||lower(?7)||'%' OR CAST(w.number AS TEXT) LIKE '%'||?7||'%')
)
"#;

pub fn page(db: &Database, query: KanbanQuery) -> AppResult<KanbanPage> {
    if !crate::sync::app_settings(db)?.kanban_enabled {
        return Err(AppError::InvalidArgument(
            "Enable Kanban in Settings first".into(),
        ));
    }
    if !["prs", "issues", "both", "none"].contains(&query.kind.as_str())
        || !["everyone", "author", "assignee", "author_or_assignee"]
            .contains(&query.relationship.as_str())
        || query.offset < 0
        || !(1..=200).contains(&query.limit)
    {
        return Err(AppError::InvalidArgument(
            "Invalid board filter or page size".into(),
        ));
    }
    let Some(scope) = account_scope(db)? else {
        return Ok(KanbanPage::default());
    };
    let login = scope.strip_prefix("github.com:").unwrap_or("");
    let selected = db.selected_repositories()?;
    let selected: Vec<_> = selected
        .into_iter()
        .filter(|r| {
            query
                .repository_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&r.id))
        })
        .collect();
    let ids = serde_json::to_string(&selected.iter().map(|r| r.id).collect::<Vec<_>>())?;
    let conn = db.connect()?;
    let counts_sql = format!("{BOARD_BASE} SELECT SUM(CASE WHEN upper(state)='OPEN' THEN 1 ELSE 0 END),SUM(CASE WHEN upper(state)<>'OPEN' THEN 1 ELSE 0 END) FROM filtered");
    let (active, completed): (Option<i64>, Option<i64>) = conn.query_row(
        &counts_sql,
        params![
            scope,
            ids,
            query.kind,
            query.repository_id,
            query.relationship,
            login,
            query.search
        ],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let active = active.unwrap_or(0);
    let completed = completed.unwrap_or(0);
    let sql = format!("{BOARD_BASE} SELECT item_key,kind,repository_id,repository,number,title,state,is_draft,updated_at,url,author,assignees,ci_state,closed_at,merged_at,completion_reason,manual_column,priority,notes,unavailable,sort_rank,revision FROM filtered WHERE (?8=1 OR upper(state)='OPEN') ORDER BY CASE WHEN upper(state)='OPEN' THEN 0 ELSE 1 END, sort_rank, updated_at DESC, item_key LIMIT ?9 OFFSET ?10");
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(
            params![
                scope,
                ids,
                query.kind,
                query.repository_id,
                query.relationship,
                login,
                query.search,
                query.show_completed,
                query.limit,
                query.offset
            ],
            |row| {
                let assignees: String = row.get(11)?;
                Ok(KanbanItem {
                    item_key: row.get(0)?,
                    kind: row.get(1)?,
                    repository_id: row.get(2)?,
                    repository: row.get(3)?,
                    number: row.get(4)?,
                    title: row.get(5)?,
                    state: row.get(6)?,
                    is_draft: row.get(7)?,
                    updated_at: row.get(8)?,
                    url: row.get(9)?,
                    author: row.get(10)?,
                    assignees: serde_json::from_str(&assignees).unwrap_or_default(),
                    ci_state: row.get(12)?,
                    closed_at: row.get(13)?,
                    merged_at: row.get(14)?,
                    completion_reason: row.get(15)?,
                    manual_column: row.get(16)?,
                    priority: row.get(17)?,
                    notes: row.get(18)?,
                    unavailable: row.get(19)?,
                    sort_rank: row.get(20)?,
                    revision: row.get(21)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut last_refresh: Option<String> = None;
    let mut partial = false;
    let mut errors = Vec::new();
    for repo in &selected {
        let status: Option<(Option<String>, bool, Option<String>)> = conn.query_row("SELECT last_successful_at,partial,error FROM kanban_activity_status WHERE account_scope=?1 AND github_repository_id=?2", params![scope, repo.github_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        match status {
            Some((Some(date), false, None)) => {
                if last_refresh.as_ref().is_none_or(|old| date < *old) {
                    last_refresh = Some(date);
                }
            }
            Some((_, _, error)) => {
                partial = true;
                if let Some(error) = error {
                    errors.push(format!("{}: {error}", repo.name_with_owner));
                }
            }
            None => partial = true,
        }
    }
    if partial {
        last_refresh = None;
    }
    Ok(KanbanPage {
        items,
        total: if query.show_completed {
            active + completed
        } else {
            active
        },
        active_count: active,
        completed_count: completed,
        last_successful_refresh: last_refresh,
        partial,
        errors,
    })
}

pub fn save_metadata(
    db: &Database,
    update: KanbanMetadata,
    expected_revision: i64,
) -> AppResult<KanbanMetadata> {
    if !crate::sync::app_settings(db)?.kanban_enabled {
        return Err(AppError::InvalidArgument(
            "Enable Kanban in Settings first".into(),
        ));
    }
    if ![
        None,
        Some("Todo"),
        Some("In progress"),
        Some("Review"),
        Some("Blocked"),
    ]
    .contains(&update.manual_column.as_deref())
        || !["None", "Low", "Medium", "High"].contains(&update.priority.as_str())
        || update.notes.chars().count() > 4000
        || update.sort_rank.abs() > 1_000_000_000
        || expected_revision < 0
    {
        return Err(AppError::InvalidArgument(
            "Invalid column, priority, note length, rank, or revision".into(),
        ));
    }
    let scope = account_scope(db)?
        .ok_or_else(|| AppError::InvalidArgument("No signed-in GitHub account".into()))?;
    let conn = db.connect()?;
    // The item must belong to a repository previously seen by this account.
    let known: bool = {
        let selected_ids = serde_json::to_string(
            &db.selected_repositories()?
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
        )?;
        conn.query_row(
            &format!("{BOARD_BASE} SELECT EXISTS(SELECT 1 FROM filtered WHERE item_key=?8)"),
            params![
                scope,
                selected_ids,
                "both",
                Option::<i64>::None,
                "everyone",
                "",
                "",
                update.item_key
            ],
            |row| row.get(0),
        )?
    };
    if !known {
        return Err(AppError::InvalidArgument(
            "Work item is unavailable for this account".into(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    let changed = if expected_revision == 0 {
        conn.execute("INSERT OR IGNORE INTO kanban_item_metadata(account_scope,item_key,manual_column,priority,notes,sort_rank,revision,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,1,?7,?7)",params![scope,update.item_key,update.manual_column,update.priority,update.notes,update.sort_rank,now])?
    } else {
        conn.execute("UPDATE kanban_item_metadata SET manual_column=?3,priority=?4,notes=?5,sort_rank=?6,revision=revision+1,updated_at=?7 WHERE account_scope=?1 AND item_key=?2 AND revision=?8",params![scope,update.item_key,update.manual_column,update.priority,update.notes,update.sort_rank,now,expected_revision])?
    };
    if changed == 0 {
        return Err(AppError::InvalidArgument(
            "Board item changed since it was opened; reload and retry".into(),
        ));
    }
    Ok(KanbanMetadata {
        revision: expected_revision + 1,
        ..update
    })
}

pub fn cached_links(db: &Database, item_key: &str) -> AppResult<KanbanLinks> {
    let Some(scope) = account_scope(db)? else {
        return Ok(KanbanLinks {
            partial: true,
            message: Some("No signed-in account".into()),
            ..KanbanLinks::default()
        });
    };
    let conn = db.connect()?;
    let cached: Option<(String,bool,Option<String>)> = conn.query_row("SELECT links_json,partial,message FROM kanban_links_cache WHERE account_scope=?1 AND item_key=?2",params![scope,item_key],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    match cached {
        Some((json, partial, message)) => Ok(KanbanLinks {
            items: serde_json::from_str(&json)?,
            partial,
            message,
        }),
        None => Ok(KanbanLinks {
            partial: true,
            message: Some("Linked work has not been loaded yet".into()),
            ..KanbanLinks::default()
        }),
    }
}

/// Read only explicit GitHub closing references, on demand. A bounded set of
/// pages is cached before returning; later opens resume a partial connection.
pub fn load_links(db: &Database, item_key: &str) -> AppResult<KanbanLinks> {
    if !crate::sync::app_settings(db)?.kanban_enabled {
        return Err(AppError::InvalidArgument(
            "Enable Kanban in Settings first".into(),
        ));
    }
    let Some(scope) = account_scope(db)? else {
        return Ok(KanbanLinks {
            partial: true,
            message: Some("No signed-in account".into()),
            ..KanbanLinks::default()
        });
    };
    let conn = db.connect()?;
    let origin: Option<(String,String,String,i64)> = conn.query_row(
        "SELECT kind,owner,name,number FROM (
           SELECT 'pr' kind,r.owner,r.name,p.number,r.github_id,p.node_id FROM pull_requests p JOIN repositories r ON r.id=p.repository_id
           UNION ALL SELECT 'issue',r.owner,r.name,i.number,r.github_id,i.node_id FROM issues i JOIN repositories r ON r.id=i.repository_id
         ) w JOIN kanban_account_repositories a ON a.github_repository_id=w.github_id AND a.account_scope=?1
         WHERE 'github.com:' || CASE WHEN w.node_id IS NOT NULL AND w.node_id<>'' THEN 'node:'||w.node_id ELSE 'repo:'||w.github_id||':'||w.kind||':'||w.number END=?2",
        params![scope,item_key],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    let Some((kind, owner, name, number)) = origin else {
        return Ok(KanbanLinks {
            partial: true,
            message: Some("Item is unavailable for this account".into()),
            ..KanbanLinks::default()
        });
    };
    let cached: Option<(String,bool,Option<String>,Option<String>,String)> = conn.query_row("SELECT links_json,partial,message,next_cursor,updated_at FROM kanban_links_cache WHERE account_scope=?1 AND item_key=?2",params![scope,item_key],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    if let Some((links, false, message, _, updated)) = &cached {
        if updated
            .parse::<DateTime<Utc>>()
            .ok()
            .is_some_and(|date| Utc::now() - date < Duration::minutes(15))
        {
            return Ok(KanbanLinks {
                items: serde_json::from_str(links)?,
                partial: false,
                message: message.clone(),
            });
        }
    }
    let mut links: Vec<KanbanLink> = cached
        .as_ref()
        .and_then(|(value, partial, _, _, _)| {
            if *partial {
                serde_json::from_str(value).ok()
            } else {
                None
            }
        })
        .unwrap_or_default();
    let previous_complete: Option<Vec<KanbanLink>> =
        cached.as_ref().and_then(|(value, partial, _, _, _)| {
            if !*partial {
                serde_json::from_str(value).ok()
            } else {
                None
            }
        });
    let mut after = cached
        .as_ref()
        .and_then(|(_, partial, _, cursor, _)| if *partial { cursor.clone() } else { None });
    let field = if kind == "pr" {
        "closingIssuesReferences"
    } else {
        "closedByPullRequestsReferences"
    };
    let connection_args = if kind == "pr" {
        "first:100,after:$after"
    } else {
        "first:100,after:$after,includeClosedPrs:true"
    };
    let query = format!("query($owner:String!,$name:String!,$number:Int!,$after:String){{rateLimit{{cost remaining resetAt}} repository(owner:$owner,name:$name){{item:{}(number:$number){{{field}({connection_args}){{nodes{{number title state url repository{{nameWithOwner}}}} pageInfo{{hasNextPage endCursor}}}}}}}}}}",if kind=="pr" {"pullRequest"} else {"issue"});
    let mut partial = false;
    let mut message = None;
    let mut fetched_pages = 0;
    for _ in 0..5 {
        let response = match crate::github_sync::graphql(
            db,
            &query,
            json!({"owner":owner,"name":name,"number":number,"after":after}),
        ) {
            Ok(response) => response,
            Err(error) => {
                partial = true;
                message = Some(format!("Linked work could not be refreshed: {error}"));
                break;
            }
        };
        let connection = &response["data"]["repository"]["item"][field];
        let Some(nodes) = connection["nodes"].as_array() else {
            partial = true;
            message = Some("GitHub did not return linked work".into());
            break;
        };
        fetched_pages += 1;
        for node in nodes {
            let Some(url) = node["url"]
                .as_str()
                .filter(|url| url.starts_with("https://github.com/"))
            else {
                continue;
            };
            let Some(repository) = node["repository"]["nameWithOwner"].as_str() else {
                continue;
            };
            let Some(number) = node["number"].as_i64() else {
                continue;
            };
            if links
                .iter()
                .any(|old| old.repository == repository && old.number == number)
            {
                continue;
            }
            links.push(KanbanLink {
                kind: if kind == "pr" { "issue" } else { "pr" }.into(),
                repository: repository.into(),
                number,
                title: node["title"].as_str().unwrap_or("").into(),
                state: node["state"].as_str().unwrap_or("UNKNOWN").into(),
                url: url.into(),
            });
        }
        let more = connection["pageInfo"]["hasNextPage"]
            .as_bool()
            .unwrap_or(false);
        if !more {
            after = None;
            partial = false;
            message = None;
            break;
        }
        let next = connection["pageInfo"]["endCursor"]
            .as_str()
            .filter(|value| !value.is_empty());
        if next.is_none() || next == after.as_deref() {
            partial = true;
            message = Some("GitHub linked-work pagination stopped before completion".into());
            break;
        }
        after = next.map(str::to_owned);
        partial = true;
    }
    if after.is_some() {
        partial = true;
        message.get_or_insert_with(|| {
            "More linked work is available; reopen details to continue".into()
        });
    }
    if fetched_pages == 0 {
        if let Some(items) = previous_complete {
            return Ok(KanbanLinks {
                items,
                partial: true,
                message,
            });
        }
    }
    conn.execute("INSERT INTO kanban_links_cache(account_scope,item_key,links_json,partial,message,updated_at,next_cursor) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(account_scope,item_key) DO UPDATE SET links_json=excluded.links_json,partial=excluded.partial,message=excluded.message,updated_at=excluded.updated_at,next_cursor=excluded.next_cursor",params![scope,item_key,serde_json::to_string(&links)?,partial,message,Utc::now().to_rfc3339(),after])?;
    Ok(KanbanLinks {
        items: links,
        partial,
        message,
    })
}

pub fn preferences(db: &Database) -> AppResult<KanbanPreferences> {
    let Some(scope) = account_scope(db)? else {
        return Ok(KanbanPreferences::default());
    };
    Ok(db.connect()?.query_row("SELECT kind,repository_scope,relationship,search,show_completed FROM kanban_preferences WHERE account_scope=?1",[scope],|row|Ok(KanbanPreferences {kind:row.get(0)?,repository_scope:row.get(1)?,relationship:row.get(2)?,search:row.get(3)?,show_completed:row.get(4)?})).optional()?.unwrap_or_default())
}

pub fn save_preferences(db: &Database, value: KanbanPreferences) -> AppResult<KanbanPreferences> {
    if !["prs", "issues", "both", "none"].contains(&value.kind.as_str())
        || !["everyone", "author", "assignee", "author_or_assignee"]
            .contains(&value.relationship.as_str())
        || value.repository_scope.len() > 200
        || value.search.chars().count() > 300
    {
        return Err(AppError::InvalidArgument(
            "Invalid Kanban preferences".into(),
        ));
    }
    let scope = account_scope(db)?
        .ok_or_else(|| AppError::InvalidArgument("No signed-in GitHub account".into()))?;
    db.connect()?.execute("INSERT INTO kanban_preferences(account_scope,kind,repository_scope,relationship,search,show_completed,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(account_scope) DO UPDATE SET kind=excluded.kind,repository_scope=excluded.repository_scope,relationship=excluded.relationship,search=excluded.search,show_completed=excluded.show_completed,updated_at=excluded.updated_at",params![scope,value.kind,value.repository_scope,value.relationship,value.search,value.show_completed,Utc::now().to_rfc3339()])?;
    Ok(value)
}
