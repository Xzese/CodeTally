use crate::classify::month_sample_dates;
use crate::db::{DashboardCache, Database};
use crate::error::{AppError, AppResult};
use crate::gitops::{commit_at_or_before_nonempty, current_commit, current_commit_optional, earliest_commit, ensure_clone, scan_at_commit_detached, scan_at_commit_with_config};
use crate::github;
use crate::github_sync;
use crate::models::{AppSettings, Repository, Snapshot, SyncProgress, SyncResult};
use chrono::{DateTime, Duration, Utc};
use rusqlite::params;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub progress: Arc<Mutex<SyncProgress>>,
    pub job_lock: Arc<Mutex<()>>,
    pub dashboard_cache: Arc<Mutex<DashboardCache>>,
}

pub const LOC_SWEEP_METADATA_KEY: &str = "last_loc_sweep_at";
pub const LAST_LOC_REFRESH_METADATA_KEY: &str = "last_loc_refresh_at";
pub const LAST_FULL_REFRESH_METADATA_KEY: &str = "last_full_refresh_at";
pub const LAST_ACTIVITY_REFRESH_METADATA_KEY: &str = "last_activity_refresh_at";
pub const LAST_PERSONAL_REFRESH_METADATA_KEY: &str = "last_personal_refresh_at";
pub const APP_SETTINGS_METADATA_KEY: &str = "app_settings";
pub const REFRESH_CADENCE_V2_METADATA_KEY: &str = "refresh_cadence_v2";
pub const REFRESH_CADENCE_V3_METADATA_KEY: &str = "refresh_cadence_v3";
pub const REFRESH_CADENCE_V4_METADATA_KEY: &str = "refresh_cadence_v4";
pub const REFRESH_CADENCE_V5_METADATA_KEY: &str = "refresh_cadence_v5";
const REPO_REFRESH_INTERVALS: [i64; 4] = [60, 1_440, 10_080, 43_200];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocSyncDecision {
    pub run: bool,
    pub force_fetch: bool,
}

pub fn app_settings(db: &Database) -> AppResult<AppSettings> {
    let Some(value) = db.metadata(APP_SETTINGS_METADATA_KEY)? else { return Ok(AppSettings::default()); };
    let mut settings = serde_json::from_str::<AppSettings>(&value).unwrap_or_default();
    if db.metadata(REFRESH_CADENCE_V2_METADATA_KEY)?.is_none() {
        // Move installations that were still on the previous defaults to the
        // quieter cadence once. Explicit custom values remain untouched.
        if settings.activity_refresh_minutes == 10 { settings.activity_refresh_minutes = 30; }
        if settings.lines_refresh_minutes == 45 { settings.lines_refresh_minutes = 120; }
        db.set_metadata(APP_SETTINGS_METADATA_KEY, &serde_json::to_string(&settings)?)?;
        db.set_metadata(REFRESH_CADENCE_V2_METADATA_KEY, "1")?;
    }
    if db.metadata(REFRESH_CADENCE_V3_METADATA_KEY)?.is_none() {
        // Move the previous code-verification default to daily. Explicitly
        // shorter custom values remain untouched. Older activity intervals
        // below the new API-safe minimum are raised to that minimum.
        if settings.activity_refresh_minutes < 15 { settings.activity_refresh_minutes = 15; }
        if settings.lines_refresh_minutes == 120 { settings.lines_refresh_minutes = 1440; }
        db.set_metadata(APP_SETTINGS_METADATA_KEY, &serde_json::to_string(&settings)?)?;
        db.set_metadata(REFRESH_CADENCE_V3_METADATA_KEY, "1")?;
    }
    if db.metadata(REFRESH_CADENCE_V4_METADATA_KEY)?.is_none() {
        // The previous 30-minute repository activity default becomes the
        // single daily Repo Refresh cadence. Other saved intervals, including
        // custom values, keep their exact value.
        if settings.activity_refresh_minutes == 30 {
            settings.activity_refresh_minutes = 1440;
        }
        db.set_metadata(APP_SETTINGS_METADATA_KEY, &serde_json::to_string(&settings)?)?;
        db.set_metadata(REFRESH_CADENCE_V4_METADATA_KEY, "1")?;
    }
    if db.metadata(REFRESH_CADENCE_V5_METADATA_KEY)?.is_none() {
        // Repo Refresh now has four presets. Migrate every older custom value
        // to Daily while leaving the independent personal interval untouched.
        if !REPO_REFRESH_INTERVALS.contains(&settings.activity_refresh_minutes) {
            settings.activity_refresh_minutes = 1_440;
        }
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO app_metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![APP_SETTINGS_METADATA_KEY, serde_json::to_string(&settings)?])?;
        tx.execute("INSERT INTO app_metadata(key,value) VALUES (?1,'1') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [REFRESH_CADENCE_V5_METADATA_KEY])?;
        tx.commit()?;
    }
    if validate_app_settings(&settings).is_err() { return Ok(AppSettings::default()); }
    settings.normalize_total_line_categories();
    let stored: serde_json::Value = serde_json::from_str(&value).unwrap_or_default();
    if stored.get("total_line_categories") != Some(&serde_json::to_value(&settings.total_line_categories)?) {
        db.set_metadata(APP_SETTINGS_METADATA_KEY, &serde_json::to_string(&settings)?)?;
    }
    Ok(settings)
}

