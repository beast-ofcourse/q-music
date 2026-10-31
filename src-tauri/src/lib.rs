//! Q-Music desktop shell.
//!
//! The window is a view over the Rust core: it never sees samples, SQL rows or tag
//! bytes directly. Startup restores the listener's last volume, speed, equaliser and
//! output device so reopening the app puts them back where they left off.

mod commands;
mod ext_commands;
mod media;
mod state;

use std::sync::Mutex;

use qcore::{AppPaths, Engine, EqSettings, Library};
use tauri::{Manager, WindowEvent};

use state::AppState;

const KEY_VOLUME: &str = "player.volume";
const KEY_SPEED: &str = "player.speed";
const KEY_EQ: &str = "player.eq";
const KEY_DEVICE: &str = "player.device";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::library_stats,
            commands::list_tracks,
            commands::count_tracks,
            commands::distinct_values,
            commands::group_tracks,
            commands::get_track,
            commands::set_favorite,
            commands::recent_tracks,
            commands::recently_added,
            commands::list_folders,
            commands::scan_folder,
            commands::rescan_library,
            commands::remove_folder,
            commands::artwork,
            commands::update_tags,
            commands::lyrics_for,
            commands::save_lyrics,
            commands::import_lyrics,
            commands::export_lyrics,
            commands::estimate_lyrics,
            commands::list_playlists,
            commands::create_playlist,
            commands::rename_playlist,
            commands::delete_playlist,
            commands::playlist_tracks,
            commands::update_smart_rules,
            commands::add_to_playlist,
            commands::remove_from_playlist,
            commands::move_playlist_item,
            commands::get_setting,
            commands::set_setting,
            commands::engine_snapshot,
            commands::engine_load,
            commands::engine_play,
            commands::engine_pause,
            commands::engine_stop,
            commands::engine_seek,
            commands::engine_set_volume,
            commands::engine_set_speed,
            commands::engine_set_eq,
            commands::engine_set_next,
            commands::output_devices,
            commands::set_output_device,
            commands::report_play,
            commands::report_playback,
            commands::pick_folder,
            commands::pick_files,
            commands::reveal_track,
            ext_commands::extensions_snapshot,
            ext_commands::extensions_install_file,
            ext_commands::extensions_settings,
            ext_commands::extensions_save_settings,
            ext_commands::extensions_invoke_action,
            ext_commands::extensions_set_enabled,
            ext_commands::extensions_uninstall,
            ext_commands::search_providers,
            ext_commands::providers_list,
            ext_commands::providers_set_priority,
            ext_commands::providers_retry,
            ext_commands::provider_lyrics,
            ext_commands::queue_jobs,
            ext_commands::queue_cancel,
            ext_commands::queue_retry,
            ext_commands::queue_enqueue,
        ])
        .setup(|app| {
            let root = AppPaths::resolve().data_dir;
            let library = Library::open(root.clone())?;

            let device = setting(&library, KEY_DEVICE).unwrap_or_default();
            let engine = Engine::start(Some(device).filter(|name: &String| !name.is_empty()));

            let volume: f32 = setting(&library, KEY_VOLUME).unwrap_or(0.85);
            let speed: f32 = setting(&library, KEY_SPEED).unwrap_or(1.0);
            let eq: EqSettings = setting(&library, KEY_EQ).unwrap_or_else(EqSettings::flat);
            let _ = engine.set_volume(volume);
            let _ = engine.set_speed(speed);
            let _ = engine.set_eq(eq);

            let media = match app.get_webview_window("main") {
                Some(window) => match window.hwnd() {
                    Ok(hwnd) => media::spawn(app.handle().clone(), hwnd.0 as isize),
                    Err(_) => media::spawn(app.handle().clone(), 0),
                },
                None => media::spawn(app.handle().clone(), 0),
            };

            // Extension host.
            let ext_root = root.join("extensions");
            let ext_store = qext::store::ExtensionStore::open(&ext_root)
                .map_err(|e| e.to_string())?;
            app.manage(ext_commands::ExtensionState {
                store: Mutex::new(ext_store),
                root: ext_root,
            });

            app.manage(AppState {
                library: Mutex::new(library),
                engine: Mutex::new(engine),
                root,
                media,
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                persist_playback(window.app_handle());
            }
        })
        .run(tauri::generate_context!())
        .expect("Q-Music could not start its window");
}

/// Reads a JSON setting, ignoring a value that no longer parses rather than refusing
/// to start.
fn setting<T: serde::de::DeserializeOwned>(library: &Library, key: &str) -> Option<T> {
    library
        .db()
        .get_setting(key)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str(&value).ok())
}

/// Called on close so the next launch resumes the same listening setup.
fn persist_playback(app: &tauri::AppHandle) {    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Ok(library) = state.library.lock() else {
        return;
    };
    let Ok(engine) = state.engine.lock() else {
        return;
    };

    let snapshot = engine.snapshot();
    let eq = library
        .db()
        .get_setting(KEY_EQ)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<EqSettings>(&value).ok())
        .unwrap_or_else(EqSettings::flat);

    for (key, value) in [
        (KEY_VOLUME, serde_json::to_string(&snapshot.volume)),
        (KEY_SPEED, serde_json::to_string(&snapshot.speed)),
        (KEY_EQ, serde_json::to_string(&eq)),
        (KEY_DEVICE, serde_json::to_string(&snapshot.device)),
    ] {
        if let Ok(encoded) = value {
            let _ = library.db().set_setting(key, &encoded);
        }
    }
}
