use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("GitHub requests paused until {until}: {reason}")]
    RateLimited { until: String, reason: String },
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command {program} failed: {message}")]
    Command { program: String, message: String },
    #[error("repository {0} was not found")]
    RepositoryNotFound(i64),
    #[error("invalid URL")]
    InvalidUrl,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("path error: {0}")]
    Path(PathBuf),
}

pub type AppResult<T> = Result<T, AppError>;

pub fn command_error(program: &str, output: std::process::Output) -> AppError {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = if !stderr.trim().is_empty() { stderr } else { stdout };
    AppError::Command {
        program: program.to_string(),
        message: message.trim().to_string(),
    }
}
