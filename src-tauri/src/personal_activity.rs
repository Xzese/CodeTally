//! Fast refreshes for open pull requests and issues involving the signed-in user.
//!
//! Search is used only to find repositories that need a normal activity import.
//! The activity importer remains the source of truth for pull request and issue
//! details and repository counts.

use crate::db::Database;
use crate::error::{command_error, AppError, AppResult};
use crate::github;
use crate::github_sync;
use crate::models::{ActivityRelationship, Repository, SyncProgress, SyncResult};
use crate::sync::AppState;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

pub const INTERVAL_SECONDS: u64 = 120;
pub const MATCHES_METADATA_KEY: &str = "personal_activity_matches";
pub const SEARCH_PAUSE_METADATA_KEY: &str = "personal_activity_pause_until";

const SEARCH_STRIKES_METADATA_KEY: &str = "personal_activity_search_strikes";

// Keep each quick refresh bounded to one GitHub search page. The regular full
// activity cadence remains responsible for walking older and additional items.
const SEARCH_LIMIT: &str = "100";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    PullRequests,
    Issues,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchRelation {
    Author,
    Assignee,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatchCache {
    /// Repositories returned by the latest complete set of searches.
    pub current: Vec<String>,
    /// Matches retained from earlier searches while the latest hydration is
    /// incomplete. These names let the next cycle reconcile disappearance.
    pub previous: Vec<String>,
    pub complete: bool,
    /// Relationship used for the search that produced `current`.
    #[serde(default)]
    pub relationship: ActivityRelationship,
}

impl Default for MatchCache {
    fn default() -> Self {
        Self {
            current: Vec::new(),
            previous: Vec::new(),
            complete: false,
            relationship: ActivityRelationship::default(),
        }
    }
}

impl MatchCache {
    fn all_names(&self) -> Vec<String> {
        union_repository_names(&self.current, &self.previous)
    }
}

/// Construct one of the four personal activity searches.
pub fn search_arguments(kind: SearchKind, relation: SearchRelation) -> Vec<String> {
    let kind = match kind {
        SearchKind::PullRequests => "prs",
        SearchKind::Issues => "issues",
    };
    let relation = match relation {
        SearchRelation::Author => "author",
        SearchRelation::Assignee => "assignee",
    };
    vec![
        "search".into(),
        kind.to_string(),
        format!("--{relation}"),
        "@me".into(),
        "--state".into(),
        "open".into(),
        "--limit".into(),
        SEARCH_LIMIT.into(),
        "--sort".into(),
        "updated".into(),
        "--order".into(),
        "desc".into(),
        "--json".into(),
        "repository".into(),
    ]
}

impl std::fmt::Display for SearchRelation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            SearchRelation::Author => "author",
            SearchRelation::Assignee => "assignee",
        })
    }
}

/// Build the four searches in stable order for callers and focused tests.
pub fn search_argument_sets() -> [Vec<String>; 4] {
    [
        search_arguments(SearchKind::PullRequests, SearchRelation::Author),
        search_arguments(SearchKind::PullRequests, SearchRelation::Assignee),
        search_arguments(SearchKind::Issues, SearchRelation::Author),
        search_arguments(SearchKind::Issues, SearchRelation::Assignee),
    ]
}

/// Case-insensitive union used for both search results and retry metadata.
pub fn union_repository_names(groups: &[String], additional: &[String]) -> Vec<String> {
    let mut names = BTreeMap::<String, String>::new();
    for name in groups.iter().chain(additional) {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        names.entry(name.to_ascii_lowercase()).or_insert_with(|| name.to_string());
    }
    names.into_values().collect()
}

/// Select only tracked repositories whose names appeared in a search result.
/// The database supplies the selected set, while this helper also guards
/// against archived rows for callers that use it independently.
pub fn select_matching_repositories(repositories: &[Repository], names: &[String]) -> Vec<Repository> {
    let wanted = names
        .iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    repositories
        .iter()
        .filter(|repo| !repo.is_archived && wanted.contains(&repo.name_with_owner.to_ascii_lowercase()))
        .cloned()
        .collect()
}