pub fn validate_app_settings(settings: &AppSettings) -> AppResult<()> {
    if !REPO_REFRESH_INTERVALS.contains(&settings.activity_refresh_minutes) {
        return Err(AppError::InvalidArgument("activity_refresh_minutes must be hourly, daily, weekly, or monthly".into()));
    }
    if !(1..=1_440).contains(&settings.personal_refresh_minutes) {
        return Err(AppError::InvalidArgument("personal_refresh_minutes must be a whole number from 1 to 1440".into()));
    }
    if !(1..=43_200).contains(&settings.lines_refresh_minutes) {
        return Err(AppError::InvalidArgument("lines_refresh_minutes must be a whole number from 1 to 43200".into()));
    }
    Ok(())
}

pub fn save_app_settings(db: &Database, settings: &AppSettings) -> AppResult<()> {
    validate_app_settings(settings)?;
    let previous = app_settings(db)?;
    let mut settings = settings.clone();
    settings.normalize_menu_bar_metrics();
    settings.normalize_total_line_categories();
    db.set_metadata(APP_SETTINGS_METADATA_KEY, &serde_json::to_string(&settings)?)?;
    db.set_metadata(REFRESH_CADENCE_V2_METADATA_KEY, "1")?;
    db.set_metadata(REFRESH_CADENCE_V3_METADATA_KEY, "1")?;
    db.set_metadata(REFRESH_CADENCE_V4_METADATA_KEY, "1")?;
    db.set_metadata(REFRESH_CADENCE_V5_METADATA_KEY, "1")?;
    if (!previous.include_personal_repositories && settings.include_personal_repositories)
        || (!previous.include_company_repositories && settings.include_company_repositories) {
        db.set_metadata("github_discovered_at", "")?;
    }
    Ok(())
}

/// Decide whether the scheduled repository refresh should inspect LOC.
/// This function is intentionally independent of the database and wall clock
/// access so cadence behavior can be tested without invoking GitHub or Git.
pub fn decide_loc_sync(repo: &Repository, now: DateTime<Utc>, last_sweep_at: Option<&str>, force_full: bool) -> LocSyncDecision {
    decide_loc_sync_with_settings(repo, now, last_sweep_at, force_full, &AppSettings::default())
}

pub fn decide_loc_sync_with_settings(repo: &Repository, now: DateTime<Utc>, last_sweep_at: Option<&str>, force_full: bool, settings: &AppSettings) -> LocSyncDecision {
    if force_full {
        return LocSyncDecision { run: true, force_fetch: true };
    }
    let sweep_due = loc_sweep_due_with_interval(last_sweep_at, now, settings.activity_refresh_minutes);
    let first_scan = !repo.loc_backfill_complete || repo.local_path.is_none();
    let fetch_cursor_missing = repo.last_fetched_pushed_at.is_none() && repo.local_path.is_some();
    LocSyncDecision {
        run: first_scan || sweep_due,
        force_fetch: sweep_due || fetch_cursor_missing,
    }
}

