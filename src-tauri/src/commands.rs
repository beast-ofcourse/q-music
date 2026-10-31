//! Every operation the interface can ask the core to perform.
//!
//! All of them are thin: the core owns the behaviour, so a command can only fail by
//! passing on a real error with a message a person can read.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use qcore::db::TrackQuery;
use qcore::{
    CoreError, EngineSnapshot, EqSettings, Library, LibraryStats, Lyrics, OutputDevice, Playlist,
    ScanProgress, SmartRules, Track,
};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

use crate::media::MediaCommand;
use crate::state::AppState;

fn lock<'a, T>(mutex: &'a Mutex<T>) -> Result<MutexGuard<'a, T>, String> {
    mutex
        .lock()
        .map_err(|_| "internal state was poisoned — restart Q-Music".to_string())
}

fn readable(error: CoreError) -> String {
    error.user_message()
}

// ---------------------------------------------------------------- library ----

#[tauri::command]
pub fn library_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
    lock(&state.library)?.db().stats().map_err(readable)
}

#[tauri::command]
pub fn list_tracks(state: State<'_, AppState>, query: TrackQuery) -> Result<Vec<Track>, String> {
    lock(&state.library)?.db().list_tracks(&query).map_err(readable)
}

#[tauri::command]
pub fn count_tracks(state: State<'_, AppState>, query: TrackQuery) -> Result<usize, String> {
    lock(&state.library)?.db().count_tracks(&query).map_err(readable)
}

#[tauri::command]
pub fn distinct_values(state: State<'_, AppState>, column: String) -> Result<Vec<String>, String> {
    lock(&state.library)?.db().distinct(&column).map_err(readable)
}

/// One row per album or artist for the browse lists, aggregated in SQL.
#[tauri::command]
pub fn group_tracks(
    state: State<'_, AppState>,
    column: String,
) -> Result<Vec<qcore::TrackGroup>, String> {
    lock(&state.library)?.db().groups(&column).map_err(readable)
}

#[tauri::command]
pub fn get_track(state: State<'_, AppState>, track_id: i64) -> Result<Option<Track>, String> {
    lock(&state.library)?.track(track_id).map_err(readable)
}

#[tauri::command]
pub fn set_favorite(
    state: State<'_, AppState>,
    track_id: i64,
    favorite: bool,
) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .set_favorite(track_id, favorite)
        .map_err(readable)
}

#[tauri::command]
pub fn recent_tracks(state: State<'_, AppState>, limit: u32) -> Result<Vec<Track>, String> {
    lock(&state.library)?.db().recent_tracks(limit).map_err(readable)
}

#[tauri::command]
pub fn recently_added(state: State<'_, AppState>, limit: u32) -> Result<Vec<Track>, String> {
    lock(&state.library)?.db().recently_added(limit).map_err(readable)
}

#[tauri::command]
pub fn list_folders(state: State<'_, AppState>) -> Result<Vec<qcore::LibraryFolder>, String> {
    lock(&state.library)?.db().folders().map_err(readable)
}