/// Parse the repository field returned by `gh search ... --json repository`.
/// gh has returned both a string and an object shape across CLI versions, so
/// accept either while requiring every result to identify a repository.
pub fn parse_repository_names(stdout: &str) -> AppResult<Vec<String>> {
    let values: Vec<Value> = serde_json::from_str(stdout)?;
    values
        .iter()
        .map(|item| {
            let repository = item
                .get("repository")
                .ok_or_else(|| AppError::InvalidArgument("GitHub search result missing repository".into()))?;
            repository_name(repository).ok_or_else(|| {
                AppError::InvalidArgument("GitHub search result has an invalid repository".into())
            })
        })
        .collect()
}

fn repository_name(value: &Value) -> Option<String> {
    if let Some(name) = value.as_str() {
        return (!name.trim().is_empty()).then(|| name.trim().to_string());
    }
    let object = value.as_object()?;
    for key in ["nameWithOwner", "name_with_owner", "fullName", "full_name"] {
        if let Some(name) = object.get(key).and_then(Value::as_str).filter(|name| !name.trim().is_empty()) {
            return Some(name.trim().to_string());
        }
    }
    let name = object.get("name").and_then(Value::as_str)?.trim();
    let owner = object.get("owner")?;
    let owner = owner
        .get("login")
        .and_then(Value::as_str)
        .or_else(|| owner.get("name").and_then(Value::as_str))?
        .trim();
    (!name.is_empty() && !owner.is_empty()).then(|| format!("{owner}/{name}"))
}

fn search(kind: SearchKind, relation: SearchRelation) -> AppResult<Vec<String>> {
    let output = Command::new(github::command_path("gh"))
        // Search must always target github.com even when a GHES host is set in
        // the app's inherited environment.
        .env("GH_HOST", "github.com")
        .args(search_arguments(kind, relation))
        .output()?;
    if !output.status.success() {
        return Err(command_error("gh search", output));
    }
    parse_repository_names(&String::from_utf8_lossy(&output.stdout))
}

fn rate_limited(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limit")
        || message.contains("rate_limit")
        || message.contains("secondary")
        || message.contains("abuse")
        || message.contains("http 429")
        || message.contains("429")
}

fn search_pause(db: &Database) -> AppResult<Option<AppError>> {
    Ok(db
        .metadata(SEARCH_PAUSE_METADATA_KEY)?
        .and_then(|until| {
            until.parse::<DateTime<Utc>>().ok().filter(|value| *value > Utc::now()).map(|_| AppError::RateLimited {
                until,
                reason: "Personal activity search rate limit".into(),
            })
        }))
}

fn search_guarded<T>(db: &Database, call: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
    // A persisted GraphQL pause applies to every GitHub request, including
    // search. Search failures get their own cooldown because `gh search` does
    // not expose the GraphQL quota reset used by github_sync::guarded.
    github_sync::ensure_available(db)?;
    if let Some(error) = search_pause(db)? {
        return Err(error);
    }
    match call() {
        Err(error) if rate_limited(&error.to_string()) => {
            let strikes = db
                .metadata(SEARCH_STRIKES_METADATA_KEY)?
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0)
                .min(6);
            let seconds = (60_i64 * 2_i64.pow(strikes)).min(3600);
            let until = Utc::now() + Duration::seconds(seconds);
            db.set_metadata(SEARCH_STRIKES_METADATA_KEY, &(strikes + 1).to_string())?;
            db.set_metadata(SEARCH_PAUSE_METADATA_KEY, &until.to_rfc3339())?;
            Err(AppError::RateLimited { until: until.to_rfc3339(), reason: error.to_string() })
        }
        other => other,
    }
}

