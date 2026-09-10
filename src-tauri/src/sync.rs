use crate::classify::month_sample_dates;
use crate::db::Database;
use crate::error::{AppError, AppResult};
use crate::gitops::{commit_at_or_before, current_commit, current_commit_optional, earliest_commit, ensure_clone, scan_at_commit_with_config};
use crate::github;
use crate::github_sync;
use crate::models::{AppSettings, Repository, Snapshot, SyncProgress, SyncResult};
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub progress: Arc<Mutex<SyncProgress>>,
    pub job_lock: Arc<Mutex<()>>,
}

pub const LOC_SWEEP_METADATA_KEY: &str = "last_loc_sweep_at";
pub const APP_SETTINGS_METADATA_KEY: &str = "app_settings";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocSyncDecision {
    pub run: bool,
    pub force_fetch: bool,
}

pub fn app_settings(db: &Database) -> AppResult<AppSettings> {
    let Some(value) = db.metadata(APP_SETTINGS_METADATA_KEY)? else { return Ok(AppSettings::default()); };
    let settings = serde_json::from_str::<AppSettings>(&value).unwrap_or_default();
    if validate_app_settings(&settings).is_ok() { Ok(settings) } else { Ok(AppSettings::default()) }
}

pub fn validate_app_settings(settings: &AppSettings) -> AppResult<()> {
    if !matches!(settings.activity_refresh_minutes, 1 | 2 | 5 | 10 | 15) {
        return Err(AppError::InvalidArgument("activity_refresh_minutes must be one of 1, 2, 5, 10, or 15".into()));
    }
    if !matches!(settings.lines_refresh_minutes, 30 | 45 | 60) {
        return Err(AppError::InvalidArgument("lines_refresh_minutes must be one of 30, 45, or 60".into()));
    }
    Ok(())
}

pub fn save_app_settings(db: &Database, settings: &AppSettings) -> AppResult<()> {
    validate_app_settings(settings)?;
    db.set_metadata(APP_SETTINGS_METADATA_KEY, &serde_json::to_string(settings)?)
}

/// Decide whether an automatic activity refresh should also inspect LOC.
/// This function is intentionally independent of the database and wall clock
/// access so cadence behavior can be tested without invoking GitHub or Git.
pub fn decide_loc_sync(repo: &Repository, now: DateTime<Utc>, last_sweep_at: Option<&str>, force_full: bool) -> LocSyncDecision {
    decide_loc_sync_with_settings(repo, now, last_sweep_at, force_full, &AppSettings::default())
}

pub fn decide_loc_sync_with_settings(repo: &Repository, now: DateTime<Utc>, last_sweep_at: Option<&str>, force_full: bool, settings: &AppSettings) -> LocSyncDecision {
    if force_full {
        return LocSyncDecision { run: true, force_fetch: true };
    }
    let sweep_due = loc_sweep_due_with_interval(last_sweep_at, now, settings.lines_refresh_minutes);
    let first_scan = !repo.loc_backfill_complete || repo.local_path.is_none();
    let pushed_at_changed = settings.refresh_lines_on_change && repo.last_fetched_pushed_at != repo.pushed_at;
    let fetch_cursor_missing = repo.last_fetched_pushed_at.is_none() && repo.local_path.is_some();
    LocSyncDecision {
        run: first_scan || pushed_at_changed || sweep_due,
        force_fetch: sweep_due || fetch_cursor_missing,
    }
}

pub fn loc_sweep_due(last_sweep_at: Option<&str>, now: DateTime<Utc>) -> bool {
    loc_sweep_due_with_interval(last_sweep_at, now, AppSettings::default().lines_refresh_minutes)
}

pub fn loc_sweep_due_with_interval(last_sweep_at: Option<&str>, now: DateTime<Utc>, interval_minutes: i64) -> bool {
    let Some(value) = last_sweep_at else { return true; };
    let Ok(last) = value.parse::<DateTime<Utc>>() else { return true; };
    now.signed_duration_since(last) >= Duration::minutes(interval_minutes.max(1))
}