/// Indexes one folder on a background thread and streams progress. A second handle is
/// opened for the scan so browsing and playback keep working while it runs.
#[tauri::command]
pub fn scan_folder(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<(), String> {
    let root = state.root.clone();
    let folder = PathBuf::from(path);
    let label = folder.display().to_string();

    std::thread::Builder::new()
        .name("qm-scan".into())
        .spawn(move || {
            let result = (|| -> Result<ScanProgress, CoreError> {
                let library = Library::open(root)?;
                library.db().add_folder(&label, qcore::now_seconds())?;
                let progress = library.scan(&folder, |progress| {
                    let _ = app.emit("library://scan", progress);
                })?;
                // Artwork for removed tracks is dropped after the index settles.
                let _ = library.prune_artwork();
                Ok(progress)
            })();

            match result {
                Ok(progress) => {
                    let _ = app.emit("library://scan-done", progress);
                }
                Err(error) => {
                    let _ = app.emit("library://scan-error", error.user_message());
                }
            }
        })
        .map_err(|error| format!("cannot start the scanner: {error}"))?;

    Ok(())
}

#[tauri::command]
pub fn rescan_library(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let folders: Vec<String> = lock(&state.library)?
        .db()
        .folders()
        .map_err(readable)?
        .into_iter()
        .map(|folder| folder.path)
        .collect();

    if folders.is_empty() {
        return Err("add a folder before scanning".to_string());
    }

    let root = state.root.clone();
    std::thread::Builder::new()
        .name("qm-rescan".into())
        .spawn(move || {
            for folder in folders {
                let path = PathBuf::from(&folder);
                match Library::open(root.clone()) {
                    Ok(library) => {
                        let _ = library.scan(&path, |progress| {
                            let _ = app.emit("library://scan", progress);
                        });
                    }
                    Err(error) => {
                        let _ = app.emit("library://scan-error", error.user_message());
                        return;
                    }
                }
            }
            let _ = app.emit("library://scan-complete", ());
        })
        .map_err(|error| format!("cannot start the scanner: {error}"))?;

    Ok(())
}

#[tauri::command]
pub fn remove_folder(state: State<'_, AppState>, folder_id: i64) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .remove_folder(folder_id)
        .map_err(readable)?;
    Ok(())
}

#[tauri::command]
pub fn artwork(state: State<'_, AppState>, art_id: String) -> Result<Option<String>, String> {
    let library = lock(&state.library)?;
    let Some((bytes, mime)) = library.art().read(&art_id) else {
        return Ok(None);
    };
    Ok(Some(format!(
        "data:{mime};base64,{}",
        BASE64.encode(bytes)
    )))
}

#[tauri::command]
pub fn update_tags(
    state: State<'_, AppState>,
    path: String,
    edit: qcore::tags::TagEdit,
) -> Result<Track, String> {
    let file = PathBuf::from(&path);
    qcore::tags::write_tags(&file, &edit).map_err(readable)?;
    lock(&state.library)?.reindex_file(&file).map_err(readable)
}

// ----------------------------------------------------------------- lyrics ----

#[tauri::command]
pub fn lyrics_for(state: State<'_, AppState>, track_id: i64) -> Result<Lyrics, String> {
    lock(&state.library)?.lyrics_for(track_id).map_err(readable)
}

#[tauri::command]
pub fn save_lyrics(
    state: State<'_, AppState>,
    track_id: i64,
    content: String,
) -> Result<Lyrics, String> {
    lock(&state.library)?
        .save_lyrics(track_id, &content)
        .map_err(readable)
}

#[tauri::command]
pub fn import_lyrics(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
) -> Result<Option<Lyrics>, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("Lyrics", &["lrc", "txt"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let source = picked.into_path().map_err(|error| error.to_string())?;
    lock(&state.library)?
        .import_lyrics(track_id, &source)
        .map(Some)
        .map_err(readable)
}

#[tauri::command]
pub fn export_lyrics(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
    suggested_name: String,
) -> Result<Option<String>, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(&suggested_name)
        .add_filter("LRC lyric file", &["lrc"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let destination = picked.into_path().map_err(|error| error.to_string())?;
    lock(&state.library)?
        .export_lyrics(track_id, &destination)
        .map_err(readable)?;
    Ok(Some(destination.display().to_string()))
}

/// Fills in timings for a plain-text sheet so a `.txt` lyric file still follows the
/// song. The result is explicitly marked as estimated.
#[tauri::command]
pub fn estimate_lyrics(state: State<'_, AppState>, track_id: i64) -> Result<Lyrics, String> {
    let library = lock(&state.library)?;
    let mut lyrics = library.lyrics_for(track_id).map_err(readable)?;
    let duration = library
        .track(track_id)
        .map_err(readable)?
        .map(|track| track.duration_ms)
        .unwrap_or(0);

    let estimated = qcore::lrc::estimate_timings(&lyrics.lines, duration.max(30_000));
    if estimated.is_empty() {
        return Err("this lyric sheet has no lines to time".to_string());
    }

    lyrics.lines = estimated;
    lyrics.synced = true;
    lyrics.warning = Some("timings estimated from line length — adjust them by hand".into());
    Ok(lyrics)
}

// -------------------------------------------------------------- playlists ----

#[tauri::command]
pub fn list_playlists(state: State<'_, AppState>) -> Result<Vec<Playlist>, String> {
    lock(&state.library)?.db().playlists().map_err(readable)
}

#[tauri::command]
pub fn create_playlist(
    state: State<'_, AppState>,
    name: String,
    kind: String,
    rules: Option<SmartRules>,
) -> Result<i64, String> {
    lock(&state.library)?
        .db()
        .create_playlist(&name, &kind, rules.as_ref(), qcore::now_seconds())
        .map_err(readable)
}

#[tauri::command]
pub fn rename_playlist(
    state: State<'_, AppState>,
    playlist_id: i64,
    name: String,
) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .rename_playlist(playlist_id, &name, qcore::now_seconds())
        .map_err(readable)
}

#[tauri::command]
pub fn delete_playlist(state: State<'_, AppState>, playlist_id: i64) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .delete_playlist(playlist_id)
        .map_err(readable)
}

#[tauri::command]
pub fn playlist_tracks(state: State<'_, AppState>, playlist_id: i64) -> Result<Vec<Track>, String> {
    lock(&state.library)?
        .db()
        .playlist_tracks(playlist_id)
        .map_err(readable)
}

#[tauri::command]
pub fn update_smart_rules(
    state: State<'_, AppState>,
    playlist_id: i64,
    rules: SmartRules,
) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .update_smart_rules(playlist_id, &rules, qcore::now_seconds())
        .map_err(readable)
}

#[tauri::command]
pub fn add_to_playlist(
    state: State<'_, AppState>,
    playlist_id: i64,
    track_ids: Vec<i64>,
) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .add_to_playlist(playlist_id, &track_ids)
        .map_err(readable)
}