fn reset_search_pause(db: &Database) -> AppResult<()> {
    db.set_metadata(SEARCH_STRIKES_METADATA_KEY, "0")?;
    db.set_metadata(SEARCH_PAUSE_METADATA_KEY, "")?;
    Ok(())
}

fn load_match_cache(db: &Database) -> AppResult<MatchCache> {
    let Some(value) = db.metadata(MATCHES_METADATA_KEY)? else {
        return Ok(MatchCache::default());
    };
    let mut cache: MatchCache = serde_json::from_str(&value)?;
    cache.current = union_repository_names(&cache.current, &[]);
    cache.previous = union_repository_names(&cache.previous, &[]);
    Ok(cache)
}
fn searches_for_relationship(relationship: ActivityRelationship) -> Vec<(SearchKind, SearchRelation, &'static str)> {
    let author = [
        (SearchKind::PullRequests, SearchRelation::Author, "pull requests authored by you"),
        (SearchKind::Issues, SearchRelation::Author, "issues authored by you"),
    ];
    let assignee = [
        (SearchKind::PullRequests, SearchRelation::Assignee, "pull requests assigned to you"),
        (SearchKind::Issues, SearchRelation::Assignee, "issues assigned to you"),
    ];
    match relationship {
        ActivityRelationship::Assignee => assignee.into(),
        ActivityRelationship::AuthorOrAssignee => author.into_iter().chain(assignee).collect(),
        _ => author.into(),
    }
}

fn save_match_cache(db: &Database, cache: &MatchCache) -> AppResult<()> {
    db.set_metadata(MATCHES_METADATA_KEY, &serde_json::to_string(cache)?)
}

fn first_import_complete(db: &Database) -> AppResult<bool> {
    Ok(db
        .metadata("github_discovered_at")?
        .is_some_and(|value| value.parse::<DateTime<Utc>>().is_ok()))
}

fn searching_progress(state: &AppState, index: i64, total: i64, message: String) {
    state.set_progress(SyncProgress {
        running: true,
        phase: "searching_personal_activity".into(),
        current: index,
        total,
        repository_current: 0,
        repository_total: 0,
        message,
        ..SyncProgress::default()
    });
}

fn finishing_progress(state: &AppState, error: Option<String>) {
    let prior = state.progress();
    let db = state.database();
    let pause = github_sync::ensure_available(&db)
        .err()
        .or_else(|| search_pause(&db).ok().flatten())
        .map(|value| value.to_string());
    let final_error = pause.clone().or(error);
    state.set_progress(SyncProgress {
        running: false,
        phase: if pause.is_some() { "paused" } else { "idle" }.into(),
        current: prior.current,
        total: prior.total,
        repository_current: prior.repository_current,
        repository_total: prior.repository_total,
        snapshot_current: 0,
        snapshot_total: 0,
        repository_name: prior.repository_name,
        message: pause.unwrap_or_else(|| if final_error.is_some() { "Finished with errors".into() } else { "Ready".into() }),
        error: final_error,
    });
}

