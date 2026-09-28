//! Persistent GitHub request budget and resumable, non-search activity imports.
use crate::{db::Database, error::{AppError, AppResult}, github, models::{Repository, PullRequest, Issue}};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Command;
use std::time::Duration as StdDuration;

pub const PAUSE_KEY: &str = "github_pause_until";
// Four total attempts with 250 ms, 500 ms, and 1 s waits between retries.
const GRAPHQL_TRANSIENT_RETRIES: u32 = 3;
const GRAPHQL_RETRY_BASE_MILLIS: u64 = 250;
const GRAPHQL_RETRY_MAX_MILLIS: u64 = 2_000;

pub fn ensure_available(db: &Database) -> AppResult<()> {
    if let Some(until) = db.metadata(PAUSE_KEY)? {
        if until.parse::<DateTime<Utc>>().map(|t| t > Utc::now()).unwrap_or(false) {
            return Err(AppError::RateLimited { until, reason: "GitHub API rate limit".into() });
        }
    }
    Ok(())
}

fn pause(db: &Database, until: DateTime<Utc>, reason: &str) -> AppResult<AppError> {
    let old = db.metadata(PAUSE_KEY)?.and_then(|s| s.parse::<DateTime<Utc>>().ok());
    let until = old.map(|old| old.max(until)).unwrap_or(until).to_rfc3339();
    db.set_metadata(PAUSE_KEY, &until)?;
    Ok(AppError::RateLimited { until, reason: reason.into() })
}

pub fn guarded<T>(db: &Database, call: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
    ensure_available(db)?;
    match call() {
        Err(error) if limited(&error.to_string()) => {
            let message = error.to_string().to_ascii_lowercase();
            let secondary = message.contains("secondary") || message.contains("abuse") || message.contains("429");
            let until = if secondary {
                let strikes = db.metadata("github_secondary_strikes")?.and_then(|s| s.parse::<u32>().ok()).unwrap_or(0).min(6);
                db.set_metadata("github_secondary_strikes", &(strikes + 1).to_string())?;
                Utc::now() + Duration::seconds((60 * 2_i64.pow(strikes)).min(3600))
            } else {
                db.metadata("github_quota_reset")?.and_then(|s| s.parse::<DateTime<Utc>>().ok()).filter(|t| *t > Utc::now()).unwrap_or_else(|| Utc::now() + Duration::hours(1))
            };
            Err(pause(db, until + Duration::seconds(5), &error.to_string())?)
        },
        other => other,
    }
}

fn limited(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limit") || message.contains("rate_limit") || message.contains("secondary") || message.contains("abuse") || message.contains("http 429")
}

pub fn graphql(db: &Database, query: &str, variables: Value) -> AppResult<Value> {
    let mut args = vec!["api".to_string(), "graphql".into(), "--include".into(), "-f".into(), format!("query={query}")];
    for (key, value) in variables.as_object().expect("object variables") {
        args.extend([if value.is_string() { "-f" } else { "-F" }.into(), format!("{key}={}", value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()))]);
    }
    let mut retries = 0;
    loop {
        ensure_available(db)?;
        let output = Command::new(github::command_path("gh")).args(&args).output()?;
        let success = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        match parse_response(db, success, &stdout, &stderr) {
            Ok(value) => return Ok(value),
            Err(error) if retries < GRAPHQL_TRANSIENT_RETRIES && transient_network_failure(&error, &stdout, &stderr) => {
                std::thread::sleep(graphql_retry_delay(retries));
                retries += 1;
            },
            Err(error) => return Err(error),
        }
    }
}

fn transient_network_failure(error: &AppError, stdout: &str, stderr: &str) -> bool {
    if matches!(error, AppError::RateLimited { .. }) {
        return false;
    }
    let message = format!("{stdout}\n{stderr}\n{error}").to_ascii_lowercase();
    if limited(&message) {
        return false;
    }
    message.contains("reset by peer") || message.contains("operation timed out") || message.contains("i/o timeout")
}

