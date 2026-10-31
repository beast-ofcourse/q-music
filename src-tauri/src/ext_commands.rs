//! Extension platform commands for the desktop shell.
//!
//! Thin wrappers over `qext`: the crate owns the behaviour, this module only
//! translates between Tauri's command boundary and the host's types.

use std::path::PathBuf;
use std::sync::Mutex;

use qext::store::ExtensionStore;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::state::AppState;

/// View struct returned to the frontend (camelCase, matching `extensions.ts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledExtensionView {
    pub id: String,
    pub version: String,
    pub display_name: Option<String>,
    pub sha256: String,
    pub installed_at: i64,
    pub enabled: bool,
    pub directory: String,
    pub data_directory: String,
}

impl From<qext::store::InstalledExtension> for InstalledExtensionView {
    fn from(ext: qext::store::InstalledExtension) -> Self {
        Self {
            id: ext.id,
            version: ext.version.to_string(),
            display_name: ext.display_name,
            sha256: ext.sha256,
            installed_at: ext.installed_at,
            enabled: ext.enabled,
            directory: ext.directory.to_string_lossy().to_string(),
            data_directory: ext.data_directory.to_string_lossy().to_string(),
        }
    }
}

/// Extension store state managed by Tauri.
pub struct ExtensionState {
    pub store: Mutex<ExtensionStore>,
    pub root: PathBuf,
}

/// Install an extension from a file chosen via dialog.
#[tauri::command]
pub async fn extensions_install_file(
    app: AppHandle,
    ext_state: State<'_, ExtensionState>,
) -> Result<Option<InstalledExtensionView>, String> {
    let path = app
        .dialog()
        .file()
        .add_filter("Extension package", &["sflx", "spotiflac-ext"])
        .blocking_pick_file();

    let Some(path) = path else {
        return Ok(None);
    };

    let path = path
        .as_path()
        .ok_or_else(|| "invalid file path".to_string())?
        .to_path_buf();

    // Install via the store (inspects internally).
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let installed = store.install(&path).map_err(|e| e.to_string())?;

    Ok(Some(installed.into()))
}

/// List installed extensions.
#[tauri::command]
pub fn extensions_snapshot(
    ext_state: State<'_, ExtensionState>,
) -> Result<serde_json::Value, String> {
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let scan = store.scan();
    let installed: Vec<InstalledExtensionView> =
        scan.installed.into_iter().map(Into::into).collect();
    let broken: Vec<serde_json::Value> = scan
        .broken
        .into_iter()
        .map(|b| serde_json::json!({"id": b.id, "path": b.path, "reason": b.reason}))
        .collect();

    Ok(serde_json::json!({
        "entries": [],
        "installed": installed,
        "broken": broken,
        "refreshError": null,
        "fetchedAt": 0,
    }))
}