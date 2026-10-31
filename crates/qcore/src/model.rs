//! Values shared by the library, the engine and the interface.

use serde::{Deserialize, Serialize};

/// A single audio file as the library knows it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_ms: u64,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bit_depth: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    /// Container/codec label shown in the technical strip: FLAC, ALAC, MP3, AAC, OPUS, VORBIS, WAV, AIFF.
    pub format: String,
    pub size_bytes: u64,
    /// File modification time at the last scan, used to skip unchanged files.
    pub mtime: i64,
    pub added_at: i64,
    pub play_count: u32,
    pub last_played_at: Option<i64>,
    pub favorite: bool,
    pub art_id: Option<String>,
    pub has_lyrics: bool,
}

impl Track {
    /// Display artist, falling back to the album artist when the tag is absent.
    pub fn display_artist(&self) -> &str {
        if self.artist.is_empty() {
            &self.album_artist
        } else {
            &self.artist
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl Default for RepeatMode {
    fn default() -> Self {
        RepeatMode::Off
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortField {
    Title,
    Artist,
    Album,
    Added,
    Duration,
    Year,
    PlayCount,
    Format,
}

impl Default for SortField {
    fn default() -> Self {
        SortField::Artist
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortOrder {
    Asc,
    Desc,
}

impl Default for SortOrder {
    fn default() -> Self {
        SortOrder::Asc
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaybackStatus {
    Idle,
    Playing,
    Paused,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub track_id: i64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    /// `manual` playlists hold an ordered list; `smart` playlists hold rules.
    pub kind: String,
    pub rules: Option<SmartRules>,
    pub track_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Rules for a smart playlist. Every rule must match (AND); `match_any` turns
/// the rule list into an OR so a user can build "jazz or blues".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartRules {
    #[serde(default)]
    pub match_any: bool,
    #[serde(default)]
    pub rules: Vec<SmartRule>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartRule {
    pub field: SmartField,
    pub op: SmartOp,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SmartField {
    Artist,
    Album,
    Genre,
    Title,
    Year,
    Format,
    Favorite,
    PlayCount,
    AddedDays,
    DurationSeconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SmartOp {
    Is,
    IsNot,
    Contains,
    GreaterThan,
    LessThan,
    InLast,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EqSettings {
    pub enabled: bool,
    /// Gain in dB for each of the ten bands, in the order of `EQ_FREQUENCIES`.
    pub gains: Vec<f32>,
    pub preamp_db: f32,
}

impl EqSettings {
    pub fn flat() -> Self {
        Self {
            enabled: false,
            gains: vec![0.0; crate::eq::EQ_FREQUENCIES.len()],
            preamp_db: 0.0,
        }
    }

    /// Repairs a persisted preset that does not match the current band count.
    pub fn normalized(mut self) -> Self {
        let bands = crate::eq::EQ_FREQUENCIES.len();
        self.gains.resize(bands, 0.0);
        for gain in &mut self.gains {
            *gain = gain.clamp(-12.0, 12.0);
        }
        self.preamp_db = self.preamp_db.clamp(-12.0, 12.0);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LyricsSource {
    /// Lyrics read from the file's own tag.
    Embedded,
    /// A sidecar `.lrc`/`.txt` next to the audio file.
    Sidecar,
    /// Lyrics typed in the app and stored in the library.
    Manual,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    /// Milliseconds from the start of the track, `None` for unsynced lines.
    pub time_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub track_id: i64,
    pub source: LyricsSource,
    pub lines: Vec<LyricLine>,
    /// True when every line carries a timestamp.
    pub synced: bool,
    /// Populated when the source file could not be parsed cleanly.
    pub warning: Option<String>,
}

/// One album or artist as the browse lists show it: a name, its totals and one piece
/// of artwork to stand for the whole group.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackGroup {
    pub name: String,
    /// The artist for an album group; empty for an artist group.
    pub subtitle: String,
    pub track_count: usize,
    pub duration_ms: u64,
    pub art_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    pub id: i64,
    pub path: String,
    pub added_at: i64,
    pub track_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub tracks: usize,
    pub albums: usize,
    pub artists: usize,
    pub total_duration_ms: u64,
    pub total_bytes: u64,
    pub last_scan_at: Option<i64>,
    pub scanning: bool,
}

/// Progress of a running scan. Emitted often enough to feel live, throttled
/// enough not to flood the interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanning: bool,
    pub folder: String,
    pub files_seen: usize,
    pub tracks_added: usize,
    pub tracks_updated: usize,
    pub tracks_removed: usize,
    pub failures: Vec<String>,
    pub finished_at: Option<i64>,
}

impl Default for ScanProgress {
    fn default() -> Self {
        Self {
            scanning: false,
            folder: String::new(),
            files_seen: 0,
            tracks_added: 0,
            tracks_updated: 0,
            tracks_removed: 0,
            failures: Vec::new(),
            finished_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevice {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub status: PlaybackStatus,
    pub path: Option<String>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    pub speed: f32,
    pub device: String,
    pub sample_rate: u32,
    /// Smoothed RMS of what the engine is sending to the output, 0.0..=1.0.
    pub level: f32,
    /// Highest sample magnitude in the last block, 0.0..=1.0.
    pub peak: f32,
    /// The track that reached its end since the last poll, so the queue can advance.
    pub ended_path: Option<String>,
    pub error: Option<String>,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Idle,
            path: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 0.85,
            speed: 1.0,
            device: String::new(),
            sample_rate: 48_000,
            level: 0.0,
            peak: 0.0,
            ended_path: None,
            error: None,
        }
    }
}