fn graphql_retry_delay(retry: u32) -> StdDuration {
    let multiplier = 1_u64 << retry.min(3);
    StdDuration::from_millis((GRAPHQL_RETRY_BASE_MILLIS * multiplier).min(GRAPHQL_RETRY_MAX_MILLIS))
}

pub fn parse_response(db: &Database, success: bool, stdout: &str, stderr: &str) -> AppResult<Value> {
    let normalized = stdout.replace("\r\n", "\n");
    let (headers, body) = if normalized.starts_with("HTTP/") { normalized.split_once("\n\n").unwrap_or(("", &normalized)) } else { ("", normalized.as_str()) };
    let header = |name: &str| headers.lines().filter_map(|line| line.split_once(':')).find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim());
    let value = serde_json::from_str::<Value>(body);
    let message = format!("{} {} {}", if success { "" } else { stderr }, value.as_ref().ok().and_then(|v| v.get("errors")).map(Value::to_string).unwrap_or_default(), if success { String::new() } else { value.as_ref().ok().and_then(|v| v["message"].as_str()).unwrap_or("").to_string() });
    let rate = value.as_ref().ok().and_then(|v| v.pointer("/data/rateLimit"));
    let remaining = rate.and_then(|v| v["remaining"].as_i64()).or_else(|| header("x-ratelimit-remaining").and_then(|s| s.parse::<i64>().ok()));
    let exhausted = remaining.is_some_and(|remaining| remaining <= 100);
    if let Some(remaining) = remaining { db.set_metadata("github_quota_remaining", &remaining.to_string())?; }
    if let Some(cost) = rate.and_then(|v| v["cost"].as_i64()) { db.set_metadata("github_last_query_cost", &cost.to_string())?; }
    if let Some(reset) = rate.and_then(|v| v["resetAt"].as_str()) { db.set_metadata("github_quota_reset", reset)?; }
    if exhausted || limited(&message) || headers.lines().next().unwrap_or("").contains("429") {
        let reset = header("x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok()).and_then(|t| DateTime::from_timestamp(t, 0))
            .or_else(|| rate.and_then(|v| v["resetAt"].as_str()).and_then(|s| s.parse::<DateTime<Utc>>().ok()));
        let retry = header("retry-after").and_then(|v| v.parse::<i64>().ok()).map(|seconds| Utc::now() + Duration::seconds(seconds.max(60)));
        let strikes = db.metadata("github_secondary_strikes")?.and_then(|s| s.parse::<u32>().ok()).unwrap_or(0).min(6);
        if !exhausted { db.set_metadata("github_secondary_strikes", &(strikes + 1).to_string())?; }
        let backoff = Utc::now() + Duration::seconds((60 * 2_i64.pow(strikes)).min(3600));
        let until = if exhausted { reset.unwrap_or_else(|| Utc::now() + Duration::hours(1)).max(retry.unwrap_or_else(Utc::now)) } else { backoff.max(retry.unwrap_or_else(Utc::now)) };
        let error = pause(db, until + Duration::seconds(5), "GitHub API rate limit")?;
        // A successful final response may still be safely committed. The next request stops.
        if !success || value.as_ref().ok().and_then(|v| v.get("errors")).is_some() { return Err(error); }
    }
    // Only a missing repository is skippable; authentication, permission errors
    // on child fields, and other API failures must still be reported.
    let response_errors = value.as_ref().ok().and_then(|v| v.get("errors")).and_then(Value::as_array);
    let missing_repository = response_errors.is_some_and(|errors| !errors.is_empty() && errors.iter().all(|error| {
        error["type"] == "NOT_FOUND" && error["path"] == json!(["repository"])
    }));
    if missing_repository || (response_errors.is_none() && !success && stderr.contains("Could not resolve to a Repository with the name '")) {
        return Err(AppError::RepositoryUnavailable);
    }
    if !success { return Err(AppError::Command { program: "gh api graphql".into(), message: stderr.trim().to_owned() }); }
    let value = value?;
    if let Some(errors) = value.get("errors") { return Err(AppError::Command { program: "gh api graphql".into(), message: errors.to_string() }); }
    if value.get("data").map_or(true, Value::is_null) { return Err(AppError::InvalidArgument("GitHub response missing data".into())); }
    Ok(value)
}

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    completed_at: Option<String>,
    started_at: Option<String>,
    after: Option<String>,
}

