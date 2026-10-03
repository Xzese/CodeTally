//! Persistent GitHub request budget and resumable, non-search activity imports.
use crate::{db::Database, error::{AppError, AppResult}, github, models::{Repository, PullRequest, Issue}};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Command;

pub const PAUSE_KEY: &str = "github_pause_until";

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
    ensure_available(db)?;
    let mut args = vec!["api".to_string(), "graphql".into(), "--include".into(), "-f".into(), format!("query={query}")];
    for (key, value) in variables.as_object().expect("object variables") {
        args.extend([if value.is_string() { "-f" } else { "-F" }.into(), format!("{key}={}", value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()))]);
    }
    let output = Command::new(github::command_path("gh")).args(args).output()?;
    parse_response(db, output.status.success(), &String::from_utf8_lossy(&output.stdout), &String::from_utf8_lossy(&output.stderr))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since: Option<String>,
}

const PR_FIELDS: &str = "number title state isDraft createdAt updatedAt mergedAt closedAt url additions deletions changedFiles commits(last:1){nodes{commit{statusCheckRollup{state}}}}";
const ISSUE_FIELDS: &str = "number title state createdAt updatedAt closedAt url author{login} labels(first:100){nodes{name}} assignees(first:100){nodes{login}}";

struct Feed {
    alias: &'static str,
    kind: usize,
    open: bool,
    key: String,
    cursor: Cursor,
    started: String,
    cutoff: DateTime<Utc>,
    done: bool,
}

fn activity_query(repo: &Repository, feeds: &[Feed], requested: &[usize]) -> (String, Value) {
    let mut variables = json!({"owner": repo.owner, "name": repo.name});
    let mut definitions = String::new();
    let mut selections = String::new();
    for &index in requested {
        let feed = &feeds[index];
        let variable = format!("{}After", feed.alias);
        definitions.push_str(&format!(",${variable}:String"));
        variables[&variable] = json!(feed.cursor.after);
        let states = if feed.open { "[OPEN]" } else if feed.kind == 0 { "[CLOSED,MERGED]" } else { "[CLOSED]" };
        let filter = if let Some(since) = &feed.cursor.since {
            let since_variable = format!("{}Since", feed.alias);
            definitions.push_str(&format!(",${since_variable}:DateTime"));
            variables[&since_variable] = json!(since);
            format!(",filterBy:{{since:${since_variable}}}")
        } else { String::new() };
        let connection = if feed.kind == 0 { "pullRequests" } else { "issues" };
        let fields = if feed.kind == 0 { PR_FIELDS } else { ISSUE_FIELDS };
        selections.push_str(&format!(" {}:{connection}(first:100,after:${variable},states:{states}{filter},orderBy:{{field:UPDATED_AT,direction:DESC}}){{nodes{{{fields}}} pageInfo{{hasNextPage endCursor}}}}", feed.alias));
    }
    let query = format!("query($owner:String!,$name:String!{definitions}){{rateLimit{{remaining resetAt}} repository(owner:$owner,name:$name){{openPRs:pullRequests(states:OPEN){{totalCount}} openIssues:issues(states:OPEN){{totalCount}}{selections}}}}}");
    (query, variables)
}

fn cursor_error(error: &AppError) -> bool {
    !matches!(error, AppError::RateLimited { .. }) && error.to_string().to_ascii_lowercase().contains("cursor")
}

fn reset_cursor(db: &Database, feed: &mut Feed) -> AppResult<()> {
    feed.cursor.after = None;
    // Retain the original overlap watermark and cycle start on recovery.
    db.set_metadata(&feed.key, &serde_json::to_string(&feed.cursor)?)
}

fn recover_invalid_cursors(db: &Database, repo: &Repository, feeds: &mut [Feed], requested: &[usize], error: &AppError) -> AppResult<()> {
    if !cursor_error(error) { return Ok(()); }
    let resumed: Vec<_> = requested.iter().copied().filter(|&index| feeds[index].cursor.after.is_some()).collect();
    let errors = match error {
        AppError::Command { message, .. } => serde_json::from_str::<Vec<Value>>(message).ok(),
        _ => None,
    };
    // GraphQL field errors identify the failing alias in their response path.
    let identified: Vec<_> = resumed.iter().copied().filter(|&index| errors.as_ref().is_some_and(|errors| errors.iter().any(|error| {
        error["message"].as_str().is_some_and(|message| message.to_ascii_lowercase().contains("cursor")) &&
        error["path"].as_array().is_some_and(|path| path.iter().any(|part| part.as_str() == Some(feeds[index].alias)))
    }))).collect();
    if !identified.is_empty() {
        for index in identified { reset_cursor(db, &mut feeds[index])?; }
    } else if resumed.len() == 1 {
        reset_cursor(db, &mut feeds[resumed[0]])?;
    } else {
        // Some servers omit error paths. Diagnose only resumed feeds, bounded
        // to four requests, rather than discard healthy pagination checkpoints.
        for index in resumed {
            let (query, variables) = activity_query(repo, feeds, &[index]);
            match graphql(db, &query, variables) {
                Err(error) if cursor_error(&error) => reset_cursor(db, &mut feeds[index])?,
                Err(error @ AppError::RateLimited { .. }) => return Err(error),
                _ => (),
            }
        }
    }
    Ok(())
}

