//! Online search, provider detail and home-feed commands.
//!
//! Thin orchestration over `qext`: this module owns the Tauri command boundary,
//! provider activation, priority persistence and in-library detection. The heavy
//! lifting (concurrency, timeouts, dedup) lives in `qext::search`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qcore::db::TrackQuery;
use qcore::{SortField, SortOrder};
use qext::adapter::Capabilities;
use qext::calls::{CallCache, Calls};
use qext::manifest::Manifest;
use qext::runtime::http::{HostHttp, HttpLimits};
use qext::runtime::permissions::NetworkAllowlist;
use qext::runtime::polyfills::{HostConfig, HostServices};
use qext::search::{
    OnlineAlbum, OnlineArtist, OnlinePlaylist, OnlineTrack, ProviderOutcome, SearchAggregate,
    SearchCoordinator, SearchProvider, SearchRequest, SearchType,
};
use qext::store::ExtensionStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, State};

use crate::ext_commands::ExtensionState;
use crate::state::AppState;

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HTTP_TIMEOUT: u64 = 15;

fn installed_ordered(
    store: &mut ExtensionStore,
    ext_state: &ExtensionState,
) -> Vec<qext::store::InstalledExtension> {
    let scan = store.scan();
    let priority = crate::ext_commands::persisted_priority(ext_state, "metadata");
    let mut installed = scan.installed;
    installed.sort_by_key(|ext| {
        priority
            .iter()
            .position(|id| id == &ext.id)
            .unwrap_or(usize::MAX)
    });
    installed
}

fn activate_provider(
    store: &mut ExtensionStore,
    ext_state_root: &PathBuf,
    installed: &qext::store::InstalledExtension,
) -> Result<(), String> {
    if store.is_active(&installed.id) {
        return Ok(());
    }
    if !installed.enabled {
        return Err("disabled".to_string());
    }
    let manifest_text = fs::read_to_string(installed.manifest_path()).map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_str(&manifest_text).map_err(|e| e.to_string())?;

    let allowlist = NetworkAllowlist::from_patterns(&manifest.permissions.network);
    let http_timeout = manifest
        .capabilities
        .network_timeout_seconds
        .unwrap_or(DEFAULT_HTTP_TIMEOUT)
        .clamp(5, 30);
    let http = HostHttp::new(allowlist, HttpLimits::with_timeout_seconds(http_timeout));
    let config = HostConfig {
        app_version: "4.10.0".to_string(),
        user_agent: "Q-Music/0.1.0".to_string(),
        data_dir: Some(ext_state_root.join(".data").join(&installed.id)),
        http: Some(http),
        ffmpeg: None,
    };
    let host = Arc::new(HostServices::new(config));
    store.activate(&installed.id, host).map_err(|e| e.to_string())?;
    eprintln!("[qmusic.ext] activated {}", installed.id);
    Ok(())
}

