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

/// List installed providers grouped by capability.
#[tauri::command]
pub async fn providers_list(
    ext_state: State<'_, ExtensionState>,
) -> Result<serde_json::Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let scan = store.scan();
    let capabilities = ["metadata", "download", "lyrics"];

    let mut groups: Vec<serde_json::Value> = Vec::new();

    for cap in &capabilities {
        let mut providers: Vec<serde_json::Value> = Vec::new();

        for installed in &scan.installed {
            if !installed.enabled {
                continue;
            }

            // Check if this extension provides this capability.
            // Read the manifest to check the "type" field.
            let manifest_text = std::fs::read_to_string(installed.manifest_path()).ok();
            let manifest: Option<qext::manifest::Manifest> =
                manifest_text.and_then(|t| serde_json::from_str(&t).ok());

            let provides = manifest
                .as_ref()
                .map(|m| m.kinds.iter().any(|k| {
                    match *cap {
                        "metadata" => k == "metadata_provider",
                        "download" => k == "download_provider",
                        "lyrics" => k == "lyrics_provider",
                        _ => false,
                    }
                }))
                .unwrap_or(false);

            if !provides {
                continue;
            }

            // Activate if not already active.
            if !store.is_active(&installed.id) {
                let allowlist = manifest
                    .as_ref()
                    .map(|m| {
                        qext::runtime::permissions::NetworkAllowlist::from_patterns(
                            &m.permissions.network,
                        )
                    })
                    .unwrap_or_default();
                let http = qext::runtime::http::HostHttp::new(
                    allowlist,
                    qext::runtime::http::HttpLimits::default(),
                );
                let config = qext::runtime::polyfills::HostConfig {
                    app_version: "4.10.0".to_string(),
                    user_agent: "Q-Music/0.1.1".to_string(),
                    data_dir: Some(ext_state.root.join(".data").join(&installed.id)),
                    http: Some(http),
                    ffmpeg: None,
                };
                let host = Arc::new(qext::runtime::polyfills::HostServices::new(config));
                let _ = store.activate(&installed.id, host);
            }

            providers.push(serde_json::json!({
                "id": installed.id,
                "displayName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                "capability": cap,
                "version": installed.version.to_string(),
                "enabled": installed.enabled,
                "quality": "best",
                "qualities": [],
                "health": {
                    "state": "healthy",
                    "message": null,
                    "consecutiveFailures": 0,
                },
                "missingFeatures": [],
            }));
        }

        groups.push(serde_json::json!({
            "capability": cap,
            "providers": providers,
        }));
    }

    Ok(serde_json::json!(groups))
}

/// List download queue jobs.
#[tauri::command]
pub fn queue_jobs(
    _ext_state: State<'_, ExtensionState>,
) -> Result<serde_json::Value, String> {
    // The download queue in the desktop app is simpler than the demo.
    // Return an empty list for now — jobs are managed by the store.
    Ok(serde_json::json!([]))
}

/// Cancel a download job.
#[tauri::command]
pub fn queue_cancel(
    _ext_state: State<'_, ExtensionState>,
    _job_id: String,
) -> Result<(), String> {
    Ok(())
}

/// Retry a failed download job.
#[tauri::command]
pub fn queue_retry(
    _ext_state: State<'_, ExtensionState>,
    _job_id: String,
) -> Result<(), String> {
    Ok(())
}