pub fn loc_sweep_due(last_sweep_at: Option<&str>, now: DateTime<Utc>) -> bool {
    loc_sweep_due_with_interval(last_sweep_at, now, AppSettings::default().activity_refresh_minutes)
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

    pub fn dashboard(&self) -> AppResult<Arc<crate::models::Dashboard>> {
        let mut cache = self.dashboard_cache.lock().map_err(|_| AppError::InvalidArgument("dashboard cache lock poisoned".into()))?;
        self.database().dashboard_cached(&mut cache)
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
    let settings = app_settings(&db)?;
    if !settings.include_personal_repositories && !settings.include_company_repositories { return db.repositories(); }
    github_sync::ensure_available(&db)?;
    let (user, mut discovered) = github_sync::discover_owned(&db, settings.include_personal_repositories)?;
    github_sync::ensure_available(&db)?;
    db.set_metadata("github_login", &user.login)?;
    db.set_metadata("org_discovery_errors", "")?;
    let mut org_errors = Vec::new();
    if settings.include_company_repositories {
        match github_sync::guarded(&db, github::list_organizations) {
            Ok(organizations) => {
                for organization in organizations {
                    if !app_settings(&db)?.include_company_repositories { break; }
                    match github_sync::guarded(&db, || github::list_repositories_for_owner(&organization)) {
                        Ok(repositories) => discovered.extend(repositories),
                        Err(error @ (AppError::RateLimited { .. } | AppError::Authentication)) => return Err(error),
                        Err(error) => org_errors.push(format!("{organization}: {error}")),
                    }
                }
            }
            Err(error @ (AppError::RateLimited { .. } | AppError::Authentication)) => return Err(error),
            Err(error) => org_errors.push(format!("organization discovery: {error}")),
        }
    }
    if !org_errors.is_empty() {
        db.set_metadata("org_discovery_errors", &org_errors.join("\n"))?;
    }
    let mut seen = BTreeSet::new();
    let discovered: Vec<_> = discovered.into_iter().filter(|repo| seen.insert(repo.github_id.clone())).collect();
    db.upsert_repositories(&discovered)?;
    for repo in &discovered { crate::kanban::remember_repository(&db, &repo.github_id)?; }
    if org_errors.is_empty() { db.set_metadata("github_discovered_at", &Utc::now().to_rfc3339())?; }
    db.repositories()
}

pub fn sync_all(state: &AppState) -> AppResult<SyncResult> {
    let settings = app_settings(&state.database())?;
    if !settings.include_personal_repositories && !settings.include_company_repositories {
        finish_progress(state, None);
        return Ok(SyncResult { ok: true, message: "No repository groups selected".into(), ..SyncResult::default() });
    }
    github_sync::ensure_available(&state.database())?;
    let mut result = SyncResult { ok: true, message: "Refresh complete".into(), ..SyncResult::default() };
    state.set_progress(SyncProgress { running: true, phase: "discovering".into(), message: "Discovering repositories".into(), ..SyncProgress::default() });

    let repos = match discover(state) {
        Ok(repos) => repos,
        Err(AppError::Authentication) => {
            result.errors.push(AppError::Authentication.to_string());
            return Ok(finish_authentication_failure(state, result));
        }
        Err(error) => {
            result.ok = false;
            result.errors.push(error.to_string());
            state.database().repositories()?
        }
    };
    if let Some(org_errors) = state.database().metadata("org_discovery_errors")?.filter(|_| app_settings(&state.database()).map(|settings| settings.include_company_repositories).unwrap_or(false)) {
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
    let selected_ids: BTreeSet<i64> = state.database().selected_repositories()?.iter().map(|repo| repo.id).collect();
    let mut active: Vec<Repository> = repos.into_iter().filter(|repo| selected_ids.contains(&repo.id)).collect();
    let last = state.database().metadata("github_last_attempted_repository")?.and_then(|s| s.parse::<i64>().ok());
    if let Some(position) = active.iter().position(|repo| Some(repo.id) == last) {
        let length = active.len();
        active.rotate_left((position + 1) % length);
    }
    let repository_total = active.len() as i64;
    state.set_progress(SyncProgress { running: true, phase: "syncing".into(), current: 0, total: repository_total, repository_current: 0, repository_total, message: "Refreshing GitHub activity and line counts".into(), ..SyncProgress::default() });
    for (index, repo) in active.into_iter().enumerate() {
        if !state.database().repository_enabled(&repo)? { continue; }
        if let Err(error) = github_sync::ensure_available(&state.database()) {
            result.ok = false;
            result.errors.push(error.to_string());
            break;
        }
        state.database().set_metadata("github_last_attempted_repository", &repo.id.to_string())?;
        state.set_progress(SyncProgress { running: true, phase: "syncing_repository".into(), current: index as i64, total: repository_total, repository_current: index as i64, repository_total, snapshot_current: 0, snapshot_total: 0, repository_name: Some(repo.name_with_owner.clone()), message: format!("Refreshing activity and line counts for {}", repo.name_with_owner), ..SyncProgress::default() });
        match sync_repo_data(state, &repo, true, true) {
            Ok(RepoSyncOutcome { prs, issues, snapshots, errors: repo_errors, loc_completed, authentication_failed }) => {
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
                if authentication_failed { return Ok(finish_authentication_failure(state, result)); }
            }
            Err(AppError::RepositoryUnavailable) => {
                if settings.kanban_enabled {
                    result.ok = false;
                    result.errors.push(format!("{}: Repository unavailable", repo.name_with_owner));
                }
            },
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
    record_refresh_timestamps(&state.database(), &mut result, true);
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

/// Scheduled repository refresh, including activity and line-count work for
/// repositories that are new, incomplete, or due for the periodic sweep.
pub fn sync_activity(state: &AppState) -> AppResult<SyncResult> {
    let settings = app_settings(&state.database())?;
    if !settings.include_personal_repositories && !settings.include_company_repositories {
        finish_progress(state, None);
        return Ok(SyncResult { ok: true, message: "No repository groups selected".into(), ..SyncResult::default() });
    }
    github_sync::ensure_available(&state.database())?;
    let mut result = SyncResult { ok: true, message: "PR & issue refresh complete".into(), ..SyncResult::default() };
    state.set_progress(SyncProgress { running: true, phase: "discovering_activity".into(), message: "Discovering repositories".into(), ..SyncProgress::default() });

    let cached = state.database().metadata("github_discovered_at")?.and_then(|s| s.parse::<DateTime<Utc>>().ok()).is_some_and(|t| Utc::now() - t < Duration::hours(1));
    let repos = match if cached { state.database().repositories() } else { discover(state) } {
        Ok(repos) => repos,
        Err(AppError::Authentication) => {
            result.errors.push(AppError::Authentication.to_string());
            return Ok(finish_authentication_failure(state, result));
        }
        Err(error) => {
            result.ok = false;
            result.errors.push(error.to_string());
            state.database().repositories()?
        }
    };
    if let Some(org_errors) = state.database().metadata("org_discovery_errors")?.filter(|_| app_settings(&state.database()).map(|settings| settings.include_company_repositories).unwrap_or(false)) {
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
    let selected_ids: BTreeSet<i64> = state.database().selected_repositories()?.iter().map(|repo| repo.id).collect();
    let mut active: Vec<Repository> = repos.into_iter().filter(|repo| selected_ids.contains(&repo.id)).collect();
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
    let sweep_due = loc_sweep_due_with_interval(last_sweep_at.as_deref(), now, settings.activity_refresh_minutes);
    state.set_progress(SyncProgress { running: true, phase: "syncing_activity".into(), current: 0, total: repository_total, repository_current: 0, repository_total, snapshot_current: 0, snapshot_total: 0, message: if sweep_due { "Refreshing PRs, issues, and due line-count checks".into() } else { "Refreshing PRs and issues".into() }, ..SyncProgress::default() });

    for (index, repo) in active.into_iter().enumerate() {
        if !state.database().repository_enabled(&repo)? { continue; }
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
        state.set_progress(SyncProgress { running: true, phase: if decision.run { "syncing_repository" } else { "syncing_activity" }.into(), current: index as i64, total: repository_total, repository_current: index as i64, repository_total, snapshot_current: 0, snapshot_total: 0, repository_name: Some(repo.name_with_owner.clone()), message: if decision.run { format!("Refreshing PRs, issues, and line counts for {}", repo.name_with_owner) } else { format!("Refreshing PRs and issues for {}", repo.name_with_owner) }, ..SyncProgress::default() });
        match sync_repo_data(state, &repo, decision.run, decision.force_fetch) {
            Ok(RepoSyncOutcome { prs, issues, snapshots, errors: repo_errors, loc_completed, authentication_failed }) => {
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
                if authentication_failed { return Ok(finish_authentication_failure(state, result)); }
            }
            Err(AppError::RepositoryUnavailable) => {
                if settings.kanban_enabled {
                    result.ok = false;
                    result.errors.push(format!("{}: Repository unavailable", repo.name_with_owner));
                }
            },
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
        result.message = "PR & issue refresh completed with some errors".into();
    } else if result.loc_repositories_synced > 0 {
        result.message = format!("PR & issue refresh complete; line counts checked for {} repositories", result.loc_repositories_synced);
    }
    // A completed Repo Refresh covers repository activity and every line-count
    // check due on this cadence, so it is the broad refresh shown in the header.
    record_refresh_timestamps(&state.database(), &mut result, true);
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

/// Explicit GitHub activity refresh. This path never evaluates LOC decisions,
/// touches clones, or writes line-analysis timestamps.
pub fn sync_work_items(state: &AppState) -> AppResult<SyncResult> {
    let db = state.database();
    let settings = app_settings(&db)?;
    if !settings.include_personal_repositories && !settings.include_company_repositories {
        finish_work_item_progress(state, None);
        return Ok(SyncResult { ok: true, message: "No repository groups selected".into(), ..SyncResult::default() });
    }
    let mut result = SyncResult { ok: true, message: "PRs and issues refreshed".into(), ..SyncResult::default() };
    state.set_progress(SyncProgress { running: true, phase: "discovering_work_items".into(), message: "Finding tracked repositories for PRs and issues".into(), ..SyncProgress::default() });
    let user = match github_sync::ensure_available(&db).and_then(|_| github_sync::guarded(&db, github::current_user)) {
        Ok(user) => user,
        Err(error) => {
            result.ok = false;
            result.message = "GitHub sign-in or API access is unavailable".into();
            result.errors.push(error.to_string());
            finish_work_item_progress(state, result.errors.first().cloned());
            return Ok(result);
        }
    };
    // Refresh the current login before querying cached items. A login switch
    // must not make the previous account's board appear under the new account.
    let previous_login = db.metadata("github_login")?;
    db.set_metadata("github_login", &user.login)?;
    let cached = previous_login.as_deref().is_some_and(|old| old.eq_ignore_ascii_case(&user.login))
        && db.metadata("github_discovered_at")?.and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .is_some_and(|t| Utc::now() - t < Duration::minutes(settings.activity_refresh_minutes.clamp(15, 60)));
    let repos = match if cached { db.repositories() } else { discover(state) } {
        Ok(repos) => repos,
        Err(AppError::Authentication) => {
            result.ok = false;
            result.message = "GitHub CLI is not authenticated".into();
            result.errors.push(AppError::Authentication.to_string());
            finish_work_item_progress(state, result.errors.last().cloned());
            return Ok(result);
        }
        Err(error) => { result.ok = false; result.errors.push(format!("Repository discovery: {error}")); db.repositories()? }
    };
    if let Err(error) = github_sync::ensure_available(&db) {
        result.ok = false;
        result.errors.push(error.to_string());
        result.message = "GitHub requests are paused".into();
        finish_work_item_progress(state, result.errors.first().cloned());
        return Ok(result);
    }
    let account_repo_ids: BTreeSet<String> = crate::kanban::account_repository_ids(&db)?;
    let selected_ids: BTreeSet<i64> = db.selected_repositories()?.iter().map(|repo| repo.id).collect();
    let mut active: Vec<Repository> = repos.into_iter()
        .filter(|repo| selected_ids.contains(&repo.id) && account_repo_ids.contains(&repo.github_id)).collect();
    let last = db.metadata("github_last_attempted_repository")?.and_then(|s| s.parse::<i64>().ok());
    if let Some(position) = active.iter().position(|repo| Some(repo.id) == last) {
        let length = active.len();
        active.rotate_left((position + 1) % length);
    }
    let total = active.len() as i64;
    state.set_progress(SyncProgress { running: true, phase: "syncing_work_items".into(), total, repository_total:total, message: "Refreshing PRs and issues across all tracked repositories".into(), ..SyncProgress::default() });
    for (index, repo) in active.into_iter().enumerate() {
        if !db.repository_enabled(&repo)? { continue; }
        if let Err(error) = github_sync::ensure_available(&db) {
            result.ok = false;
            result.errors.push(error.to_string());
            break;
        }
        db.set_metadata("github_last_attempted_repository", &repo.id.to_string())?;
        state.set_progress(SyncProgress { running:true, phase:"syncing_work_items".into(), current:index as i64, total, repository_current:index as i64, repository_total:total, repository_name:Some(repo.name_with_owner.clone()), message:format!("Refreshing PRs and issues for {}",repo.name_with_owner), ..SyncProgress::default() });
        let mut counts = [0,0];
        match github_sync::sync_activity_reporting(&db, &repo, &mut counts) {
            Ok((_,_,complete)) => {
                result.pull_requests_synced += counts[0];
                result.issues_synced += counts[1];
                if complete { result.repositories_synced += 1; result.activity_repositories_synced += 1; }
                else { result.ok = false; result.errors.push(format!("{}: Import continues on the next refresh",repo.name_with_owner)); }
                crate::kanban::record_activity(&db,&repo.github_id,complete,None)?;
            }
            Err(error) => {
                result.pull_requests_synced += counts[0];
                result.issues_synced += counts[1];
                result.ok = false;
                result.errors.push(format!("{}: {error}",repo.name_with_owner));
                crate::kanban::record_activity(&db,&repo.github_id,false,Some(&error.to_string()))?;
                if matches!(error, AppError::Authentication) {
                    result.message = "GitHub CLI is not authenticated".into();
                    finish_work_item_progress(state, result.errors.last().cloned());
                    return Ok(result);
                }
            }
        }
        let mut progress = state.progress();
        progress.current = index as i64 + 1;
        progress.repository_current = index as i64 + 1;
        progress.message = format!("Completed {}/{} repositories; {} PRs and {} issues updated", index+1,total,result.pull_requests_synced,result.issues_synced);
        state.set_progress(progress);
    }
    if !result.ok { result.message = "PRs and issues refreshed with partial results".into(); }
    record_refresh_timestamps(&db, &mut result, false);
    finish_work_item_progress(state, result.errors.first().cloned());
    Ok(result)
}

/// Frequent activity update for the personal feed and menu bar PR/issue totals.
/// Repository discovery and line analysis use the Repo Refresh schedule.
pub fn sync_personal_work_items(state: &AppState) -> AppResult<SyncResult> {
    let db = state.database();
    let mut result = SyncResult { ok: true, message: "Personal PRs and issues refreshed".into(), ..SyncResult::default() };
    state.set_progress(SyncProgress { running: true, phase: "syncing_personal_work_items".into(), message: "Refreshing your PRs and issues".into(), ..SyncProgress::default() });
    let user = match github_sync::ensure_available(&db).and_then(|_| github_sync::guarded(&db, github::current_user)) {
        Ok(user) => user,
        Err(error) => {
            result.ok = false;
            result.errors.push(error.to_string());
            result.message = "Personal PR and issue refresh unavailable".into();
            finish_work_item_progress(state, result.errors.first().cloned());
            return Ok(result);
        }
    };
    let repos = (|| {
        db.set_metadata("github_login", &user.login)?;
        // Search results carry a stable repository ID, so only returned items
        // can match this tracked selection. This also works with an older
        // cache before the next repository discovery populates account scope.
        Ok::<_, AppError>(db.selected_repositories()?)
    })();
    let repos = match repos {
        Ok(repos) => repos,
        Err(error) => {
            result.ok = false;
            result.errors.push(error.to_string());
            result.message = "Personal PR and issue refresh unavailable".into();
            finish_work_item_progress(state, result.errors.first().cloned());
            return Ok(result);
        }
    };
    if repos.is_empty() {
        result.message = "No tracked repositories are available for this account".into();
        finish_work_item_progress(state, None);
        return Ok(result);
    }
    match github_sync::sync_personal_activity(&db, &user.login, &repos) {
        Ok(report) => {
            result.pull_requests_synced = report.pull_requests;
            result.issues_synced = report.issues;
            if report.complete {
                if let Err(error) = db.set_metadata(LAST_PERSONAL_REFRESH_METADATA_KEY, &Utc::now().to_rfc3339()) {
                    result.ok = false;
                    result.errors.push(format!("Personal refresh timestamp: {error}"));
                }
            } else {
                result.ok = false;
                result.errors.push("Personal ticket search or assignment checks have more work; they will continue on the next refresh".into());
            }
        }
        Err(error) => {
            result.ok = false;
            result.errors.push(error.to_string());
            if matches!(error, AppError::Authentication) {
                result.message = "GitHub CLI is not authenticated".into();
                finish_work_item_progress(state, result.errors.last().cloned());
                return Ok(result);
            }
        }
    }
    // The menu bar uses repository totals, which otherwise remain stale until
    // the much slower Repo Refresh even while personal search updates the list.
    if let Err(error) = github_sync::sync_open_counts(&db, &repos) {
        result.ok = false;
        result.errors.push(format!("Open PR and issue totals: {error}"));
    }
    if !result.ok { result.message = "Personal PR and issue refresh has partial results".into(); }
    finish_work_item_progress(state, result.errors.first().cloned());
    Ok(result)
}

fn record_refresh_timestamps(db: &Database, result: &mut SyncResult, full: bool) {
    let now = Utc::now().to_rfc3339();
    let fields = [
        (LAST_LOC_REFRESH_METADATA_KEY, result.loc_repositories_synced > 0),
        (LAST_ACTIVITY_REFRESH_METADATA_KEY, result.ok && result.activity_repositories_synced > 0),
        (LAST_PERSONAL_REFRESH_METADATA_KEY, result.ok && result.activity_repositories_synced > 0),
        (LAST_FULL_REFRESH_METADATA_KEY, result.ok && full),
    ];
    for (key, enabled) in fields {
        if enabled {
            if let Err(error) = db.set_metadata(key, &now) {
                result.ok = false;
                result.errors.push(format!("Refresh timestamp: {error}"));
            }
        }
    }
}

fn finish_work_item_progress(state: &AppState, error: Option<String>) {
    let prior = state.progress();
    let personal = prior.phase == "syncing_personal_work_items";
    state.set_progress(SyncProgress {
        running:false,
        phase:if personal { "personal_work_items_complete" } else { "work_items_complete" }.into(),
        error:error.clone(),
        message:match (personal, error.is_some()) {
            (true, true) => "Your PR and issue refresh finished with errors",
            (true, false) => "Your PRs and issues refreshed",
            (false, true) => "PR and issue refresh finished with errors",
            (false, false) => "PRs and issues refreshed",
        }.into(),
        ..prior
    });
}

pub fn sync_one(state: &AppState, repository_id: i64) -> AppResult<SyncResult> {
    let repo = state.database().repository(repository_id)?.ok_or(AppError::RepositoryNotFound(repository_id))?;
    if !state.database().repository_enabled(&repo)? {
        return Err(AppError::InvalidArgument("Repository is disabled in settings".into()));
    }
    github_sync::ensure_available(&state.database())?;
    state.set_progress(SyncProgress { running: true, phase: "syncing_repository".into(), current: 0, total: 1, repository_current: 0, repository_total: 1, repository_name: Some(repo.name_with_owner.clone()), message: format!("Refreshing activity and line counts for {}", repo.name_with_owner), ..SyncProgress::default() });
    let mut result = SyncResult { ok: true, message: "Repository refresh complete".into(), ..SyncResult::default() };
    match sync_repo_data(state, &repo, true, true) {
        Ok(RepoSyncOutcome { prs, issues, snapshots, errors: repo_errors, loc_completed, authentication_failed }) => {
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
            if authentication_failed { return Ok(finish_authentication_failure(state, result)); }
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
    record_refresh_timestamps(&state.database(), &mut result, false);
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

pub fn backfill_one(state: &AppState, repository_id: i64) -> AppResult<SyncResult> {
    let repo = state.database().repository(repository_id)?.ok_or(AppError::RepositoryNotFound(repository_id))?;
    if !state.database().repository_enabled(&repo)? {
        return Err(AppError::InvalidArgument("Repository is disabled in settings".into()));
    }
    github_sync::ensure_available(&state.database())?;
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
    record_refresh_timestamps(&state.database(), &mut result, false);
    finish_progress(state, result.errors.first().cloned());
    Ok(result)
}

fn finish_authentication_failure(state: &AppState, mut result: SyncResult) -> SyncResult {
    result.ok = false;
    result.message = "GitHub CLI is not authenticated".into();
    finish_progress(state, result.errors.last().cloned());
    result
}

struct RepoSyncOutcome {
    prs: i64,
    issues: i64,
    snapshots: i64,
    errors: Vec<String>,
    loc_completed: bool,
    authentication_failed: bool,
}

fn sync_repo_data(state: &AppState, repo: &Repository, run_loc: bool, force_fetch: bool) -> AppResult<RepoSyncOutcome> {
    let db = state.database();
    github_sync::ensure_available(&db)?;
    let mut counts = [0, 0];
    let mut errors = Vec::new();
    let mut authentication_failed = false;
    let activity_fetched = match github_sync::sync_activity_reporting(&db, repo, &mut counts) {
        Ok((_, _, complete)) => {
            if app_settings(&db)?.kanban_enabled { crate::kanban::record_activity(&db, &repo.github_id, complete, None)?; }
            if !complete { errors.push("Activity import is continuing next cycle".into()); }
            true
        }
        Err(error @ AppError::RepositoryUnavailable) => {
            if app_settings(&db)?.kanban_enabled {
                crate::kanban::record_activity(&db, &repo.github_id, false, Some("Repository unavailable"))?;
            } else {
                db.delete_repository(repo)?;
            }
            return Err(error);
        }
        Err(error) => {
            if app_settings(&db)?.kanban_enabled { crate::kanban::record_activity(&db, &repo.github_id, false, Some(&error.to_string()))?; }
            authentication_failed = matches!(error, AppError::Authentication);
            errors.push(error.to_string()); false
        }
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
    Ok(RepoSyncOutcome { prs: counts[0], issues: counts[1], snapshots, errors, loc_completed, authentication_failed })
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
        let empty = Snapshot { id: 0, repository_id: repo.id, commit_sha: "EMPTY_REPOSITORY".into(), commit_date: now.to_rfc3339(), snapshot_date: now.to_rfc3339(), total_loc: 0, source_loc: 0, test_loc: 0, docs_loc: 0, created_at: now.to_rfc3339() };
        let (recovered, errors) = recover_loc_rebuild_at_path(&db, repo, &path, &config)?;
        let created = recovered + if db.upsert_snapshot(&empty)? { 1 } else { 0 };
        db.upsert_observation(&empty)?;
        if let Some(error) = errors.first() { return Err(AppError::InvalidArgument(error.clone())); }
        db.set_backfill_complete(repo.id, true)?;
        return Ok(created);
    }
    if !repo.loc_backfill_complete || !db.pending_loc_rebuild(repo.id)?.is_empty() {
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
        let snapshot = Snapshot { id: 0, repository_id: repo.id, commit_sha: sha.clone(), commit_date, snapshot_date: Utc::now().to_rfc3339(), total_loc: scan.total_loc, source_loc: scan.source_loc, test_loc: scan.test_loc, docs_loc: scan.docs_loc, created_at: Utc::now().to_rfc3339() };
        if db.upsert_snapshot(&snapshot)? { created += 1; }
        measurement = Some(snapshot);
    }
    let now = Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let latest_observation = db.latest_observation(repo.id)?;
    let should_observe = latest_observation.as_ref().map(|item| item.commit_sha != sha || item.snapshot_date.get(..10).unwrap_or(&item.snapshot_date) != today.as_str()).unwrap_or(true);
    if should_observe {
        if let Some(measurement) = measurement {
            let observation = Snapshot { id: 0, repository_id: repo.id, commit_sha: measurement.commit_sha, commit_date: measurement.commit_date, snapshot_date: now.to_rfc3339(), total_loc: measurement.total_loc, source_loc: measurement.source_loc, test_loc: measurement.test_loc, docs_loc: measurement.docs_loc, created_at: now.to_rfc3339() };
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
        let empty = Snapshot { id: 0, repository_id: repo.id, commit_sha: "EMPTY_REPOSITORY".into(), commit_date: now.to_rfc3339(), snapshot_date: now.to_rfc3339(), total_loc: 0, source_loc: 0, test_loc: 0, docs_loc: 0, created_at: now.to_rfc3339() };
        let config = db.classification_config(repo.id)?;
        let (recovered, errors) = recover_loc_rebuild_at_path(&db, repo, &path, &config)?;
        let created = recovered + if db.upsert_snapshot(&empty)? { 1 } else { 0 };
        db.upsert_observation(&empty)?;
        if let Some(error) = errors.first() { return Err(AppError::InvalidArgument(error.clone())); }
        db.set_backfill_complete(repo.id, true)?;
        return Ok(created);
    }
    let created = backfill_repo_at_path(state, repo, &path)?;
    if created >= 0 { state.database().set_backfill_complete(repo.id, true)?; }
    Ok(created)
}

/// Recover dated measurements before monthly backfill. New snapshots are also
/// the durable per-commit cache, so repeated days and retries scan each SHA once.
fn recover_loc_rebuild_at_path(db: &Database, repo: &Repository, path: &Path, config: &crate::models::ClassificationConfig) -> AppResult<(i64, Vec<String>)> {
    let pending = db.pending_loc_rebuild(repo.id)?;
    if pending.is_empty() { return Ok((0, Vec::new())); }
    let can_scan = current_commit_optional(path)?.is_some();
    let mut failed = BTreeSet::new();
    let mut errors = Vec::new();
    let mut created = 0;
    for item in pending {
        if failed.contains(&item.commit_sha) { continue; }
        let measurement = if let Some(cached) = db.snapshot_for_commit(repo.id, &item.commit_sha)? {
            cached
        } else {
            let scan = if item.commit_sha == "EMPTY_REPOSITORY" {
                crate::gitops::LocScan::default()
            } else if can_scan {
                match scan_at_commit_detached(path, &item.commit_sha, Some(config)) {
                    Ok(scan) => scan,
                    Err(error) => {
                        errors.push(format!("Could not rebuild saved measurement {}: {error}", item.commit_sha));
                        failed.insert(item.commit_sha.clone());
                        continue;
                    }
                }
            } else {
                errors.push(format!("Could not rebuild saved measurement {} from an empty repository", item.commit_sha));
                failed.insert(item.commit_sha.clone());
                continue;
            };
            let commit_date = if item.kind == "snapshot" || item.commit_sha == "EMPTY_REPOSITORY" {
                item.commit_date.clone()
            } else {
                current_commit(path)?.1
            };
            let snapshot = Snapshot {
                repository_id: repo.id, commit_sha: item.commit_sha.clone(), commit_date,
                snapshot_date: item.snapshot_date.clone(), created_at: item.created_at.clone(),
                total_loc: scan.total_loc, source_loc: scan.source_loc, test_loc: scan.test_loc, docs_loc: scan.docs_loc,
                ..Snapshot::default()
            };
            if db.upsert_snapshot(&snapshot)? { created += 1; }
            snapshot
        };
        // The restored row and queue removal commit together. A database error
        // leaves the item pending and its scanned SHA cached for the next retry.
        db.restore_loc_rebuild(&item, &measurement)?;
    }
    Ok((created, errors))
}

fn backfill_repo_at_path(state: &AppState, repo: &Repository, path: &Path) -> AppResult<i64> {
    let result = (|| {
        let history_start = earliest_commit(path, &repo.default_branch)?
            .map(|(_, date)| date)
            .or_else(|| repo.created_at.clone());
        let dates = month_sample_dates(history_start.as_deref(), Utc::now());
        let prior = state.progress();
        state.set_progress(SyncProgress { running: true, phase: "backfilling".into(), current: prior.current, total: prior.total, repository_current: prior.repository_current, repository_total: prior.repository_total, snapshot_current: 0, snapshot_total: dates.len() as i64, repository_name: Some(repo.name_with_owner.clone()), message: "Selecting monthly commits".into(), ..SyncProgress::default() });
        let db = state.database();
        let config = db.classification_config(repo.id)?;
        let (mut created, mut errors) = recover_loc_rebuild_at_path(&db, repo, path, &config)?;
        for (index, sample_date) in dates.iter().enumerate() {
            state.set_progress(SyncProgress { running: true, phase: "backfilling".into(), current: prior.current, total: prior.total, repository_current: prior.repository_current, repository_total: prior.repository_total, snapshot_current: index as i64, snapshot_total: dates.len() as i64, repository_name: Some(repo.name_with_owner.clone()), message: format!("Analysing snapshot {}/{}", index + 1, dates.len()), ..SyncProgress::default() });
            let Some((sha, commit_date)) = commit_at_or_before_nonempty(path, &repo.default_branch, sample_date)? else {
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
            match scan_at_commit_detached(path, &sha, Some(&config)) {
                Ok(scan) => {
                    let snapshot = Snapshot { id: 0, repository_id: repo.id, commit_sha: sha, commit_date, snapshot_date: sample_date.clone(), total_loc: scan.total_loc, source_loc: scan.source_loc, test_loc: scan.test_loc, docs_loc: scan.docs_loc, created_at: Utc::now().to_rfc3339() };
                    if db.upsert_snapshot(&snapshot)? { created += 1; }
                }
                Err(error) => errors.push(error.to_string()),
            }
            let mut progress = state.progress();
            progress.snapshot_current = index as i64 + 1;
            state.set_progress(progress);
        }
        if let Some(error) = errors.first() { return Err(AppError::Command { program: "tokei".into(), message: error.clone() }); }
        Ok(created)
    })();
    // Restore even when selecting a commit, saving a sample, or scanning fails.
    let restored = crate::gitops::checkout_branch(path, &repo.default_branch);
    result.and_then(|created| restored.map(|_| created))
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
