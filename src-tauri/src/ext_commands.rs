//! Extension platform commands for the desktop shell.
//!
//! Thin wrappers over `qext`: the crate owns the behaviour, this module only
//! translates between Tauri's command boundary and the host's types.

use std::path::PathBuf;
use std::sync::Mutex;

use qext::runtime::polyfills::HostServices;
use qext::store::ExtensionStore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

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

/// Search all installed metadata providers.
#[tauri::command]
pub async fn search_providers(
    ext_state: State<'_, ExtensionState>,
    query: String,
    limit: u32,
) -> Result<serde_json::Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    // Ensure data directory exists.
    let _ = std::fs::create_dir_all(ext_state.root.join(".data"));

    let scan = store.scan();
    let mut outcomes: Vec<serde_json::Value> = Vec::new();

    for installed in &scan.installed {
        if !installed.enabled {
            continue;
        }

        // Activate if not already active, with per-extension host services.
        if !store.is_active(&installed.id) {
            let installed_info = store.installed(&installed.id);
            let allowlist = installed_info
                .as_ref()
                .and_then(|i| {
                    let manifest_text = std::fs::read_to_string(i.manifest_path()).ok()?;
                    let manifest: qext::manifest::Manifest = serde_json::from_str(&manifest_text).ok()?;
                    Some(qext::runtime::permissions::NetworkAllowlist::from_patterns(&manifest.permissions.network))
                })
                .unwrap_or_default();
            let http = qext::runtime::http::HostHttp::new(
                allowlist,
                qext::runtime::http::HttpLimits::default(),
            );
            let config = qext::runtime::polyfills::HostConfig {
                app_version: "4.10.0".to_string(),
                user_agent: "Q-Music/0.1.0".to_string(),
                data_dir: Some(ext_state.root.join(".data").join(&installed.id)),
                http: Some(http),
                ffmpeg: None,
            };
            let host = Arc::new(qext::runtime::polyfills::HostServices::new(config));
            if let Err(e) = store.activate(&installed.id, host) {
                eprintln!("Failed to activate {}: {}", installed.id, e);
                continue;
            }
        }

        // Search via with_runtime.
        let result = store.with_runtime(&installed.id, |runtime| {
            let caps = qext::adapter::Capabilities::from_runtime(runtime);
            if !caps.has("searchTracks") && !caps.has("customSearch") {
                return None;
            }
            // Use a fresh cache (no persistent cache for simplicity).
            let mut cache = qext::calls::CallCache::open_in_memory(0).unwrap();
            let mut calls = qext::calls::Calls::new(&installed.id, runtime, &mut cache);
            let options = qext::calls::SearchOptions {
                limit: Some(limit),
                ..Default::default()
            };
            calls.search(&query, &options).ok()
        });

        match result {
            Some(Some(search_results)) => {
                let results: Vec<serde_json::Value> = search_results
                    .hits
                    .iter()
                    .filter_map(|hit| {
                        if let qext::calls::SearchHit::Track(track) = hit {
                            Some(serde_json::json!({
                                "provider": installed.id,
                                "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                                "providerTrackId": track.id,
                                "title": track.name,
                                "artist": track.artists.joined(),
                                "album": track.album_name.clone().unwrap_or_default(),
                                "durationMs": track.duration_ms,
                                "artUrl": track.images.first().cloned(),
                                "quality": "best",
                            }))
                        } else {
                            None
                        }
                    })
                    .collect();

                outcomes.push(serde_json::json!({
                    "provider": installed.id,
                    "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                    "results": results,
                    "failure": null,
                }));
            }
            Some(None) => {
                outcomes.push(serde_json::json!({
                    "provider": installed.id,
                    "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                    "results": [],
                    "failure": "No search function registered.",
                }));
            }
            None => {
                outcomes.push(serde_json::json!({
                    "provider": installed.id,
                    "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                    "results": [],
                    "failure": "Extension not active.",
                }));
            }
        }
    }

    Ok(serde_json::json!(outcomes))
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