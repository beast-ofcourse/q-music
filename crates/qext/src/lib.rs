//! Q-Music extension host.
//!
//! Executes Mornye-compatible extension packages (`.sflx`) inside a sandboxed
//! JavaScript runtime and routes metadata, search, lyrics and download work
//! through them.
//!
//! Modules are added as the host is built; see `plans/tasks.md`.

pub mod adapter;
pub mod calls;
pub mod contract;
pub mod download;
pub mod manifest;
pub mod negotiate;
pub mod package;
pub mod postprocess;
pub mod providers;
pub mod queue;
pub mod registry;
pub mod runtime;
pub mod search;
pub mod store;

pub use manifest::{Manifest, ManifestError};


