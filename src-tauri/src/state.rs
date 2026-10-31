//! Process-wide state shared by every command.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use qcore::{Engine, Library};

use crate::media::MediaCommand;

pub struct AppState {
    /// The index and artwork cache the interface reads.
    pub library: Mutex<Library>,
    /// The playback engine. Scanning deliberately uses its own `Library` handle so a
    /// long scan never blocks a play or a search.
    pub engine: Mutex<Engine>,
    /// Where the index lives, so background scans can open their own handle.
    pub root: PathBuf,
    pub media: Sender<MediaCommand>,
}