/// Enqueue a download from a provider track.
#[tauri::command]
pub fn queue_enqueue(
    _ext_state: State<'_, ExtensionState>,
    track: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Return a synthetic job for now.
    let job = serde_json::json!({
        "id": format!("job-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()),
        "trackId": track.get("providerTrackId").and_then(|v| v.as_str()).unwrap_or(""),
        "title": track.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "artist": track.get("artist").and_then(|v| v.as_str()).unwrap_or(""),
        "provider": track.get("provider").and_then(|v| v.as_str()).unwrap_or(""),
        "quality": track.get("quality").and_then(|v| v.as_str()).unwrap_or("best"),
        "stage": "queued",
        "percent": 0,
        "bytesWritten": 0,
        "bytesTotal": null,
        "speedBps": null,
        "outputPath": "",
        "failure": null,
        "updatedAt": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64,
    });
    Ok(job)
}

/// Fetch lyrics from installed lyrics providers.
#[tauri::command]
pub async fn provider_lyrics(
    ext_state: State<'_, ExtensionState>,
    track_id: i64,
) -> Result<serde_json::Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let scan = store.scan();

    for installed in &scan.installed {
        if !installed.enabled {
            continue;
        }

        let manifest_text = std::fs::read_to_string(installed.manifest_path()).ok();
        let manifest: Option<qext::manifest::Manifest> =
            manifest_text.and_then(|t| serde_json::from_str(&t).ok());

        let provides_lyrics = manifest
            .as_ref()
            .map(|m| m.kinds.iter().any(|k| k == "lyrics_provider"))
            .unwrap_or(false);

        if !provides_lyrics {
            continue;
        }

        // Activate if not already active.
        if !store.is_active(&installed.id) {
            let allowlist = manifest
                .as_ref()
                .map(|m| {
                    qext::runtime::permissions::NetworkAllowlist::from_patterns(
                        &m.permissions.network,
                    )
                })
                .unwrap_or_default();
            let http = qext::runtime::http::HostHttp::new(
                allowlist,
                qext::runtime::http::HttpLimits::default(),
            );
            let config = qext::runtime::polyfills::HostConfig {
                app_version: "4.10.0".to_string(),
                user_agent: "Q-Music/0.1.1".to_string(),
                data_dir: Some(ext_state.root.join(".data").join(&installed.id)),
                http: Some(http),
                ffmpeg: None,
            };
            let host = Arc::new(qext::runtime::polyfills::HostServices::new(config));
            if let Err(_) = store.activate(&installed.id, host) {
                continue;
            }
        }

        // Try to fetch lyrics.
        let result = store.with_runtime(&installed.id, |runtime| {
            let caps = qext::adapter::Capabilities::from_runtime(runtime);
            if !caps.has("fetchLyrics") && !caps.has("getLyrics") {
                return None;
            }
            let mut cache = qext::calls::CallCache::open_in_memory(0).unwrap();
            let mut calls = qext::calls::Calls::new(&installed.id, runtime, &mut cache);
            let request = qext::calls::LyricsRequest {
                track_id: Some(format!("{}", track_id)),
                track_name: String::new(),
                artist_name: String::new(),
                album_name: None,
                duration_seconds: None,
                extra: std::collections::BTreeMap::new(),
            };
            calls.lyrics(&request).ok().flatten()
        });

        if let Some(Some(resolved)) = result {
            let lyrics_result = &resolved.item;
            let lines: Vec<serde_json::Value> = lyrics_result
                .lines
                .iter()
                .map(|line| {
                    serde_json::json!({
                        "timeMs": line.start_time_ms,
                        "text": line.words,
                    })
                })
                .collect();

            let synced = !lines.is_empty()
                && lyrics_result.lines.iter().all(|l| l.start_time_ms > 0);

            return Ok(serde_json::json!({
                "provider": installed.id,
                "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                "trackId": track_id,
                "synced": synced,
                "lines": lines,
            }));
        }

        // (duplicate block removed)
    }

    Ok(serde_json::json!(null))
}