#[tauri::command]
pub fn remove_from_playlist(
    state: State<'_, AppState>,
    playlist_id: i64,
    position: i64,
) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .remove_from_playlist(playlist_id, position)
        .map_err(readable)
}

#[tauri::command]
pub fn move_playlist_item(
    state: State<'_, AppState>,
    playlist_id: i64,
    from: i64,
    to: i64,
) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .move_playlist_item(playlist_id, from, to)
        .map_err(readable)
}

// --------------------------------------------------------------- settings ----

#[tauri::command]
pub fn get_setting(state: State<'_, AppState>, key: String) -> Result<Option<String>, String> {
    lock(&state.library)?.db().get_setting(&key).map_err(readable)
}

#[tauri::command]
pub fn set_setting(state: State<'_, AppState>, key: String, value: String) -> Result<(), String> {
    lock(&state.library)?
        .db()
        .set_setting(&key, &value)
        .map_err(readable)
}

// ----------------------------------------------------------------- engine ----

#[tauri::command]
pub fn engine_snapshot(state: State<'_, AppState>) -> Result<EngineSnapshot, String> {
    Ok(lock(&state.engine)?.snapshot())
}

#[tauri::command]
pub fn engine_load(state: State<'_, AppState>, path: String, start_ms: u64) -> Result<(), String> {
    lock(&state.engine)?
        .load(&path, start_ms)
        .map_err(|error| error.user_message())
}

#[tauri::command]
pub fn engine_play(state: State<'_, AppState>) -> Result<(), String> {
    lock(&state.engine)?.play().map_err(|error| error.user_message())
}

#[tauri::command]
pub fn engine_pause(state: State<'_, AppState>) -> Result<(), String> {
    lock(&state.engine)?.pause().map_err(|error| error.user_message())
}

#[tauri::command]
pub fn engine_stop(state: State<'_, AppState>) -> Result<(), String> {
    lock(&state.engine)?
        .stop()
        .map_err(|error| error.user_message())?;
    // Stopping means nothing is playing, so the shell overlay should not keep showing
    // a paused track that the user cannot resume from there.
    let _ = state.media.send(MediaCommand::Clear);
    Ok(())
}