impl AppState {
    pub fn database(&self) -> Database {
        Database::new(&self.db_path)
    }

    pub fn set_progress(&self, progress: SyncProgress) {
        if let Ok(mut current) = self.progress.lock() {
            *current = progress;
        }
    }

    pub fn progress(&self) -> SyncProgress {
        self.progress.lock().map(|value| value.clone()).unwrap_or_default()
    }
}

pub fn discover(state: &AppState) -> AppResult<Vec<Repository>> {
    let db = state.database();
    github_sync::ensure_available(&db)?;
    github_sync::graphql(&db, "query{rateLimit{remaining resetAt}}", serde_json::json!({}))?;
    github_sync::ensure_available(&db)?;
    let user = github_sync::guarded(&db, github::current_user)?;
    db.set_metadata("github_login", &user.login)?;
    db.set_metadata("org_discovery_errors", "")?;
    let mut discovered = github_sync::guarded(&db, || github::list_repositories(&user.login))?;
    let mut org_errors = Vec::new();
    match github_sync::guarded(&db, github::list_organizations) {
        Ok(organizations) => {
            for organization in organizations {
                match github_sync::guarded(&db, || github::list_repositories_for_owner(&organization)) {
                    Ok(repositories) => discovered.extend(repositories),
                    Err(error @ AppError::RateLimited { .. }) => return Err(error),
                    Err(error) => org_errors.push(format!("{organization}: {error}")),
                }
            }
        }
        Err(error @ AppError::RateLimited { .. }) => return Err(error),
        Err(error) => org_errors.push(format!("organization discovery: {error}")),
    }
    if !org_errors.is_empty() {
        db.set_metadata("org_discovery_errors", &org_errors.join("\n"))?;
    }
    let mut seen = BTreeSet::new();
    for repo in discovered.into_iter().filter(|repo| seen.insert(repo.github_id.clone())) {
        db.upsert_repository(&repo)?;
    }
    if org_errors.is_empty() { db.set_metadata("github_discovered_at", &Utc::now().to_rfc3339())?; }
    db.repositories()
}