/// At most five pages per feed per cycle. Each page is durable before its cursor
/// advances; failures replay safely through upserts. Open feeds are fully walked
/// every pass so CI-only changes and old open items remain visible.
pub fn sync_activity(db: &Database, repo: &Repository) -> AppResult<(i64, i64, bool)> {
    sync_activity_reporting(db, repo, &mut [0, 0])
}

/// Counts advance after each durable upsert, including when a later request fails.
pub(crate) fn sync_activity_reporting(db: &Database, repo: &Repository, counts: &mut [i64; 2]) -> AppResult<(i64, i64, bool)> {
    let mut complete = true;
    let cycle_started = Utc::now().to_rfc3339();
    let board_enabled = crate::sync::app_settings(db)?.kanban_enabled;
    for open in [true, false] {
        let mut sibling_page = None;
        for (index, feed) in ["pullRequests", "issues"].iter().enumerate() {
            let key = format!("github_activity_v2:{}:{feed}:{open}", repo.id);
            let mut cursor: Cursor = db.metadata(&key)?.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default();
            let started = cursor.started_at.clone().unwrap_or_else(|| cycle_started.clone());
            let cutoff = cursor.completed_at.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok()).map(|t| t - Duration::minutes(5)).unwrap_or_else(|| started.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()) - Duration::days(30));
            let fields = if index == 0 { format!("{}number title state isDraft createdAt updatedAt mergedAt closedAt url author{{login}} assignees(first:100){{nodes{{login}}}} additions deletions changedFiles commits(last:1){{nodes{{commit{{statusCheckRollup{{state}}}}}}}}", if board_enabled { "id " } else { "" }) } else { format!("{}number title state createdAt updatedAt closedAt url author{{login}} labels(first:100){{nodes{{name}}}} assignees(first:100){{nodes{{login}}}}", if board_enabled { "id stateReason " } else { "" }) };
            let states = if open { "[OPEN]" } else if index == 0 { "[CLOSED,MERGED]" } else { "[CLOSED]" };
            for page in 0..5 {
                let sibling = if index == 0 && page == 0 {
                    let states = if open { "[OPEN]" } else { "[CLOSED]" };
                    format!("sibling:issues(first:100,states:{states},orderBy:{{field:UPDATED_AT,direction:DESC}}){{nodes{{{}{}}} pageInfo{{hasNextPage endCursor}}}}", if board_enabled { "id stateReason " } else { "" }, "number title state createdAt updatedAt closedAt url author{login} labels(first:100){nodes{name}} assignees(first:100){nodes{login}}")
                } else { String::new() };
                let query = format!("query($owner:String!,$name:String!,$after:String){{rateLimit{{cost remaining resetAt}} repository(owner:$owner,name:$name){{openPRs:pullRequests(states:OPEN){{totalCount}} openIssues:issues(states:OPEN){{totalCount}} {sibling} items:{feed}(first:100,after:$after,states:{states},orderBy:{{field:UPDATED_AT,direction:DESC}}){{nodes{{{fields}}} pageInfo{{hasNextPage endCursor}}}}}}}}");
                let response = if index == 1 && page == 0 && cursor.after.is_none() && sibling_page.is_some() {
                    sibling_page.take().expect("checked sibling page")
                } else {
                    match graphql(db, &query, json!({"owner":repo.owner,"name":repo.name,"after":cursor.after})) {
                        Ok(response) => response,
                        Err(error) => {
                            if cursor.after.is_some() && error.to_string().to_ascii_lowercase().contains("cursor") && !matches!(error, AppError::RateLimited { .. }) {
                                cursor.after = None;
                                db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
                            }
                            return Err(error);
                        }
                    }
                };
                if index == 0 && page == 0 {
                    let mut sibling = response.clone();
                    sibling["data"]["repository"]["items"] = sibling["data"]["repository"]["sibling"].clone();
                    sibling_page = Some(sibling);
                }
                let repository = &response["data"]["repository"];
                let nodes = repository["items"]["nodes"].as_array().ok_or_else(|| AppError::InvalidArgument("GitHub response missing activity nodes".into()))?;
                let pr_total = repository["openPRs"]["totalCount"].as_i64().ok_or_else(|| AppError::InvalidArgument("GitHub response missing PR count".into()))?;
                let issue_total = repository["openIssues"]["totalCount"].as_i64().ok_or_else(|| AppError::InvalidArgument("GitHub response missing issue count".into()))?;
                let mut reached_cutoff = false;
                for node in nodes {
                    let updated = node["updatedAt"].as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok()).ok_or_else(|| AppError::InvalidArgument("GitHub activity missing updatedAt".into()))?;
                    if !open && updated < cutoff { reached_cutoff = true; continue; }
                    let mut node = node.clone();
                    let node_id = node["id"].as_str().map(str::to_owned);
                    let completion_reason = node["stateReason"].as_str().map(str::to_owned);
                    if index == 0 {
                        let ci = node.pointer("/commits/nodes/0/commit/statusCheckRollup/state").and_then(Value::as_str).map(|s| match s { "ERROR" | "FAILURE" => "failure".into(), "EXPECTED" | "PENDING" => "pending".into(), _ => s.to_ascii_lowercase() });
                        if node.get("assignees").and_then(|value| value.get("nodes")).is_none_or(Value::is_null) {
                            node["assignees"] = json!([]);
                        } else {
                            node["assignees"] = node["assignees"]["nodes"].clone();
                        }
                        let v: crate::models::GithubPullRequestJson = serde_json::from_value(node)?;
                        db.upsert_pull_request(&PullRequest { repository_id:repo.id, repository:repo.name_with_owner.clone(), number:v.number,title:v.title,state:v.state,is_draft:v.is_draft,created_at:v.created_at,updated_at:v.updated_at,merged_at:v.merged_at,closed_at:v.closed_at,url:v.url,author:v.author.map(|a| a.login),assignees:v.assignees.into_iter().map(|a| a.login).collect(),additions:v.additions.unwrap_or(0),deletions:v.deletions.unwrap_or(0),changed_files:v.changed_files.unwrap_or(0),ci_state:ci })?;
                        if board_enabled && node_id.is_some() { crate::kanban::store_identity(db,repo.id,&repo.github_id,"pr",v.number,node_id.as_deref(),None)?; }
                    } else {
                        node["labels"] = node["labels"]["nodes"].clone();
                        node["assignees"] = node["assignees"]["nodes"].clone();
                        let v: crate::models::GithubIssueJson = serde_json::from_value(node)?;
                        db.upsert_issue(&Issue {repository_id:repo.id,repository:repo.name_with_owner.clone(),number:v.number,title:v.title,state:v.state,created_at:v.created_at,updated_at:v.updated_at,closed_at:v.closed_at,url:v.url,author:v.author.map(|a|a.login),labels:v.labels.into_iter().map(|a|a.name).collect(),assignees:v.assignees.into_iter().map(|a|a.login).collect()})?;
                        if board_enabled && node_id.is_some() { crate::kanban::store_identity(db,repo.id,&repo.github_id,"issue",v.number,node_id.as_deref(),completion_reason.as_deref())?; }
                    }
                    counts[index] += 1;
                }
                db.set_open_counts(repo.id, pr_total, issue_total)?;
                let more = repository["items"]["pageInfo"]["hasNextPage"].as_bool().ok_or_else(|| AppError::InvalidArgument("GitHub response missing pageInfo".into()))?;
                if !more || reached_cutoff {
                    cursor = Cursor { completed_at:Some(started.clone()), ..Cursor::default() };
                    db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
                    break;
                }
                let after = repository["items"]["pageInfo"]["endCursor"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| AppError::InvalidArgument("GitHub response missing endCursor".into()))?;
                if cursor.after.as_deref() == Some(after) { return Err(AppError::InvalidArgument("GitHub pagination cursor did not advance".into())); }
                cursor.after = Some(after.into());
                cursor.started_at = Some(started.clone());
                db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
                if page == 4 { complete = false; }
            }
        }
    }
    Ok((counts[0], counts[1], complete))
}

