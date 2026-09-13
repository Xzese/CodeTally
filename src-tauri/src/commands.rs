use crate::error;
use crate::models::{ActivityFeed, AppInfo, AppSettings, ClassificationConfig, Dashboard, GithubUser, LocHistory, RepositorySummary, SyncProgress, SyncResult};
use crate::sync::{self, AppState};
use chrono::Utc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;
use tauri_plugin_updater::UpdaterExt;

const APP_REPOSITORY_URL: &str = "https://github.com/Xzese/CodeTally";
static UPDATE_INSTALLING: AtomicBool = AtomicBool::new(false);

struct UpdateInstallGuard;

impl Drop for UpdateInstallGuard {
    fn drop(&mut self) {
        UPDATE_INSTALLING.store(false, Ordering::Release);
    }
}

fn begin_update_install() -> Result<UpdateInstallGuard, String> {
    UPDATE_INSTALLING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| UpdateInstallGuard)
        .map_err(|_| "An update is already being installed.".to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn check_dependencies() -> crate::models::DependencyStatus {
    tokio::task::spawn_blocking(crate::github::dependency_status).await.unwrap_or_default()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn check_for_updates(app: tauri::AppHandle) -> Result<crate::updates::UpdateCheck, String> {
    let current_version = app.package_info().version.to_string();
    tokio::task::spawn_blocking(move || crate::updates::check_for_updates(&current_version))
        .await
        .map_err(|error| format!("update check failed: {error}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let _install_guard = begin_update_install()?;
    let update = app
        .updater()
        .map_err(|error| format!("Could not start the updater: {error}"))?
        .check()
        .await
        .map_err(|error| format!("Could not find a downloadable update: {error}"))?
        .ok_or_else(|| "No newer update is available for this Mac.".to_string())?;

    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| format!("Could not install the update: {error}"))?;

    app.restart();
}

#[cfg(test)]
mod update_install_tests {
    use super::begin_update_install;

    #[test]
    fn update_install_guard_prevents_overlap_and_releases_after_failure() {
        let first = begin_update_install().expect("first update should start");
        assert_eq!(
            begin_update_install().err().as_deref(),
            Some("An update is already being installed.")
        );
        drop(first);
        assert!(begin_update_install().is_ok());
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_github_user(state: State<'_, AppState>) -> Result<GithubUser, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let database = state.database();
        match crate::github_sync::guarded(&database, crate::github::current_user) {
            Ok(user) => {
                database.set_metadata("github_login", &user.login).map_err(|error| error.to_string())?;
                Ok(user)
            }
            Err(_) => database.metadata("github_login")
                .map_err(|error| error.to_string())?
                .map(|login| GithubUser { login })
                .ok_or_else(|| "GitHub CLI is not authenticated".to_string()),
        }
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn discover_repositories(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Vec<RepositorySummary>, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        let result = sync::discover(&state).map_err(|error| error.to_string());
        drop(_job);
        crate::native::refresh_menu(&app, &state);
        result?;
        state.database().summaries().map(|items| items.into_iter().filter(|repo| !repo.is_archived).collect()).map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sync_github_data(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        let result = sync::sync_all(&state).map_err(|error| error.to_string());
        drop(_job);
        crate::native::refresh_menu(&app, &state);
        result
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sync_activity(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        let result = sync::sync_activity(&state).map_err(|error| error.to_string());
        drop(_job);
        crate::native::refresh_menu(&app, &state);
        result
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_app_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    sync::app_settings(&state.database()).map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_app_info(app: tauri::AppHandle) -> AppInfo {
    let package_info = app.package_info();
    AppInfo {
        name: package_info.name.clone(),
        version: package_info.version.to_string(),
        identifier: app.config().identifier.clone(),
        repository_url: APP_REPOSITORY_URL.to_string(),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_repository_selection(state: State<'_, AppState>) -> Result<Vec<crate::models::RepositorySelection>, String> {
    state.database().repository_selection().map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_app_settings(app: tauri::AppHandle, state: State<'_, AppState>, mut settings: AppSettings) -> Result<AppSettings, String> {
    settings.normalize_menu_bar_metrics();
    sync::save_app_settings(&state.database(), &settings).map_err(|error| error.to_string())?;
    crate::native::apply_activation_policy(&app, &settings).map_err(|error| error.to_string())?;
    crate::native::refresh_menu(&app, &state);
    Ok(settings)
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_database_location(state: State<'_, AppState>) -> Result<String, String> {
    tauri::Url::from_file_path(&state.db_path)
        .map(|url| url.to_string())
        .map_err(|_| "Could not create a file URL for the SQLite database.".to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub fn reveal_database(state: State<'_, AppState>) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .arg("-R")
            .arg(&state.db_path)
            .status()
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(true)
        } else {
            Err(format!("Finder could not reveal the SQLite database (status {status})."))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let parent = state.db_path.parent().ok_or_else(|| "The SQLite database has no containing directory.".to_string())?;
        open::that(parent).map(|_| true).map_err(|error| error.to_string())
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sync_repository(app: tauri::AppHandle, state: State<'_, AppState>, repo_id: i64) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        let result = sync::sync_one(&state, repo_id).map_err(|error| error.to_string());
        drop(_job);
        crate::native::refresh_menu(&app, &state);
        result
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn backfill_loc(app: tauri::AppHandle, state: State<'_, AppState>, repo_id: i64) -> Result<SyncResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
        let result = sync::backfill_one(&state, repo_id).map_err(|error| error.to_string());
        drop(_job);
        crate::native::refresh_menu(&app, &state);
        result
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
pub fn set_repository_classification(app: tauri::AppHandle, state: State<'_, AppState>, repository_id: i64, config: ClassificationConfig) -> Result<bool, String> {
    let _job = state.job_lock.lock().map_err(|_| "sync lock poisoned".to_string())?;
    state.database().set_classification_config(repository_id, &config).map_err(|error| error.to_string())?;
    drop(_job);
    crate::native::refresh_menu(&app, &state);
    Ok(true)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_dashboard(state: State<'_, AppState>) -> Result<Dashboard, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let db = state.database();
        let summaries: Vec<RepositorySummary> = db.summaries().map_err(|error| error.to_string())?.into_iter().filter(|repo| !repo.is_archived).collect();
        let totals = db.totals(&summaries).map_err(|error| error.to_string())?;
        let history = db.history(None).map_err(|error| error.to_string())?;
        let last_sync_at = summaries.iter().filter_map(|repo| repo.last_sync_at.clone()).max();
        let user = db.metadata("github_login").map_err(|error| error.to_string())?.map(|login| GithubUser { login });
        let errors = db.metadata("org_discovery_errors").map_err(|error| error.to_string())?.filter(|_| sync::app_settings(&db).map(|settings| settings.include_company_repositories).unwrap_or(false)).map(|value| value.lines().map(str::to_string).collect()).unwrap_or_default();
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
pub fn get_activity_feed(app_state: State<'_, AppState>, kind: String, state: Option<String>, repository_id: Option<i64>, repository_ids: Option<Vec<i64>>, limit: Option<usize>, relationship: Option<String>) -> Result<ActivityFeed, String> {
    let state_value = state.as_deref().and_then(|value| if value.eq_ignore_ascii_case("all") { None } else { Some(value) });
    let relationship = relationship.unwrap_or_else(|| "everyone".into()).to_ascii_lowercase();
    if !matches!(relationship.as_str(), "everyone" | "author" | "assignee" | "author_or_assignee") {
        return Err(error::AppError::InvalidArgument("relationship must be one of everyone, author, assignee, author_or_assignee".into()).to_string());
    }
    let database = app_state.database();
    let login = if relationship == "everyone" {
        None
    } else {
        database.metadata("github_login").map_err(|error| error.to_string())?.filter(|login| !login.trim().is_empty())
    };
    let items = database.scoped_activity_for_relationship(&kind, repository_id, repository_ids.as_deref(), state_value, limit.unwrap_or(100).min(1000), &relationship, login.as_deref()).map_err(|error| error.to_string())?;
    Ok(ActivityFeed { kind, items })
}

#[tauri::command(rename_all = "snake_case")]
pub fn open_external_url(url: String) -> Result<bool, String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) { return Err(error::AppError::InvalidUrl.to_string()); }
    open::that(url).map(|_| true).map_err(|error| error.to_string())
}
