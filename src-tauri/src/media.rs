//! Operating-system media controls.
//!
//! Windows ties System Media Transport Controls to a window, and the COM objects
//! behind them are not safe to drive from arbitrary threads. So this module owns a
//! single thread that creates the controls, listens for hardware media keys and the
//! shell flyout, and applies playback state as it arrives over a channel. The rest of
//! the app only ever holds a `Sender`, which keeps the engine and the UI free of COM.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

const DISPLAY_NAME: &str = "Q-Music";

/// State the shell asks the media session to show.
#[derive(Debug, Clone)]
pub enum MediaCommand {
    Track {
        title: String,
        artist: String,
        album: String,
        duration_ms: u64,
        cover_path: Option<PathBuf>,
    },
    Playback {
        playing: bool,
        position_ms: u64,
    },
    Clear,
}

/// Starts the media thread. A failure to create the controls (older Windows build,
/// no shell integration) leaves the channel open and silently drops commands, so
/// playback itself is never affected.
pub fn spawn(app: AppHandle, window_handle: isize) -> Sender<MediaCommand> {
    let (sender, receiver) = mpsc::channel::<MediaCommand>();

    std::thread::Builder::new()
        .name("qm-media".into())
        .spawn(move || {
            let config = souvlaki::PlatformConfig {
                display_name: DISPLAY_NAME,
                dbus_name: "app.qmusic.desktop",
                hwnd: Some(window_handle as *mut c_void),
            };

            let mut controls = match souvlaki::MediaControls::new(config) {
                Ok(controls) => controls,
                Err(_) => return,
            };

            let event_app = app.clone();
            if controls
                .attach(move |event| {
                    let _ = event_app.emit("media://control", control_name(event));
                })
                .is_err()
            {
                return;
            }

            while let Ok(command) = receiver.recv() {
                match command {
                    MediaCommand::Track { title, artist, album, duration_ms, cover_path } => {
                        let duration = Duration::from_millis(duration_ms);
                        let cover_url = cover_path.as_deref().map(file_url);
                        let _ = controls.set_metadata(souvlaki::MediaMetadata {
                            title: Some(title.as_str()),
                            artist: Some(artist.as_str()),
                            album: Some(album.as_str()),
                            cover_url: cover_url.as_deref(),
                            duration: Some(duration),
                        });
                    }
                    MediaCommand::Playback { playing, position_ms } => {
                        let position =
                            Some(souvlaki::MediaPosition(Duration::from_millis(position_ms)));
                        let state = if playing {
                            souvlaki::MediaPlayback::Playing { progress: position }
                        } else {
                            souvlaki::MediaPlayback::Paused { progress: position }
                        };
                        let _ = controls.set_playback(state);
                    }
                    MediaCommand::Clear => {
                        let _ = controls.set_playback(souvlaki::MediaPlayback::Stopped);
                    }
                }
            }
        })
        .ok();

    sender
}

/// A media-key press, named for the webview. The UI decides what each one means for
/// the queue, so the mapping stays in one place.
fn control_name(event: souvlaki::MediaControlEvent) -> &'static str {
    match event {
        souvlaki::MediaControlEvent::Play => "play",
        souvlaki::MediaControlEvent::Pause => "pause",
        souvlaki::MediaControlEvent::Toggle => "toggle",
        souvlaki::MediaControlEvent::Next => "next",
        souvlaki::MediaControlEvent::Previous => "previous",
        souvlaki::MediaControlEvent::Stop => "stop",
        souvlaki::MediaControlEvent::Seek(souvlaki::SeekDirection::Forward) => "seek-forward",
        souvlaki::MediaControlEvent::Seek(souvlaki::SeekDirection::Backward) => "seek-backward",
        souvlaki::MediaControlEvent::SeekBy(souvlaki::SeekDirection::Forward, _) => "seek-forward",
        souvlaki::MediaControlEvent::SeekBy(souvlaki::SeekDirection::Backward, _) => "seek-backward",
        souvlaki::MediaControlEvent::Raise => "raise",
        _ => "other",
    }
}

/// `C:\Music\cover.jpg` becomes `file:///C:/Music/cover.jpg`, which is the only form
/// the shell accepts for a local thumbnail.
fn file_url(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}
