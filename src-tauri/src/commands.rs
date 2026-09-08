use crate::error;
use crate::models::{ActivityFeed, AppSettings, ClassificationConfig, Dashboard, GithubUser, LocHistory, RepositorySummary, SyncProgress, SyncResult};
use crate::sync::{self, AppState};
use chrono::Utc;
use tauri::State;

#[tauri::command(rename_all = "snake_case")]
pub async fn check_dependencies() -> crate::models::DependencyStatus {
    tokio::task::spawn_blocking(crate::github::dependency_status).await.unwrap_or_default()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_github_user(state: State<'_, AppState>) -> Result<GithubUser, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        crate::github::current_user().or_else(|_| {
            state.database().metadata("github_login")
                .map_err(|error| error.to_string())?
                .map(|login| GithubUser { login })
                .ok_or_else(|| "GitHub CLI is not authenticated".to_string())
        })
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn discover_repositories(state: State<'_, AppState>) -> Result<Vec<RepositorySummary>, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        let _ = sync::discover(&state).map_err(|error| error.to_string())?;
        state.database().summaries().map(|items| items.into_iter().filter(|repo| !repo.is_archived).collect()).map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sync_github_data(state: State<'_, AppState>) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        sync::sync_all(&state).map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sync_activity(state: State<'_, AppState>) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        sync::sync_activity(&state).map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_app_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    sync::app_settings(&state.database()).map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_app_settings(state: State<'_, AppState>, settings: AppSettings) -> Result<AppSettings, String> {
    sync::save_app_settings(&state.database(), &settings).map_err(|error| error.to_string())?;
    Ok(settings)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sync_repository(state: State<'_, AppState>, repo_id: i64) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        sync::sync_one(&state, repo_id).map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn backfill_loc(state: State<'_, AppState>, repo_id: i64) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        sync::backfill_one(&state, repo_id).map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_sync_progress(state: State<'_, AppState>) -> SyncProgress {
    state.progress()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_repository_classification(state: State<'_, AppState>, repository_id: i64) -> Result<ClassificationConfig, String> {
    state.database().classification_config(repository_id).map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_repository_classification(state: State<'_, AppState>, repository_id: i64, config: ClassificationConfig) -> Result<bool, String> {
    let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
    state.database().set_classification_config(repository_id, &config).map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_dashboard(state: State<'_, AppState>) -> Result<Dashboard, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let db = state.database();
        let summaries: Vec<RepositorySummary> = db.summaries().map_err(|error| error.to_string())?.into_iter().filter(|repo| !repo.is_archived).collect();
        let totals = db.totals(&summaries);
        let history = db.history(None).map_err(|error| error.to_string())?;
        let last_sync_at = summaries.iter().filter_map(|repo| repo.last_sync_at.clone()).max();
        let user = db.metadata("github_login").map_err(|error| error.to_string())?.map(|login| GithubUser { login });
        let errors = db.metadata("org_discovery_errors").map_err(|error| error.to_string())?.map(|value| value.lines().map(str::to_string).collect()).unwrap_or_default();
        Ok(Dashboard { user, repositories: summaries, totals, history, last_sync_at, errors })
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_loc_history(state: State<'_, AppState>, repository_id: Option<i64>, range: Option<String>) -> Result<LocHistory, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let range_name = range.clone().unwrap_or_else(|| "all".into());
        let points = state.database().history(repository_id).map_err(|error| error.to_string())?;
        let start = crate::classify::range_start(range.as_deref(), Utc::now());
        let filtered = if let Some(start) = start {
            let mut before = None;
            let mut selected = Vec::new();
            for point in points {
                if point.snapshot_date <= start { before = Some(point); } else { selected.push(point); }
            }
            if let Some(anchor) = before { selected.insert(0, anchor); }
            selected
        } else { points };
        Ok(LocHistory { repository_id, range: range_name, points: filtered })
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_activity_feed(app_state: State<'_, AppState>, kind: String, state: Option<String>, repository_id: Option<i64>, limit: Option<usize>) -> Result<ActivityFeed, String> {
    let state_value = state.as_deref().and_then(|value| if value.eq_ignore_ascii_case("all") { None } else { Some(value) });
    let items = app_state.database().all_activity(&kind, repository_id, state_value, limit.unwrap_or(100).min(1000)).map_err(|error| error.to_string())?;
    Ok(ActivityFeed { kind, items })
}

#[tauri::command(rename_all = "snake_case")]
pub fn open_external_url(url: String) -> Result<bool, String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) { return Err(error::AppError::InvalidUrl.to_string()); }
    open::that(url).map(|_| true).map_err(|error| error.to_string())
}