#[derive(Debug, Default)]
pub struct PersonalSyncReport {
    pub pull_requests: i64,
    pub issues: i64,
    pub complete: bool,
}

#[derive(Serialize, Deserialize)]
struct PersonalSearchWindow {
    // Inclusive, whole-second bounds match GitHub's documented search precision.
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

#[derive(Default, Serialize, Deserialize)]
struct PersonalCursor {
    #[serde(flatten)]
    progress: Cursor,
    // A stack: the oldest remaining window is last and owns progress.after.
    #[serde(default)]
    windows: Vec<PersonalSearchWindow>,
    #[serde(default)]
    saturated: bool,
}

/// Search is an additive discovery pass: repository feeds remain authoritative
/// for open totals. Each stream checkpoints only after its page is durable.
pub fn sync_personal_activity(db: &Database, login: &str, selected: &[Repository]) -> AppResult<PersonalSyncReport> {
    use std::collections::{HashMap, HashSet};
    let login = login.trim().to_ascii_lowercase();
    if login.is_empty() || !login.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err(AppError::InvalidArgument("Invalid GitHub login for personal search".into()));
    }
    let repositories: HashMap<_, _> = selected.iter().filter(|r| !r.github_id.is_empty()).map(|r| (r.github_id.as_str(), r)).collect();
    let mut report = PersonalSyncReport { complete: true, ..PersonalSyncReport::default() };
    if repositories.is_empty() { return Ok(report); }
    // Selection changes restart the window; otherwise newly selected repositories
    // could lose matches behind the previous selection's checkpoint.
    let mut scope: Vec<_> = repositories.keys().copied().collect();
    scope.sort_unstable();
    let scope = serde_json::to_string(&scope)?;
    let board_enabled = crate::sync::app_settings(db)?.kanban_enabled;
    let cycle_started = Utc::now().to_rfc3339();
    let mut seen = HashSet::new();
    reconcile_personal_assignments(db, &login, selected, &scope, board_enabled, &mut report, &mut seen)?;
    let mut saturated_streams = Vec::new();
    for role in ["author", "assignee"] {
        for kind in ["pr", "issue"] {
            let key = format!("github_personal_v1:github.com:{login}:{scope}:{role}:{kind}");
            let mut cursor: PersonalCursor = db.metadata(&key)?.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default();
            if cursor.saturated {
                saturated_streams.push(format!("{role}:{login} is:{kind}"));
                report.complete = false;
                continue;
            }
            let started = cursor.progress.started_at.clone().unwrap_or_else(|| cycle_started.clone());
            if cursor.windows.is_empty() {
                let end = started.parse::<DateTime<Utc>>().map_err(|_| AppError::InvalidArgument("Invalid personal search start time".into()))?;
                let cutoff = cursor.progress.completed_at.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok())
                    .map(|t| t - Duration::minutes(5)).unwrap_or_else(|| end - Duration::days(30));
                // Old cursors had no upper bound. Replay their window rather than
                // reuse an opaque cursor against a changed search expression.
                cursor.progress.after = None;
                cursor.progress.started_at = Some(started.clone());
                cursor.windows.push(PersonalSearchWindow {
                    start: DateTime::from_timestamp(cutoff.timestamp(), 0).expect("valid cutoff"),
                    end: DateTime::from_timestamp(end.timestamp(), 0).expect("valid start time"),
                });
                db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
            }
            let fields = personal_item_fields(kind);
            let query = format!("query($search:String!,$after:String){{rateLimit{{cost remaining resetAt}} search(type:ISSUE,query:$search,first:100,after:$after){{issueCount nodes{{{fields}}} pageInfo{{hasNextPage endCursor}}}}}}");
            // Split probes consume this budget too, so an oversized result set
            // cannot turn one sync into an unbounded number of API requests.
            for _ in 0..5 {
                let window = cursor.windows.last().expect("pending search window");
                let search = format!("is:{kind} {role}:{login} updated:{}..{} sort:updated-asc", window.start.format("%Y-%m-%dT%H:%M:%SZ"), window.end.format("%Y-%m-%dT%H:%M:%SZ"));
                let response = match graphql(db, &query, json!({"search":search,"after":cursor.progress.after})) {
                    Ok(response) => response,
                    Err(error) => {
                        if cursor.progress.after.is_some() && !matches!(error, AppError::RateLimited { .. }) && error.to_string().to_ascii_lowercase().contains("cursor") {
                            cursor.progress.after = None;
                            db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
                        }
                        return Err(error);
                    }
                };
                let result = &response["data"]["search"];
                let total = result["issueCount"].as_u64().ok_or_else(|| AppError::InvalidArgument("GitHub response missing personal search count".into()))?;
                if total > 1000 {
                    let window = cursor.windows.pop().expect("pending search window");
                    cursor.progress.after = None;
                    let span = (window.end - window.start).num_seconds();
                    if span <= 0 {
                        cursor.windows.push(window);
                        cursor.saturated = true;
                        db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
                        saturated_streams.push(format!("{role}:{login} is:{kind}"));
                        report.complete = false;
                        break;
                    }
                    let middle = window.start + Duration::seconds(span / 2);
                    cursor.windows.push(PersonalSearchWindow { start: middle + Duration::seconds(1), end: window.end });
                    cursor.windows.push(PersonalSearchWindow { start: window.start, end: middle });
                    db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
                    continue;
                }
                let nodes = result["nodes"].as_array().ok_or_else(|| AppError::InvalidArgument("GitHub response missing personal search nodes".into()))?;
                let more = result["pageInfo"]["hasNextPage"].as_bool().ok_or_else(|| AppError::InvalidArgument("GitHub response missing personal search pageInfo".into()))?;
                for node in nodes {
                    let Some(repo) = node["repository"]["id"].as_str().and_then(|id| repositories.get(id)) else { continue; };
                    let number = node["number"].as_i64().ok_or_else(|| AppError::InvalidArgument("GitHub search item missing number".into()))?;
                    let identity = node["id"].as_str().map(str::to_owned).unwrap_or_else(|| format!("{}:{kind}:{number}", repo.github_id));
                    store_personal_item(db, repo, kind, node.clone(), board_enabled)?;
                    if seen.insert(identity) {
                        if kind == "pr" { report.pull_requests += 1; } else { report.issues += 1; }
                    }
                }
                if !more {
                    cursor.windows.pop();
                    cursor.progress.after = None;
                    if cursor.windows.is_empty() {
                        cursor.progress = Cursor { completed_at: Some(started.clone()), ..Cursor::default() };
                    }
                    db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
                    if cursor.windows.is_empty() { break; }
                    continue;
                }
                let after = result["pageInfo"]["endCursor"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| AppError::InvalidArgument("GitHub search missing endCursor".into()))?;
                if cursor.progress.after.as_deref() == Some(after) { return Err(AppError::InvalidArgument("GitHub search cursor did not advance".into())); }
                cursor.progress.after = Some(after.into());
                db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
            }
            if !cursor.windows.is_empty() { report.complete = false; }
        }
    }
    if !saturated_streams.is_empty() {
        return Err(AppError::InvalidArgument(format!("Personal GitHub search exceeds 1,000 results in a single second for {}; blocked checkpoints are retained and automatic retries are stopped for those streams; other streams were refreshed", saturated_streams.join(", "))));
    }
    Ok(report)
}