#[tauri::command]
pub fn engine_seek(state: State<'_, AppState>, position_ms: u64) -> Result<(), String> {
    lock(&state.engine)?
        .seek(position_ms)
        .map_err(|error| error.user_message())
}

#[tauri::command]
pub fn engine_set_volume(state: State<'_, AppState>, volume: f32) -> Result<(), String> {
    lock(&state.engine)?
        .set_volume(volume)
        .map_err(|error| error.user_message())
}

#[tauri::command]
pub fn engine_set_speed(state: State<'_, AppState>, speed: f32) -> Result<(), String> {
    lock(&state.engine)?
        .set_speed(speed)
        .map_err(|error| error.user_message())
}

#[tauri::command]
pub fn engine_set_eq(state: State<'_, AppState>, eq: EqSettings) -> Result<(), String> {
    lock(&state.engine)?
        .set_eq(eq)
        .map_err(|error| error.user_message())
}

#[tauri::command]
pub fn engine_set_next(state: State<'_, AppState>, path: Option<String>) -> Result<(), String> {
    lock(&state.engine)?
        .set_next(path)
        .map_err(|error| error.user_message())
}

#[tauri::command]
pub fn output_devices(state: State<'_, AppState>) -> Result<Vec<OutputDevice>, String> {
    Ok(lock(&state.engine)?.devices())
}

#[tauri::command]
pub fn set_output_device(state: State<'_, AppState>, name: String) -> Result<(), String> {
    lock(&state.engine)?
        .set_device(&name)
        .map_err(|error| error.user_message())
}

/// Records the play and pushes what the OS media session should display. Called once
/// per track start, so play counts and the shell overlay never disagree.
#[tauri::command]
pub fn report_play(state: State<'_, AppState>, track_id: i64) -> Result<(), String> {
    let library = lock(&state.library)?;
    library
        .db()
        .record_play(track_id, qcore::now_seconds())
        .map_err(readable)?;

    let Some(track) = library.track(track_id).map_err(readable)? else {
        return Ok(());
    };
    let cover_path = track
        .art_id
        .as_deref()
        .and_then(|art_id| library.art().find(art_id));
    let artist = track.display_artist().to_string();

    let _ = state.media.send(MediaCommand::Track {
        title: track.title,
        artist,
        album: track.album,
        duration_ms: track.duration_ms,
        cover_path,
    });
    Ok(())
}

#[tauri::command]
pub fn report_playback(
    state: State<'_, AppState>,
    playing: bool,
    position_ms: u64,
) -> Result<(), String> {
    let _ = state.media.send(MediaCommand::Playback { playing, position_ms });
    Ok(())
}

// --------------------------------------------------------------- dialogs ----

#[tauri::command]
pub fn pick_folder(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .set_title("Add a music folder")
        .blocking_pick_folder()
        .and_then(|picked| picked.into_path().ok())
        .map(|path| path.display().to_string())
}

#[tauri::command]
pub fn pick_files(app: AppHandle) -> Vec<String> {
    app.dialog()
        .file()
        .set_title("Add files to the queue")
        .add_filter("Audio", &["flac", "mp3", "m4a", "aac", "wav", "ogg", "opus", "aiff", "aif", "alac"])
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|picked| picked.into_path().ok())
        .map(|path| path.display().to_string())
        .collect()
}

/// Opens the OS file manager at a track's folder, which is the fastest way to reach
/// the files for a user who organises by folder.
#[tauri::command]
pub fn reveal_track(path: String) -> Result<(), String> {
    let file = Path::new(&path);
    let folder = file.parent().unwrap_or(file);
    opener_reveal(folder)
}

#[cfg(target_os = "windows")]
fn opener_reveal(folder: &Path) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(folder)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot open {folder:?}: {error}"))
}

#[cfg(not(target_os = "windows"))]
fn opener_reveal(folder: &Path) -> Result<(), String> {
    Err(format!("revealing {folder:?} is not implemented on this platform"))
}
