pub mod classify;
pub mod commands;
pub mod db;
pub mod error;
pub mod gitops;
pub mod github;
pub mod github_sync;
pub mod models;
pub mod native;
pub mod sync;

use models::SyncProgress;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use sync::AppState;
use tauri::Manager;

const APP_DATABASE_FILE: &str = "codetally.sqlite3";
const LEGACY_APP_DATA_DIRECTORY: &str = "com.samfaid.github-portfolio";
const LEGACY_DATABASE_FILE: &str = "github-portfolio.sqlite3";

/// Copy data from the pre-CodeTally application directory without replacing
/// files that already exist in the new location. Keeping this operation
/// copy-only makes repeated launches and interrupted migrations safe.
pub fn migrate_legacy_app_data(app_data_dir: &Path) -> io::Result<()> {
    let Some(parent) = app_data_dir.parent() else { return Ok(()); };
    let legacy_dir = parent.join(LEGACY_APP_DATA_DIRECTORY);
    if legacy_dir.as_path() == app_data_dir || !legacy_dir.is_dir() {
        return Ok(());
    }

    std::fs::create_dir_all(app_data_dir)?;
    let legacy_database = legacy_dir.join(LEGACY_DATABASE_FILE);
    let database = app_data_dir.join(APP_DATABASE_FILE);
    if legacy_database.is_file() && !database.exists() {
        // Copy SQLite sidecars before the database so a copied WAL is present
        // when the new connection is opened, while never overwriting a file
        // created by a prior launch.
        for suffix in ["-wal", "-shm"] {
            copy_file_if_missing(
                &legacy_dir.join(format!("{LEGACY_DATABASE_FILE}{suffix}")),
                &app_data_dir.join(format!("{APP_DATABASE_FILE}{suffix}")),
            )?;
        }
        copy_file_if_missing(&legacy_database, &database)?;
    }

    copy_directory_contents_if_missing(
        &legacy_dir.join("repositories"),
        &app_data_dir.join("repositories"),
    )?;
    Ok(())
}

fn copy_file_if_missing(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_file() || destination.exists() {
        return Ok(());
    }
    std::fs::copy(source, destination)?;
    Ok(())
}

fn copy_directory_contents_if_missing(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory_contents_if_missing(&source_path, &destination_path)?;
        } else if !destination_path.exists() {
            std::fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
            migrate_legacy_app_data(&app_data_dir).map_err(|error| error.to_string())?;
            std::fs::create_dir_all(&app_data_dir).map_err(|error| error.to_string())?;
            let cache_dir = app_data_dir.join("repositories");
            std::fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
            let db_path = app_data_dir.join(APP_DATABASE_FILE);
            let database = db::Database::new(&db_path);
            database.init().map_err(|error| error.to_string())?;
            if let Some(parent) = app_data_dir.parent() {
                let legacy_cache_dir = parent.join(LEGACY_APP_DATA_DIRECTORY).join("repositories");
                database
                    .rebase_local_paths(&legacy_cache_dir, &cache_dir)
                    .map_err(|error| error.to_string())?;
            }
            app.manage(AppState { db_path, cache_dir, progress: Arc::new(Mutex::new(SyncProgress::default())), job_lock: Arc::new(Mutex::new(())) });
            native::setup(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if sync::app_settings(&state.database()).map(|settings| settings.run_in_background).unwrap_or(true) {
                    if window.hide().is_ok() { api.prevent_close(); }
                } else {
                    window.app_handle().exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_dependencies,
            commands::get_github_user,
            commands::discover_repositories,
            commands::sync_github_data,
            commands::sync_activity,
            commands::get_app_settings,
            commands::get_repository_selection,
            commands::set_app_settings,
            commands::get_database_location,
            commands::reveal_database,
            commands::sync_repository,
            commands::backfill_loc,
            commands::get_sync_progress,
            commands::get_repository_classification,
            commands::set_repository_classification,
            commands::get_dashboard,
            commands::get_loc_history,
            commands::get_activity_feed,
            commands::open_external_url
        ])
        .build(tauri::generate_context!())
        .expect("error while building CodeTally")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                native::show_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