fn manifest_for(installed: &qext::store::InstalledExtension) -> Option<Manifest> {
    let text = fs::read_to_string(installed.manifest_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn init_settings_for(manifest: &Manifest) -> serde_json::Map<String, Value> {
    manifest.default_settings()
}

fn build_search_providers(
    store: &mut ExtensionStore,
    ext_state: &ExtensionState,
) -> Vec<SearchProvider> {
    let mut providers = Vec::new();
    for installed in installed_ordered(store, ext_state) {
        if !installed.enabled {
            continue;
        }
        if let Err(err) = activate_provider(store, &ext_state.root, &installed) {
            eprintln!("[qmusic.ext] activate failed {}: {}", installed.id, err);
            continue;
        }
        let manifest = match manifest_for(&installed) {
            Some(m) => m,
            None => continue,
        };
        // capability check via runtime
        let runtime = match store.runtime_of(&installed.id) {
            Some(r) => r,
            None => continue,
        };
        let caps = Capabilities::from_runtime(&runtime);
        if !caps.has_search() {
            continue;
        }
        // initialize (best effort, once per process handled by coordinator memoization
        // but we still pass settings; coordinator calls initialize).
        let init_settings = init_settings_for(&manifest);
        let timeout_secs = manifest
            .capabilities
            .network_timeout_seconds
            .unwrap_or(DEFAULT_HTTP_TIMEOUT)
            .clamp(5, 30);
        providers.push(SearchProvider {
            id: installed.id.clone(),
            display_name: installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
            runtime,
            init_settings,
            timeout: Duration::from_secs(timeout_secs),
        });
    }
    providers
}

fn local_library_keys(state: &AppState) -> HashSet<(String, String)> {
    let mut keys = HashSet::new();
    let library = match state.library.lock() {
        Ok(l) => l,
        Err(_) => return keys,
    };
    let query = TrackQuery {
        search: None,
        artist: None,
        album: None,
        genre: None,
        format: None,
        favorites_only: false,
        sort: SortField::Title,
        order: SortOrder::Asc,
        limit: 0,
        offset: 0,
    };
    if let Ok(tracks) = library.db().list_tracks(&query) {
        for t in tracks {
            keys.insert((normalize(&t.title), normalize(&t.artist)));
        }
    }
    keys
}

fn normalize(input: &str) -> String {
    input.trim().to_lowercase()
}

// (in-library annotation is performed inline in online_search)


// We need an extra field on OnlineTrack for inLibrary. Since OnlineTrack is in qext,
// store it in extra? qext OnlineTrack has no in_library field. Add via a wrapper? 
// Instead, we'll post-process by serializing and injecting "inLibrary" into the JSON
// in the command layer. See online_search command.

#[tauri::command]
pub async fn online_search(
    ext_state: State<'_, ExtensionState>,
    app_state: State<'_, AppState>,
    request_id: String,
    query: String,
    types: Vec<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let providers = build_search_providers(&mut store, &ext_state);

    let search_types: Vec<SearchType> = types
        .iter()
        .filter_map(|t| match t.as_str() {
            "tracks" | "track" | "songs" | "song" => Some(SearchType::Tracks),
            "albums" | "album" => Some(SearchType::Albums),
            "artists" | "artist" => Some(SearchType::Artists),
            "playlists" | "playlist" => Some(SearchType::Playlists),
            _ => None,
        })
        .collect();

    let request = SearchRequest {
        query: query.clone(),
        types: search_types,
        limit: limit.unwrap_or(10) as usize,
        offset: offset.unwrap_or(0) as usize,
    };

    let coordinator = &ext_state.search_coordinator;
    let aggregate = coordinator.search(&request_id, &request, &providers, CALL_TIMEOUT);

    // in-library keys
    let keys = local_library_keys(&app_state);

    // Build JSON with inLibrary injected.
    let mut tracks_json: Vec<Value> = aggregate
        .tracks
        .iter()
        .map(|t| {
            let mut v = serde_json::to_value(t).unwrap_or(Value::Null);
            if let Some(obj) = v.as_object_mut() {
                let key = (normalize(&t.title), normalize(&t.artists.first().cloned().unwrap_or_default()));
                obj.insert("inLibrary".to_string(), json!(keys.contains(&key)));
            }
            v
        })
        .collect();
    let _ = &mut tracks_json;

    let providers_json: Vec<Value> = aggregate
        .providers
        .iter()
        .map(|p: &ProviderOutcome| {
            json!({
                "id": p.provider,
                "name": p.display_name,
                "state": format!("{:?}", p.state).to_lowercase(),
                "error": p.error,
                "elapsedMs": p.elapsed_ms,
                "hitCounts": {
                    "tracks": p.tracks.len(),
                    "albums": p.albums.len(),
                    "artists": p.artists.len(),
                    "playlists": p.playlists.len(),
                }
            })
        })
        .collect();

    let result = json!({
        "requestId": aggregate.request_id,
        "query": aggregate.query,
        "tracks": aggregate.tracks,
        "albums": aggregate.albums,
        "artists": aggregate.artists,
        "playlists": aggregate.playlists,
        "providers": providers_json,
        "elapsedMs": aggregate.elapsed_ms,
        "cancelled": aggregate.cancelled,
    });

    // Re-inject inLibrary into the serialized tracks array (the above used aggregate.tracks without it).
    // Rebuild tracks with inLibrary by mutating the result object.
    if let Some(obj) = result.as_object() {
        let _ = obj;
    }
    // Replace tracks array with annotated version.
    let mut result = result;
    if let Some(obj) = result.as_object_mut() {
        obj.insert("tracks".to_string(), json!(tracks_json));
    }

    Ok(result)
}

#[tauri::command]
pub fn online_search_cancel(ext_state: State<'_, ExtensionState>, request_id: String) {
    ext_state.search_coordinator.cancel(&request_id);
}

#[tauri::command]
pub async fn provider_detail(
    ext_state: State<'_, ExtensionState>,
    kind: String,
    provider: String,
    id: String,
) -> Result<Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    // find installed
    let scan = store.scan();
    let installed = scan
        .installed
        .into_iter()
        .find(|e| e.id == provider)
        .ok_or_else(|| format!("provider {provider} not installed"))?;

    activate_provider(&mut store, &ext_state.root, &installed)?;

    let runtime = store.runtime_of(&provider).ok_or("provider not active")?;
    let mut cache = CallCache::open_in_memory(0).map_err(|e| e.to_string())?;
    let mut calls = Calls::new(&provider, &runtime, &mut cache);

    let display_name = installed.display_name.clone().unwrap_or_else(|| provider.clone());

    match kind.as_str() {
        "album" => {
            let resolved = calls.album(&id).map_err(|e| e.to_string())?;
            match resolved {
                Some(r) => {
                    let album = r.item;
                    let tracks: Vec<Value> = album
                        .tracks
                        .iter()
                        .map(|t| {
                            json!({
                                "id": format!("{provider}:{}", t.id),
                                "provider": provider,
                                "providerName": display_name,
                                "providerTrackId": t.id,
                                "title": t.name,
                                "artists": t.artists.as_slice(),
                                "album": album.name,
                                "durationMs": t.duration_ms,
                                "artUrl": t.images.first().cloned().or_else(|| album.images.first().cloned()),
                            })
                        })
                        .collect();
                    Ok(json!({
                        "kind": "album",
                        "provider": provider,
                        "providerName": display_name,
                        "id": format!("{provider}:{}", album.id),
                        "providerAlbumId": album.id,
                        "title": album.name,
                        "artists": album.artists.as_slice(),
                        "year": album.release_date.as_deref().and_then(|d| d.get(0..4)).and_then(|y| y.parse::<i32>().ok()),
                        "trackCount": album.total_tracks.or(Some(tracks.len() as u32)),
                        "artUrl": album.images.first().cloned(),
                        "tracks": tracks,
                    }))
                }
                None => Ok(json!(null)),
            }
        }
        "playlist" => {
            let resolved = calls.playlist(&id).map_err(|e| e.to_string())?;
            match resolved {
                Some(r) => {
                    let pl = r.item;
                    let tracks: Vec<Value> = pl
                        .tracks
                        .iter()
                        .map(|t| {
                            json!({
                                "id": format!("{provider}:{}", t.id),
                                "provider": provider,
                                "providerName": display_name,
                                "providerTrackId": t.id,
                                "title": t.name,
                                "artists": t.artists.as_slice(),
                                "album": t.album_name,
                                "durationMs": t.duration_ms,
                                "artUrl": t.images.first().cloned().or_else(|| pl.images.first().cloned()),
                            })
                        })
                        .collect();
                    let creator = pl
                        .extra
                        .get("owner")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    Ok(json!({
                        "kind": "playlist",
                        "provider": provider,
                        "providerName": display_name,
                        "id": format!("{provider}:{}", pl.id),
                        "providerPlaylistId": pl.id,
                        "title": pl.name,
                        "creator": creator,
                        "trackCount": pl.total_tracks.or(Some(tracks.len() as u32)),
                        "artUrl": pl.images.first().cloned(),
                        "tracks": tracks,
                    }))
                }
                None => Ok(json!(null)),
            }
        }
        "artist" => {
            let resolved = calls.artist(&id).map_err(|e| e.to_string())?;
            match resolved {
                Some(r) => {
                    let artist = r.item;
                    let albums: Vec<Value> = artist
                        .albums
                        .iter()
                        .map(|a| {
                            json!({
                                "id": format!("{provider}:{}", a.id),
                                "provider": provider,
                                "providerName": display_name,
                                "providerAlbumId": a.id,
                                "title": a.name,
                                "artists": a.artists.as_slice(),
                                "year": a.release_date.as_deref().and_then(|d| d.get(0..4)).and_then(|y| y.parse::<i32>().ok()),
                                "artUrl": a.images.first().cloned(),
                            })
                        })
                        .collect();
                    Ok(json!({
                        "kind": "artist",
                        "provider": provider,
                        "providerName": display_name,
                        "id": format!("{provider}:{}", artist.id),
                        "providerArtistId": artist.id,
                        "name": artist.name,
                        "artUrl": artist.images.first().cloned(),
                        "albums": albums,
                    }))
                }
                None => Ok(json!(null)),
            }
        }
        _ => Err(format!("unknown detail kind {kind}")),
    }
}

#[tauri::command]
pub async fn provider_home(ext_state: State<'_, ExtensionState>) -> Result<Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let installed = installed_ordered(&mut store, &ext_state);
    let mut providers_out: Vec<Value> = Vec::new();

    for installed in installed {
        if !installed.enabled {
            continue;
        }
        if activate_provider(&mut store, &ext_state.root, &installed).is_err() {
            continue;
        }
        let runtime = match store.runtime_of(&installed.id) {
            Some(r) => r,
            None => continue,
        };
        let caps = Capabilities::from_runtime(&runtime);
        if !caps.has("getHomeFeed") {
            continue;
        }
        let mut cache = CallCache::open_in_memory(0).map_err(|e| e.to_string())?;
        let mut calls = Calls::new(&installed.id, &runtime, &mut cache);
        let display_name = installed.display_name.clone().unwrap_or_else(|| installed.id.clone());

        match calls.home_feed() {
            Ok(Some(resolved)) => {
                let value = resolved.item;
                // best-effort section extraction
                let mut sections: Vec<Value> = Vec::new();
                if let Some(arr) = value.get("sections").and_then(Value::as_array) {
                    for sec in arr {
                        let title = sec
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("Recommended")
                            .to_string();
                        let mut items: Vec<Value> = Vec::new();
                        // collect from common keys
                        for key in ["items", "tracks", "albums", "artists", "playlists"] {
                            if let Some(arr) = sec.get(key).and_then(Value::as_array) {
                                for it in arr {
                                    items.push(normalize_home_item(&installed.id, &display_name, it));
                                }
                            }
                        }
                        if !items.is_empty() {
                            sections.push(json!({ "title": title, "items": items }));
                        }
                    }
                }
                if !sections.is_empty() {
                    providers_out.push(json!({
                        "id": installed.id,
                        "name": display_name,
                        "sections": sections,
                    }));
                }
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("[qmusic.ext] home_feed failed {}: {}", installed.id, err);
            }
        }
    }

    Ok(json!({ "providers": providers_out }))
}

fn normalize_home_item(provider: &str, provider_name: &str, value: &Value) -> Value {
    let item_type = value
        .get("item_type")
        .or_else(|| value.get("itemType"))
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("track");
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let title = value
        .get("name")
        .or_else(|| value.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let art_url = value
        .get("images")
        .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_array().and_then(|a| a.first()).and_then(Value::as_str).map(str::to_string)))
        .or_else(|| value.get("cover_url").and_then(Value::as_str).map(str::to_string))
        .or_else(|| value.get("image_url").and_then(Value::as_str).map(str::to_string));
    let artists: Vec<String> = match value.get("artists") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    json!({
        "type": item_type,
        "id": format!("{provider}:{id}"),
        "provider": provider,
        "providerName": provider_name,
        "providerItemId": id,
        "title": title,
        "artists": artists,
        "artUrl": art_url,
    })
}

#[tauri::command]
pub fn providers_priority_get(ext_state: State<'_, ExtensionState>) -> Result<Value, String> {
    let order = crate::ext_commands::persisted_priority(&ext_state, "metadata");
    Ok(json!({ "order": order }))
}

#[tauri::command]
pub fn providers_priority_set(
    ext_state: State<'_, ExtensionState>,
    ids: Vec<String>,
) -> Result<Value, String> {
    crate::ext_commands::save_persisted_priority(&ext_state, "metadata", &ids)?;
    Ok(json!({ "ok": true }))
}

// Helper to expose the coordinator handle for other modules if needed.
pub fn coordinator(ext_state: &ExtensionState) -> &SearchCoordinator {
    &ext_state.search_coordinator
}

// extension point: keep ExtensionState field name in sync with ext_commands.rs