fn publish_feed_page(db: &Database, repo: &Repository, feed: &mut Feed, repository: &Value, counts: &mut [i64; 2]) -> AppResult<()> {
    let page = &repository[feed.alias];
    let nodes = page["nodes"].as_array().ok_or_else(|| AppError::InvalidArgument("GitHub response missing activity nodes".into()))?;
    let pr_total = repository["openPRs"]["totalCount"].as_i64().ok_or_else(|| AppError::InvalidArgument("GitHub response missing PR count".into()))?;
    let issue_total = repository["openIssues"]["totalCount"].as_i64().ok_or_else(|| AppError::InvalidArgument("GitHub response missing issue count".into()))?;
    let mut reached_cutoff = false;
    let mut prs = Vec::new();
    let mut issues = Vec::new();
    for node in nodes {
        let updated = node["updatedAt"].as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok()).ok_or_else(|| AppError::InvalidArgument("GitHub activity missing updatedAt".into()))?;
        if !feed.open && updated < feed.cutoff { reached_cutoff = true; continue; }
        let mut node = node.clone();
        if feed.kind == 0 {
            let ci = node.pointer("/commits/nodes/0/commit/statusCheckRollup/state").and_then(Value::as_str).map(|s| match s { "ERROR" | "FAILURE" => "failure".into(), "EXPECTED" | "PENDING" => "pending".into(), _ => s.to_ascii_lowercase() });
            let v: crate::models::GithubPullRequestJson = serde_json::from_value(node)?;
            prs.push(PullRequest { repository_id:repo.id, repository:repo.name_with_owner.clone(), number:v.number,title:v.title,state:v.state,is_draft:v.is_draft,created_at:v.created_at,updated_at:v.updated_at,merged_at:v.merged_at,closed_at:v.closed_at,url:v.url,additions:v.additions.unwrap_or(0),deletions:v.deletions.unwrap_or(0),changed_files:v.changed_files.unwrap_or(0),ci_state:ci });
        } else {
            node["labels"] = node["labels"]["nodes"].clone();
            node["assignees"] = node["assignees"]["nodes"].clone();
            let v: crate::models::GithubIssueJson = serde_json::from_value(node)?;
            issues.push(Issue {repository_id:repo.id,repository:repo.name_with_owner.clone(),number:v.number,title:v.title,state:v.state,created_at:v.created_at,updated_at:v.updated_at,closed_at:v.closed_at,url:v.url,author:v.author.map(|a|a.login),labels:v.labels.into_iter().map(|a|a.name).collect(),assignees:v.assignees.into_iter().map(|a|a.login).collect()});
        }
    }
    let more = page["pageInfo"]["hasNextPage"].as_bool().ok_or_else(|| AppError::InvalidArgument("GitHub response missing pageInfo".into()))?;
    let done = !more || reached_cutoff;
    let next_cursor = if done {
        Cursor { completed_at:Some(feed.started.clone()), ..Cursor::default() }
    } else {
        let after = page["pageInfo"]["endCursor"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| AppError::InvalidArgument("GitHub response missing endCursor".into()))?;
        if feed.cursor.after.as_deref() == Some(after) { return Err(AppError::InvalidArgument("GitHub pagination cursor did not advance".into())); }
        Cursor { after:Some(after.into()), started_at:Some(feed.started.clone()), completed_at:feed.cursor.completed_at.clone(), since:feed.cursor.since.clone() }
    };
    db.apply_activity_page(&prs, &issues, repo.id, pr_total, issue_total, &feed.key, &serde_json::to_string(&next_cursor)?)?;
    counts[feed.kind] += (prs.len() + issues.len()) as i64;
    feed.cursor = next_cursor;
    feed.done = done;
    Ok(())
}

/// At most five pages per feed per cycle. Pending PR/issue feeds share one
/// bounded request per round, each with its own cursor and durable checkpoint.
/// Open feeds are fully walked so CI-only changes and old open items stay visible.
pub fn sync_activity(db: &Database, repo: &Repository) -> AppResult<(i64, i64, bool)> {
    sync_activity_reporting(db, repo, &mut [0, 0])
}

pub(crate) fn sync_activity_reporting(db: &Database, repo: &Repository, counts: &mut [i64; 2]) -> AppResult<(i64, i64, bool)> {
    let cycle_started = Utc::now().to_rfc3339();
    let mut feeds = Vec::with_capacity(4);
    for (alias, kind, open) in [("prOpen", 0, true), ("issueOpen", 1, true), ("prClosed", 0, false), ("issueClosed", 1, false)] {
        let connection = if kind == 0 { "pullRequests" } else { "issues" };
        let key = format!("github_activity_v2:{}:{connection}:{open}", repo.id);
        let mut cursor: Cursor = db.metadata(&key)?.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default();
        let started = cursor.started_at.clone().unwrap_or_else(|| cycle_started.clone());
        let cutoff = cursor.completed_at.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok()).map(|t| t - Duration::minutes(5)).unwrap_or_else(|| started.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()) - Duration::days(30));
        // Server-side issue filtering avoids retransmitting older closed rows.
        // Legacy cursors were created without this filter, so finish them with
        // the original query. New filtered cursors persist the exact bound.
        if kind == 1 && !open && cursor.after.is_none() && cursor.since.is_none() {
            cursor.since = Some(cutoff.to_rfc3339());
        }
        feeds.push(Feed { alias, kind, open, key, cursor, started, cutoff, done: false });
    }
    for _ in 0..5 {
        let requested: Vec<_> = feeds.iter().enumerate().filter_map(|(index, feed)| (!feed.done).then_some(index)).collect();
        if requested.is_empty() { break; }
        let (query, variables) = activity_query(repo, &feeds, &requested);
        let response = match graphql(db, &query, variables) {
            Ok(response) => response,
            Err(error) => {
                recover_invalid_cursors(db, repo, &mut feeds, &requested, &error)?;
                return Err(error);
            }
        };
        for index in requested {
            publish_feed_page(db, repo, &mut feeds[index], &response["data"]["repository"], counts)?;
        }
    }
    Ok((counts[0], counts[1], feeds.iter().all(|feed| feed.done)))
}
