//! Errors that cross the library boundary.
//!
//! Every failure a user can meet is represented here with enough detail to be
//! shown in the interface, because a music player that silently drops a file is
//! worse than one that says which file failed and why.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} is not audio Q-Music can decode")]
    UnsupportedFormat { path: PathBuf },

    #[error("decoding {path} failed: {message}")]
    Decode { path: PathBuf, message: String },

    #[error("file system error: {0}")]
    Storage(#[from] std::io::Error),

    #[error("library database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("tags in {path} could not be read: {message}")]
    Tags { path: PathBuf, message: String },

    #[error("{path} is read-only, so its tags cannot be written")]
    ReadOnly { path: PathBuf },

    #[error("audio device error: {0}")]
    Device(String),

    #[error("{0}")]
    Invalid(String),
}

impl CoreError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io { path: path.into(), source }
    }

    /// A short message safe to show in a row or a toast.
    pub fn user_message(&self) -> String {
        match self {
            CoreError::Io { path, source } => {
                format!("{}: {}", short_path(path), source)
            }
            CoreError::UnsupportedFormat { path } => {
                format!("{}: unsupported format", short_path(path))
            }
            CoreError::Decode { path, message } => {
                format!("{}: {}", short_path(path), message)
            }
            CoreError::Tags { path, message } => {
                format!("{}: tags unreadable ({})", short_path(path), message)
            }
            CoreError::ReadOnly { path } => {
                format!("{}: read-only file", short_path(path))
            }
            other => other.to_string(),
        }
    }
}

fn short_path(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub type CoreResult<T> = Result<T, CoreError>;