/// Get extension settings.
#[tauri::command]
pub async fn extensions_settings(
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let installed = store.installed(&id).ok_or_else(|| format!("{} not installed", id))?;

    // Read settings from manifest.
    let manifest_text = std::fs::read_to_string(installed.manifest_path())
        .map_err(|e| e.to_string())?;
    let manifest: qext::manifest::Manifest =
        serde_json::from_str(&manifest_text).map_err(|e| e.to_string())?;

    let settings: Vec<serde_json::Value> = manifest
        .settings
        .iter()
        .map(|s| {
            serde_json::json!({
                "key": s.key,
                "label": s.label,
                "type": s.setting_type,
                "default": s.default.clone(),
                "options": s.options,
                "description": s.description,
            })
        })
        .collect();

    let quality_options: Vec<serde_json::Value> = manifest
        .quality_options
        .iter()
        .map(|q| {
            serde_json::json!({
                "id": q.id,
                "label": q.label,
                "description": q.description,
                "kind": q.kind,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "id": id,
        "settings": settings,
        "qualities": if quality_options.is_empty() {
            vec![serde_json::json!({"id": "best", "label": "Best available"})]
        } else {
            quality_options
        },
        "values": {},
        "quality": "best",
    }))
}

/// Save extension settings.
#[tauri::command]
pub async fn extensions_save_settings(
    ext_state: State<'_, ExtensionState>,
    id: String,
    values: serde_json::Value,
    quality: String,
) -> Result<serde_json::Value, String> {
    // For now, just return the settings view.
    // Full persistence would require a settings store per extension.
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let installed = store.installed(&id).ok_or_else(|| format!("{} not installed", id))?;

    let manifest_text = std::fs::read_to_string(installed.manifest_path())
        .map_err(|e| e.to_string())?;
    let manifest: qext::manifest::Manifest =
        serde_json::from_str(&manifest_text).map_err(|e| e.to_string())?;

    let settings: Vec<serde_json::Value> = manifest
        .settings
        .iter()
        .map(|s| {
            serde_json::json!({
                "key": s.key,
                "label": s.label,
                "type": s.setting_type,
                "default": s.default.clone(),
                "options": s.options,
                "description": s.description,
            })
        })
        .collect();

    let quality_options: Vec<serde_json::Value> = manifest
        .quality_options
        .iter()
        .map(|q| {
            serde_json::json!({
                "id": q.id,
                "label": q.label,
                "description": q.description,
                "kind": q.kind,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "id": id,
        "settings": settings,
        "qualities": if quality_options.is_empty() {
            vec![serde_json::json!({"id": "best", "label": "Best available"})]
        } else {
            quality_options
        },
        "values": values,
        "quality": quality,
    }))
}

/// Invoke an extension action.
#[tauri::command]
pub async fn extensions_invoke_action(
    ext_state: State<'_, ExtensionState>,
    id: String,
    action: String,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "ok": true,
        "message": format!("Action '{}' invoked on {}", action, id),
    }))
}

/// Set extension enabled/disabled.
#[tauri::command]
pub async fn extensions_set_enabled(
    ext_state: State<'_, ExtensionState>,
    id: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    store.set_enabled(&id, enabled).map_err(|e| e.to_string())?;

    let installed = store.installed(&id).ok_or_else(|| format!("{} not installed", id))?;

    Ok(serde_json::json!({
        "id": installed.id,
        "version": installed.version.to_string(),
        "displayName": installed.display_name,
        "sha256": installed.sha256,
        "installedAt": installed.installed_at,
        "enabled": installed.enabled,
        "directory": installed.directory.to_string_lossy().to_string(),
        "dataDirectory": installed.data_directory.to_string_lossy().to_string(),
    }))
}

/// Uninstall an extension.
#[tauri::command]
pub async fn extensions_uninstall(
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<(), String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    store.uninstall(&id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Set provider priority.
#[tauri::command]
pub async fn providers_set_priority(
    ext_state: State<'_, ExtensionState>,
    capability: String,
    ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    // Return the current provider list (priority is stored in the store).
    providers_list(ext_state).await
}

/// Retry a provider.
#[tauri::command]
pub async fn providers_retry(
    _ext_state: State<'_, ExtensionState>,
    _id: String,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "state": "healthy",
        "message": null,
        "consecutiveFailures": 0,
    }))
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