/// Run the quick personal activity refresh. Search results only select
/// already-tracked repositories; all item imports remain in github_sync.
pub fn sync(state: &AppState) -> AppResult<SyncResult> {
    let db = state.database();
    if !first_import_complete(&db)? {
        return Ok(SyncResult { ok: true, message: "Waiting for first repository import".into(), ..SyncResult::default() });
    }
    let selected = db.selected_repositories()?;
    if selected.is_empty() {
        return Ok(SyncResult { ok: true, message: "No selected repositories".into(), ..SyncResult::default() });
    }

    let relationship = crate::sync::app_settings(&db)?.activity_relationship;
    if relationship == ActivityRelationship::Everyone {
        return Ok(SyncResult { ok: true, message: "Personal activity refresh is disabled for everyone".into(), ..SyncResult::default() });
    }

    let old_cache = load_match_cache(&db)?;
    let old_matches = old_cache.all_names();
    let mut current = Vec::new();
    let searches = searches_for_relationship(relationship);
    let search_total = searches.len() as i64;
    for (index, (kind, relation, label)) in searches.iter().copied().enumerate() {
        searching_progress(state, index as i64, search_total, format!("Searching {label}"));
        match search_guarded(&db, || search(kind, relation)) {
            Ok(names) => current.extend(names),
            Err(error) => {
                finishing_progress(state, Some(error.to_string()));
                // Do not modify match metadata after a failed search. The
                // previous union remains intact for the next successful pass.
                return Err(error);
            }
        }
    }
    reset_search_pause(&db)?;
    let current = union_repository_names(&current, &[]);
    let all_matches = union_repository_names(&current, &old_matches);
    // Save the complete retry union before hydration. A pause halfway through
    // the repository loop must retain every candidate, including new matches.
    save_match_cache(&db, &MatchCache { current: current.clone(), previous: old_matches.clone(), complete: false, relationship })?;

    let repositories = select_matching_repositories(&selected, &all_matches);
    let repository_total = repositories.len() as i64;
    state.set_progress(SyncProgress {
        running: true,
        phase: "syncing_personal_activity".into(),
        current: 0,
        total: repository_total,
        repository_current: 0,
        repository_total,
        message: "Refreshing personal activity".into(),
        ..SyncProgress::default()
    });

    let mut result = SyncResult { ok: true, message: "Personal activity refresh complete".into(), ..SyncResult::default() };
    for (index, repo) in repositories.iter().enumerate() {
        if !db.repository_enabled(repo)? {
            let mut progress = state.progress();
            progress.current = index as i64 + 1;
            progress.repository_current = index as i64 + 1;
            progress.message = format!("Skipped {} because it is no longer selected", repo.name_with_owner);
            state.set_progress(progress);
            continue;
        }
        if let Err(error) = github_sync::ensure_available(&db) {
            result.ok = false;
            result.errors.push(error.to_string());
            break;
        }
        let mut progress = state.progress();
        progress.running = true;
        progress.phase = "syncing_personal_activity".into();
        progress.current = index as i64;
        progress.total = repository_total;
        progress.repository_current = index as i64;
        progress.repository_total = repository_total;
        progress.repository_name = Some(repo.name_with_owner.clone());
        progress.message = format!("Refreshing personal activity for {}", repo.name_with_owner);
        progress.error = None;
        state.set_progress(progress);

        let rate_limited = match github_sync::sync_activity(&db, repo) {
            Ok((prs, issues, complete)) => {
                result.pull_requests_synced += prs;
                result.issues_synced += issues;
                if complete {
                    result.repositories_synced += 1;
                    result.activity_repositories_synced += 1;
                    db.mark_sync(repo.id, None)?;
                } else {
                    result.ok = false;
                    let error = "Activity import is continuing next cycle".to_string();
                    result.errors.push(format!("{}: {}", repo.name_with_owner, error));
                    db.mark_sync(repo.id, Some(&error))?;
                }
                false
            }
            Err(error) => {
                result.ok = false;
                result.errors.push(format!("{}: {}", repo.name_with_owner, error));
                db.mark_sync(repo.id, Some(&error.to_string()))?;
                matches!(error, AppError::RateLimited { .. })
            }
        };
        let mut progress = state.progress();
        progress.current = index as i64 + 1;
        progress.repository_current = index as i64 + 1;
        progress.message = format!("Completed {}/{} repositories", index + 1, repository_total);
        state.set_progress(progress);
        if rate_limited {
            break;
        }
    }

    if result.ok {
        save_match_cache(&db, &MatchCache { current, previous: Vec::new(), complete: true, relationship })?;
    } else {
        result.message = "Personal activity refresh completed with some errors".into();
    }
    finishing_progress(state, result.errors.first().cloned());
    Ok(result)
}
