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
    for open in [true, false] {
        let mut sibling_page = None;
        for (index, feed) in ["pullRequests", "issues"].iter().enumerate() {
            let key = format!("github_activity_v2:{}:{feed}:{open}", repo.id);
            let mut cursor: Cursor = db.metadata(&key)?.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default();
            let started = cursor.started_at.clone().unwrap_or_else(|| cycle_started.clone());
            let cutoff = cursor.completed_at.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok()).map(|t| t - Duration::minutes(5)).unwrap_or_else(|| started.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()) - Duration::days(30));
            let fields = if index == 0 { "number title state isDraft createdAt updatedAt mergedAt closedAt url additions deletions changedFiles commits(last:1){nodes{commit{statusCheckRollup{state}}}}" } else { "number title state createdAt updatedAt closedAt url author{login} labels(first:100){nodes{name}} assignees(first:100){nodes{login}}" };
            let states = if open { "[OPEN]" } else if index == 0 { "[CLOSED,MERGED]" } else { "[CLOSED]" };
            for page in 0..5 {
                let sibling = if index == 0 && page == 0 {
                    let states = if open { "[OPEN]" } else { "[CLOSED]" };
                    format!("sibling:issues(first:100,states:{states},orderBy:{{field:UPDATED_AT,direction:DESC}}){{nodes{{number title state createdAt updatedAt closedAt url author{{login}} labels(first:100){{nodes{{name}}}} assignees(first:100){{nodes{{login}}}}}} pageInfo{{hasNextPage endCursor}}}}")
                } else { String::new() };
                let query = format!("query($owner:String!,$name:String!,$after:String){{rateLimit{{remaining resetAt}} repository(owner:$owner,name:$name){{openPRs:pullRequests(states:OPEN){{totalCount}} openIssues:issues(states:OPEN){{totalCount}} {sibling} items:{feed}(first:100,after:$after,states:{states},orderBy:{{field:UPDATED_AT,direction:DESC}}){{nodes{{{fields}}} pageInfo{{hasNextPage endCursor}}}}}}}}");
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
                    if index == 0 {
                        let ci = node.pointer("/commits/nodes/0/commit/statusCheckRollup/state").and_then(Value::as_str).map(|s| match s { "ERROR" | "FAILURE" => "failure".into(), "EXPECTED" | "PENDING" => "pending".into(), _ => s.to_ascii_lowercase() });
                        let v: crate::models::GithubPullRequestJson = serde_json::from_value(node)?;
                        db.upsert_pull_request(&PullRequest { repository_id:repo.id, repository:repo.name_with_owner.clone(), number:v.number,title:v.title,state:v.state,is_draft:v.is_draft,created_at:v.created_at,updated_at:v.updated_at,merged_at:v.merged_at,closed_at:v.closed_at,url:v.url,additions:v.additions.unwrap_or(0),deletions:v.deletions.unwrap_or(0),changed_files:v.changed_files.unwrap_or(0),ci_state:ci })?;
                    } else {
                        node["labels"] = node["labels"]["nodes"].clone();
                        node["assignees"] = node["assignees"]["nodes"].clone();
                        let v: crate::models::GithubIssueJson = serde_json::from_value(node)?;
                        db.upsert_issue(&Issue {repository_id:repo.id,repository:repo.name_with_owner.clone(),number:v.number,title:v.title,state:v.state,created_at:v.created_at,updated_at:v.updated_at,closed_at:v.closed_at,url:v.url,author:v.author.map(|a|a.login),labels:v.labels.into_iter().map(|a|a.name).collect(),assignees:v.assignees.into_iter().map(|a|a.login).collect()})?;
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
