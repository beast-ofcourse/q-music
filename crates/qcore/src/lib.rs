//! Q-Music core.
//!
//! The interface never touches samples, SQL or tag bytes directly: everything it
//! needs is exposed here as typed values, so the same engine can back a desktop
//! window, a test harness or a future CLI without a second implementation.

pub mod art;
pub mod db;
pub mod deck;
pub mod embed;
pub mod engine;
pub mod eq;
pub mod error;
pub mod library;
pub mod lrc;
pub mod model;
pub mod scan;
pub mod tags;

pub use engine::Engine;
pub use error::{CoreError, CoreResult};
pub use library::Library;
pub use model::*;

/// Seconds since the Unix epoch. One definition, so timestamps sort consistently
/// across the index, play history and scan records.
pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Where Q-Music keeps its index, artwork cache and settings.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: std::path::PathBuf,
}

impl AppPaths {
    /// Resolves the platform data directory, falling back to the working directory
    /// when the OS reports no home folder (headless CI, unusual service accounts).
    pub fn resolve() -> Self {
        let data_dir = dirs::data_dir()
            .map(|directory| directory.join("Q-Music"))
            .unwrap_or_else(|| std::path::PathBuf::from("q-music-data"));
        Self { data_dir }
    }

    pub fn with_root(root: impl Into<std::path::PathBuf>) -> Self {
        Self { data_dir: root.into() }
    }
}