fn personal_item_fields(kind: &str) -> &'static str {
    if kind == "pr" {
        "... on PullRequest { id repository{id} number title state isDraft createdAt updatedAt mergedAt closedAt url author{login} assignees(first:100){nodes{login}} additions deletions changedFiles commits(last:1){nodes{commit{statusCheckRollup{state}}}} }"
    } else {
        "... on Issue { id repository{id} number title state stateReason createdAt updatedAt closedAt url author{login} labels(first:100){nodes{name}} assignees(first:100){nodes{login}} }"
    }
}

#[derive(Default, Serialize, Deserialize)]
struct AssignmentCursor {
    after: Option<(i64, String, i64)>,
    // Sticky for the entire sweep, including later batches that all succeed.
    #[serde(default)]
    unresolved: bool,
}

/// Search stops returning assigned-only work after unassignment. Read a bounded
/// slice of the existing cache directly so those old relationships can be removed.
/// The stable key cursor makes every candidate reachable over successive cycles,
/// even when newer assignments keep arriving. No missing/null node erases cache.
fn reconcile_personal_assignments(
    db: &Database,
    login: &str,
    selected: &[Repository],
    scope: &str,
    board_enabled: bool,
    report: &mut PersonalSyncReport,
    seen: &mut std::collections::HashSet<String>,
) -> AppResult<()> {
    const BATCH_SIZE: usize = 50;
    let key = format!("github_personal_assignments_v1:github.com:{login}:{scope}");
    let saved: Value = db.metadata(&key)?.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or(Value::Null);
    let mut cursor: AssignmentCursor = if saved.is_object() {
        serde_json::from_value(saved)?
    } else {
        // Older versions stored only the key tuple (or null between sweeps).
        AssignmentCursor { after: serde_json::from_value(saved)?, ..AssignmentCursor::default() }
    };
    // A fresh sweep retries previously unresolved rows. Clear the flag locally;
    // durable state changes only after this batch has been processed successfully.
    if cursor.after.is_none() { cursor.unresolved = false; }
    let after = &cursor.after;
    let repository_ids: Vec<_> = selected.iter().map(|r| r.id).collect();
    let candidates: Vec<(i64, String, i64)> = {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT repository_id,kind,number FROM (
                SELECT repository_id,'pr' kind,number,author,assignees_json FROM pull_requests
                UNION ALL SELECT repository_id,'issue' kind,number,author,assignees_json FROM issues
             ) WHERE repository_id IN (SELECT value FROM json_each(?1))
               AND lower(coalesce(author,''))<>?2
               AND EXISTS (SELECT 1 FROM json_each(assignees_json) WHERE lower(value)=?2)
               AND (?3 IS NULL OR (repository_id,kind,number)>(?3,?4,?5))
             ORDER BY repository_id,kind,number LIMIT ?6"
        )?;
        let rows = stmt.query_map(rusqlite::params![serde_json::to_string(&repository_ids)?, login,
            after.as_ref().map(|a| a.0), after.as_ref().map(|a| a.1.as_str()), after.as_ref().map(|a| a.2), BATCH_SIZE as i64 + 1],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let more = candidates.len() > BATCH_SIZE;
    let candidates = &candidates[..candidates.len().min(BATCH_SIZE)];
    if candidates.is_empty() {
        cursor.after = None;
        if cursor.unresolved { report.complete = false; }
        db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
        return Ok(());
    }
    let mut definitions = Vec::new();
    let mut selections = Vec::new();
    let mut variables = serde_json::Map::new();
    for (index, (repository_id, kind, number)) in candidates.iter().enumerate() {
        let repo = selected.iter().find(|repo| repo.id == *repository_id).expect("selected repository");
        definitions.push(format!("$repo{index}:ID!,$number{index}:Int!"));
        let field = if kind == "pr" { "pullRequest" } else { "issue" };
        let fields = personal_item_fields(kind);
        selections.push(format!("r{index}:node(id:$repo{index}){{... on Repository{{item:{field}(number:$number{index}){{{fields}}}}}}}"));
        variables.insert(format!("repo{index}"), json!(repo.github_id));
        variables.insert(format!("number{index}"), json!(number));
    }
    let query = format!("query({}){{rateLimit{{cost remaining resetAt}} {}}}", definitions.join(","), selections.join(" "));
    let response = graphql(db, &query, Value::Object(variables))?;
    for (index, (repository_id, kind, number)) in candidates.iter().enumerate() {
        let repo = selected.iter().find(|repo| repo.id == *repository_id).expect("selected repository");
        let node = &response["data"][format!("r{index}")]["item"];
        if node["repository"]["id"].as_str() != Some(repo.github_id.as_str()) || node["number"].as_i64() != Some(*number) {
            cursor.unresolved = true;
            continue;
        }
        store_personal_item(db, repo, kind, node.clone(), board_enabled)?;
        let identity = node["id"].as_str().map(str::to_owned).unwrap_or_else(|| format!("{}:{kind}:{number}", repo.github_id));
        if seen.insert(identity) {
            if kind == "pr" { report.pull_requests += 1; } else { report.issues += 1; }
        }
    }
    cursor.after = if more { candidates.last().cloned() } else { None };
    db.set_metadata(&key, &serde_json::to_string(&cursor)?)?;
    if more || cursor.unresolved { report.complete = false; }
    Ok(())
}

fn store_personal_item(db: &Database, repo: &Repository, kind: &str, mut node: Value, board_enabled: bool) -> AppResult<()> {
    let node_id = node["id"].as_str().map(str::to_owned);
    let completion_reason = node["stateReason"].as_str().map(str::to_owned);
    node["assignees"] = node["assignees"]["nodes"].as_array().cloned().map(Value::Array).unwrap_or_else(|| json!([]));
    let number;
    if kind == "pr" {
        let ci = node.pointer("/commits/nodes/0/commit/statusCheckRollup/state").and_then(Value::as_str).map(|s| match s { "ERROR" | "FAILURE" => "failure".into(), "EXPECTED" | "PENDING" => "pending".into(), _ => s.to_ascii_lowercase() });
        let v: crate::models::GithubPullRequestJson = serde_json::from_value(node)?;
        number = v.number;
        db.upsert_pull_request(&PullRequest { repository_id:repo.id, repository:repo.name_with_owner.clone(), number:v.number,title:v.title,state:v.state,is_draft:v.is_draft,created_at:v.created_at,updated_at:v.updated_at,merged_at:v.merged_at,closed_at:v.closed_at,url:v.url,author:v.author.map(|a| a.login),assignees:v.assignees.into_iter().map(|a| a.login).collect(),additions:v.additions.unwrap_or(0),deletions:v.deletions.unwrap_or(0),changed_files:v.changed_files.unwrap_or(0),ci_state:ci })?;
    } else {
        node["labels"] = node["labels"]["nodes"].as_array().cloned().map(Value::Array).unwrap_or_else(|| json!([]));
        let v: crate::models::GithubIssueJson = serde_json::from_value(node)?;
        number = v.number;
        db.upsert_issue(&Issue {repository_id:repo.id,repository:repo.name_with_owner.clone(),number:v.number,title:v.title,state:v.state,created_at:v.created_at,updated_at:v.updated_at,closed_at:v.closed_at,url:v.url,author:v.author.map(|a|a.login),labels:v.labels.into_iter().map(|a|a.name).collect(),assignees:v.assignees.into_iter().map(|a|a.login).collect()})?;
    }
    if board_enabled && node_id.is_some() { crate::kanban::store_identity(db,repo.id,&repo.github_id,kind,number,node_id.as_deref(),completion_reason.as_deref())?; }
    Ok(())
}