pub fn sync_all(state: &AppState) -> AppResult<SyncResult> {
    github_sync::ensure_available(&state.database())?;
    let mut result = SyncResult { ok: true, message: "Refresh complete".into(), ..SyncResult::default() };
    state.set_progress(SyncProgress { running: true, phase: "discovering".into(), message: "Discovering repositories".into(), ..SyncProgress::default() });

    if !github::dependency_status().gh_authenticated {
        result.ok = false;
        result.message = "GitHub CLI is not authenticated".into();
        result.errors.push("Run gh auth login, then refresh CodeTally.".into());
        finish_progress(state, result.errors.first().cloned());
        return Ok(result);
    }

    let repos = match discover(state) {
        Ok(repos) => repos,
        Err(error) => {
            result.ok = false;
            result.errors.push(error.to_string());
            state.database().repositories()?
        }
    };
    if let Some(org_errors) = state.database().metadata("org_discovery_errors")? {
        if !org_errors.trim().is_empty() {
            result.ok = false;
            result.errors.extend(org_errors.lines().map(|error| format!("Organization discovery: {error}")));
        }
    }
    if let Err(error) = github_sync::ensure_available(&state.database()) {
        result.ok = false;
        result.errors.push(error.to_string());
        result.message = error.to_string();
        finish_progress(state, Some(error.to_string()));
        return Ok(result);
    }
    let mut active: Vec<Repository> = repos.into_iter().filter(|repo| !repo.is_archived).collect();
    let last = state.database().metadata("github_last_attempted_repository")?.and_then(|s| s.parse::<i64>().ok());
    if let Some(position) = active.iter().position(|repo| Some(repo.id) == last) {
        let length = active.len();
        active.rotate_left((position + 1) % length);
    }
    let repository_total = active.len() as i64;
    state.set_progress(SyncProgress { running: true, phase: "syncing".into(), current: 0, total: repository_total, repository_current: 0, repository_total, message: "Refreshing GitHub activity and line counts".into(), ..SyncProgress::default() });
    for (index, repo) in active.into_iter().enumerate() {
        if let Err(error) = github_sync::ensure_available(&state.database()) {
            result.ok = false;
            result.errors.push(error.to_string());
            break;
        }
        state.database().set_metadata("github_last_attempted_repository", &repo.id.to_string())?;
        state.set_progress(SyncProgress { running: true, phase: "syncing_repository".into(), current: index as i64, total: repository_total, repository_current: index as i64, repository_total, snapshot_current: 0, snapshot_total: 0, repository_name: Some(repo.name_with_owner.clone()), message: format!("Refreshing activity and line counts for {}", repo.name_with_owner), ..SyncProgress::default() });
        match sync_repo_data(state, &repo, true, true) {
            Ok((prs, issues, snapshots, repo_errors, loc_completed)) => {
                if repo_errors.is_empty() {
                    result.repositories_synced += 1;
                    result.activity_repositories_synced += 1;
                }
                if loc_completed { result.loc_repositories_synced += 1; }
                result.pull_requests_synced += prs;
                result.issues_synced += issues;
                result.snapshots_created += snapshots;
                if !repo_errors.is_empty() {
                    result.ok = false;
                    result.errors.extend(repo_errors.into_iter().map(|error| format!("{}: {}", repo.name_with_owner, error)));
                }
            }
            Err(error) => {
                result.ok = false;
                result.errors.push(format!("{}: {}", repo.name_with_owner, error));
                let _ = state.database().mark_sync(repo.id, Some(&error.to_string()));
            }
        }
        // `current`/`total` are the legacy overall repository progress fields.
        // Snapshot progress has its own fields and must never replace them.
        let mut progress = state.progress();
        progress.current = index as i64 + 1;
        progress.total = repository_total;
        progress.repository_current = index as i64 + 1;
        progress.repository_total = repository_total;
        progress.snapshot_current = 0;
        progress.snapshot_total = 0;
        progress.message = format!("Completed {}/{} repositories", index + 1, repository_total);
        state.set_progress(progress);
    }
    if !result.errors.is_empty() {
        result.message = "Refresh completed with some errors".into();
    }
    if let Err(error) = if result.ok { state.database().set_metadata(LOC_SWEEP_METADATA_KEY, &Utc::now().to_rfc3339()) } else { Ok(()) } {
        result.ok = false;
        result.errors.push(format!("Line-count sweep timestamp: {error}"));
    }
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

/// Refresh repository metadata and activity frequently, scanning LOC only for
/// repositories that are new, changed, incomplete, or due for the periodic
/// verification sweep.
pub fn sync_activity(state: &AppState) -> AppResult<SyncResult> {
    github_sync::ensure_available(&state.database())?;
    let mut result = SyncResult { ok: true, message: "Activity refresh complete".into(), ..SyncResult::default() };
    state.set_progress(SyncProgress { running: true, phase: "discovering_activity".into(), message: "Discovering repository activity".into(), ..SyncProgress::default() });

    if !github::dependency_status().gh_authenticated {
        result.ok = false;
        result.message = "GitHub CLI is not authenticated".into();
        result.errors.push("Run gh auth login, then refresh CodeTally.".into());
        finish_progress(state, result.errors.first().cloned());
        return Ok(result);
    }

    let cached = state.database().metadata("github_discovered_at")?.and_then(|s| s.parse::<DateTime<Utc>>().ok()).is_some_and(|t| Utc::now() - t < Duration::hours(1));
    let repos = match if cached { state.database().repositories() } else { discover(state) } {
        Ok(repos) => repos,
        Err(error) => {
            result.ok = false;
            result.errors.push(error.to_string());
            state.database().repositories()?
        }
    };
    if let Some(org_errors) = state.database().metadata("org_discovery_errors")? {
        if !org_errors.trim().is_empty() {
            result.ok = false;
            result.errors.extend(org_errors.lines().map(|error| format!("Organization discovery: {error}")));
        }
    }
    if let Err(error) = github_sync::ensure_available(&state.database()) {
        result.ok = false;
        result.errors.push(error.to_string());
        result.message = error.to_string();
        finish_progress(state, Some(error.to_string()));
        return Ok(result);
    }
    let mut active: Vec<Repository> = repos.into_iter().filter(|repo| !repo.is_archived).collect();
    let last = state.database().metadata("github_last_attempted_repository")?.and_then(|s| s.parse::<i64>().ok());
    if let Some(position) = active.iter().position(|repo| Some(repo.id) == last) {
        let length = active.len();
        active.rotate_left((position + 1) % length);
    }
    let repository_total = active.len() as i64;
    let now = Utc::now();
    let db = state.database();
    let settings = app_settings(&db)?;
    let last_sweep_at = db.metadata(LOC_SWEEP_METADATA_KEY)?;
    let sweep_due = loc_sweep_due_with_interval(last_sweep_at.as_deref(), now, settings.lines_refresh_minutes);
    state.set_progress(SyncProgress { running: true, phase: "syncing_activity".into(), current: 0, total: repository_total, repository_current: 0, repository_total, snapshot_current: 0, snapshot_total: 0, message: if sweep_due { "Refreshing activity and due line-count checks".into() } else { "Refreshing GitHub activity".into() }, ..SyncProgress::default() });

    for (index, repo) in active.into_iter().enumerate() {
        if let Err(error) = github_sync::ensure_available(&state.database()) {
            result.ok = false;
            result.errors.push(error.to_string());
            break;
        }
        state.database().set_metadata("github_last_attempted_repository", &repo.id.to_string())?;
        let decision = decide_loc_sync_with_settings(&repo, now, last_sweep_at.as_deref(), false, &settings);
        if !decision.run {
            result.loc_repositories_skipped += 1;
        }
        state.set_progress(SyncProgress { running: true, phase: if decision.run { "syncing_repository" } else { "syncing_activity" }.into(), current: index as i64, total: repository_total, repository_current: index as i64, repository_total, snapshot_current: 0, snapshot_total: 0, repository_name: Some(repo.name_with_owner.clone()), message: if decision.run { format!("Refreshing activity and line counts for {}", repo.name_with_owner) } else { format!("Refreshing activity for {}", repo.name_with_owner) }, ..SyncProgress::default() });
        match sync_repo_data(state, &repo, decision.run, decision.force_fetch) {
            Ok((prs, issues, snapshots, repo_errors, loc_completed)) => {
                if repo_errors.is_empty() {
                    result.repositories_synced += 1;
                    result.activity_repositories_synced += 1;
                }
                if loc_completed {
                    result.loc_repositories_synced += 1;
                }
                result.pull_requests_synced += prs;
                result.issues_synced += issues;
                result.snapshots_created += snapshots;
                if !repo_errors.is_empty() {
                    result.ok = false;
                    result.errors.extend(repo_errors.into_iter().map(|error| format!("{}: {}", repo.name_with_owner, error)));
                }
            }
            Err(error) => {
                result.ok = false;
                result.errors.push(format!("{}: {}", repo.name_with_owner, error));
                let _ = state.database().mark_sync(repo.id, Some(&error.to_string()));
            }
        }
        let mut progress = state.progress();
        progress.current = index as i64 + 1;
        progress.total = repository_total;
        progress.repository_current = index as i64 + 1;
        progress.repository_total = repository_total;
        progress.snapshot_current = 0;
        progress.snapshot_total = 0;
        progress.message = format!("Completed {}/{} repositories", index + 1, repository_total);
        state.set_progress(progress);
    }
    if sweep_due && result.ok {
        if let Err(error) = state.database().set_metadata(LOC_SWEEP_METADATA_KEY, &now.to_rfc3339()) {
            result.ok = false;
            result.errors.push(format!("Line-count sweep timestamp: {error}"));
        }
    }
    if !result.errors.is_empty() {
        result.message = "Activity refresh completed with some errors".into();
    } else if result.loc_repositories_synced > 0 {
        result.message = format!("Activity refresh complete; line counts checked for {} repositories", result.loc_repositories_synced);
    }
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

pub fn sync_one(state: &AppState, repository_id: i64) -> AppResult<SyncResult> {
    github_sync::ensure_available(&state.database())?;
    let repo = state.database().repository(repository_id)?.ok_or(AppError::RepositoryNotFound(repository_id))?;
    state.set_progress(SyncProgress { running: true, phase: "syncing_repository".into(), current: 0, total: 1, repository_current: 0, repository_total: 1, repository_name: Some(repo.name_with_owner.clone()), message: format!("Refreshing activity and line counts for {}", repo.name_with_owner), ..SyncProgress::default() });
    let mut result = SyncResult { ok: true, message: "Repository refresh complete".into(), ..SyncResult::default() };
    match sync_repo_data(state, &repo, true, true) {
        Ok((prs, issues, snapshots, repo_errors, loc_completed)) => {
            if repo_errors.is_empty() {
                result.repositories_synced = 1;
                result.activity_repositories_synced = 1;
            }
            result.loc_repositories_synced = i64::from(loc_completed);
            result.pull_requests_synced = prs;
            result.issues_synced = issues;
            result.snapshots_created = snapshots;
            if !repo_errors.is_empty() {
                result.ok = false;
                result.errors.extend(repo_errors);
            }
        }
        Err(error) => {
            result.ok = false;
            result.message = error.to_string();
            result.errors.push(error.to_string());
        }
    }
    let mut progress = state.progress();
    progress.current = 1;
    progress.total = 1;
    progress.repository_current = 1;
    progress.repository_total = 1;
    progress.snapshot_current = 0;
    progress.snapshot_total = 0;
    state.set_progress(progress);
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

pub fn backfill_one(state: &AppState, repository_id: i64) -> AppResult<SyncResult> {
    github_sync::ensure_available(&state.database())?;
    let repo = state.database().repository(repository_id)?.ok_or(AppError::RepositoryNotFound(repository_id))?;
    state.set_progress(SyncProgress { running: true, phase: "backfilling".into(), current: 0, total: 1, repository_current: 0, repository_total: 1, repository_name: Some(repo.name_with_owner.clone()), message: format!("Building line history for {}", repo.name_with_owner), ..SyncProgress::default() });
    let mut result = SyncResult { ok: true, message: "Line history backfill complete".into(), ..SyncResult::default() };
    match backfill_repo(state, &repo) {
        Ok(created) => {
            result.snapshots_created = created;
            result.loc_repositories_synced = 1;
        }
        Err(error) => { result.ok = false; result.message = error.to_string(); result.errors.push(error.to_string()); }
    }
    let mut progress = state.progress();
    progress.current = 1;
    progress.total = 1;
    progress.repository_current = 1;
    progress.repository_total = 1;
    state.set_progress(progress);
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

fn sync_repo_data(state: &AppState, repo: &Repository, run_loc: bool, force_fetch: bool) -> AppResult<(i64, i64, i64, Vec<String>, bool)> {
    let db = state.database();
    github_sync::ensure_available(&db)?;
    let mut counts = [0, 0];
    let mut errors = Vec::new();
    let activity_fetched = match github_sync::sync_activity_reporting(&db, repo, &mut counts) {
        Ok((_, _, complete)) => {
            if !complete { errors.push("Activity import is continuing next cycle".into()); }
            true
        }
        Err(error) => { errors.push(error.to_string()); false }
    };
    let mut loc_completed = false;
    let mut snapshots = 0;
    if activity_fetched {
        match github_sync::ensure_available(&db) {
            Err(error) => errors.push(error.to_string()),
            Ok(()) if run_loc => match sync_loc(state, repo, force_fetch) {
                Ok(created) => { snapshots = created; loc_completed = true; }
                Err(error) => errors.push(error.to_string()),
            },
            Ok(()) => {},
        }
    }
    let error_text = if errors.is_empty() { None } else { Some(errors.join("; ")) };
    db.mark_sync(repo.id, error_text.as_deref())?;
    Ok((counts[0], counts[1], snapshots, errors, loc_completed))
}

fn sync_loc(state: &AppState, repo: &Repository, force_fetch: bool) -> AppResult<i64> {
    let db = state.database();
    let cached_path = repo.local_path.as_deref().map(Path::new).filter(|path| path.join(".git").is_dir()).map(Path::to_path_buf);
    let needs_fetch = force_fetch || cached_path.is_none() || repo.last_fetched_pushed_at.as_deref() != repo.pushed_at.as_deref();
    let path = if needs_fetch {
        let path = ensure_clone(repo, &state.cache_dir)?;
        db.set_local_path(repo.id, &path.to_string_lossy())?;
        db.set_fetched_pushed_at(repo.id, repo.pushed_at.as_deref())?;
        path
    } else {
        cached_path.expect("cached path checked above")
    };
    let mut created = 0;
    let mut backfill_error = None;
    let config = db.classification_config(repo.id)?;
    // A newly created empty GitHub repository has no HEAD to analyse. Treat it
    // as a valid zero LOC repository and mark history complete instead of
    // retrying a failing rev-list on every refresh.
    if current_commit_optional(&path)?.is_none() {
        let now = Utc::now();
        let empty = Snapshot { id: 0, repository_id: repo.id, commit_sha: "EMPTY_REPOSITORY".into(), commit_date: now.to_rfc3339(), snapshot_date: now.to_rfc3339(), total_loc: 0, source_loc: 0, test_loc: 0, created_at: now.to_rfc3339() };
        let created = if db.upsert_snapshot(&empty)? { 1 } else { 0 };
        db.upsert_observation(&empty)?;
        db.set_backfill_complete(repo.id, true)?;
        return Ok(created);
    }
    if !repo.loc_backfill_complete {
        match backfill_repo_at_path(state, repo, &path) {
            Ok(count) => {
                created += count;
                db.set_backfill_complete(repo.id, true)?;
            }
            Err(error) => backfill_error = Some(error),
        }
    }
    let (sha, commit_date) = current_commit(&path)?;
    let mut measurement = db.snapshot_for_commit(repo.id, &sha)?;
    if measurement.is_none() {
        state.set_progress(SyncProgress { running: true, phase: "scanning_current".into(), repository_name: Some(repo.name_with_owner.clone()), message: format!("Scanning {}", repo.name_with_owner), ..state.progress() });
        let scan = scan_at_commit_with_config(&path, &sha, &repo.default_branch, Some(&config))?;
        let snapshot = Snapshot { id: 0, repository_id: repo.id, commit_sha: sha.clone(), commit_date, snapshot_date: Utc::now().to_rfc3339(), total_loc: scan.total_loc, source_loc: scan.source_loc, test_loc: scan.test_loc, created_at: Utc::now().to_rfc3339() };
        if db.upsert_snapshot(&snapshot)? { created += 1; }
        measurement = Some(snapshot);
    }
    let now = Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let latest_observation = db.latest_observation(repo.id)?;
    let should_observe = latest_observation.as_ref().map(|item| item.commit_sha != sha || item.snapshot_date.get(..10).unwrap_or(&item.snapshot_date) != today.as_str()).unwrap_or(true);
    if should_observe {
        if let Some(measurement) = measurement {
            let observation = Snapshot { id: 0, repository_id: repo.id, commit_sha: measurement.commit_sha, commit_date: measurement.commit_date, snapshot_date: now.to_rfc3339(), total_loc: measurement.total_loc, source_loc: measurement.source_loc, test_loc: measurement.test_loc, created_at: now.to_rfc3339() };
            db.upsert_observation(&observation)?;
        }
    }
    if let Some(error) = backfill_error { return Err(error); }
    Ok(created)
}

fn backfill_repo(state: &AppState, repo: &Repository) -> AppResult<i64> {
    let path = ensure_clone(repo, &state.cache_dir)?;
    let db = state.database();
    db.set_local_path(repo.id, &path.to_string_lossy())?;
    db.set_fetched_pushed_at(repo.id, repo.pushed_at.as_deref())?;
    if current_commit_optional(&path)?.is_none() {
        let now = Utc::now();
        let empty = Snapshot { id: 0, repository_id: repo.id, commit_sha: "EMPTY_REPOSITORY".into(), commit_date: now.to_rfc3339(), snapshot_date: now.to_rfc3339(), total_loc: 0, source_loc: 0, test_loc: 0, created_at: now.to_rfc3339() };
        let created = if db.upsert_snapshot(&empty)? { 1 } else { 0 };
        db.upsert_observation(&empty)?;
        db.set_backfill_complete(repo.id, true)?;
        return Ok(created);
    }
    let created = backfill_repo_at_path(state, repo, &path)?;
    if created >= 0 { state.database().set_backfill_complete(repo.id, true)?; }
    Ok(created)
}

fn backfill_repo_at_path(state: &AppState, repo: &Repository, path: &Path) -> AppResult<i64> {
    let history_start = earliest_commit(path, &repo.default_branch)?
        .map(|(_, date)| date)
        .or_else(|| repo.created_at.clone());
    let dates = month_sample_dates(history_start.as_deref(), Utc::now());
    let prior = state.progress();
    state.set_progress(SyncProgress { running: true, phase: "backfilling".into(), current: prior.current, total: prior.total, repository_current: prior.repository_current, repository_total: prior.repository_total, snapshot_current: 0, snapshot_total: dates.len() as i64, repository_name: Some(repo.name_with_owner.clone()), message: "Selecting monthly commits".into(), ..SyncProgress::default() });
    let db = state.database();
    let config = db.classification_config(repo.id)?;
    let mut created = 0;
    let mut errors = Vec::new();
    for (index, sample_date) in dates.iter().enumerate() {
        state.set_progress(SyncProgress { running: true, phase: "backfilling".into(), current: prior.current, total: prior.total, repository_current: prior.repository_current, repository_total: prior.repository_total, snapshot_current: index as i64, snapshot_total: dates.len() as i64, repository_name: Some(repo.name_with_owner.clone()), message: format!("Analysing snapshot {}/{}", index + 1, dates.len()), ..SyncProgress::default() });
        let Some((sha, commit_date)) = commit_at_or_before(path, &repo.default_branch, sample_date)? else {
            let mut progress = state.progress();
            progress.snapshot_current = index as i64 + 1;
            state.set_progress(progress);
            continue;
        };
        if db.has_commit_snapshot(repo.id, &sha)? {
            db.move_snapshot_date_earlier(repo.id, &sha, sample_date)?;
            let mut progress = state.progress();
            progress.snapshot_current = index as i64 + 1;
            state.set_progress(progress);
            continue;
        }
        match scan_at_commit_with_config(path, &sha, &repo.default_branch, Some(&config)) {
            Ok(scan) => {
                let snapshot = Snapshot { id: 0, repository_id: repo.id, commit_sha: sha, commit_date, snapshot_date: sample_date.clone(), total_loc: scan.total_loc, source_loc: scan.source_loc, test_loc: scan.test_loc, created_at: Utc::now().to_rfc3339() };
                if db.upsert_snapshot(&snapshot)? { created += 1; }
            }
            Err(error) => errors.push(error.to_string()),
        }
        let mut progress = state.progress();
        progress.snapshot_current = index as i64 + 1;
        state.set_progress(progress);
    }
    if let Err(error) = crate::gitops::checkout_branch(path, &repo.default_branch) { errors.push(error.to_string()); }
    if let Some(error) = errors.first() { return Err(AppError::Command { program: "tokei".into(), message: error.clone() }); }
    Ok(created)
}

fn finish_progress(state: &AppState, error: Option<String>) {
    let prior = state.progress();
    let pause = github_sync::ensure_available(&state.database()).err().map(|error| error.to_string());
    let error = pause.clone().or(error);
    state.set_progress(SyncProgress {
        running: false,
        phase: if pause.is_some() { "paused" } else { "idle" }.into(),
        current: prior.current,
        total: prior.total,
        repository_current: prior.repository_current,
        repository_total: prior.repository_total,
        snapshot_current: prior.snapshot_current,
        snapshot_total: prior.snapshot_total,
        message: pause.unwrap_or_else(|| if error.is_some() { "Finished with errors".into() } else { "Ready".into() }),
        error,
        repository_name: prior.repository_name,
    });
}
