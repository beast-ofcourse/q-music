//! Data shapes extensions exchange with the host.
//!
//! Field names follow the shipped packages, not the development guide: tracks
//! use `album_name`, `duration_ms`, `images`; availability uses `track_id`,
//! `prepared_context`. Everything a package adds beyond those is kept in
//! `extra` so a track can be handed back to JavaScript unchanged.
//!
//! Tolerances exist because packages disagree:
//! - `images` and `artists` are sometimes a string, sometimes an array
//! - `duration_ms` is sometimes a number, sometimes a numeric string
//! - failure text arrives as `error`, `error_message` or `message`

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::BTreeMap;

/// A value that packages may send either bare or as a single-item array.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OneOrMany<T>(pub Vec<T>);

impl<T> OneOrMany<T> {
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn first(&self) -> Option<&T> {
        self.0.first()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl OneOrMany<String> {
    /// All values joined the way packages render a multi-artist string.
    pub fn joined(&self) -> String {
        self.0.join(", ")
    }
}

impl<T: Serialize> Serialize for OneOrMany<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.len() {
            1 => self.0[0].serialize(serializer),
            _ => self.0.serialize(serializer),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrManyRepr<T> {
    One(T),
    Many(Vec<T>),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OneOrMany<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match OneOrManyRepr::<T>::deserialize(deserializer)? {
            OneOrManyRepr::One(value) => Ok(OneOrMany(vec![value])),
            OneOrManyRepr::Many(values) => Ok(OneOrMany(values)),
        }
    }
}

/// Milliseconds as a [`u64`], accepting numbers, numeric strings and `null`.
fn deserialize_duration<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(0),
        Value::Number(number) => number
            .as_u64()
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| value as u64)
            })
            .ok_or_else(|| D::Error::custom(format!("duration {number} is not a millisecond count"))),
        Value::String(text) => text
            .trim()
            .parse::<u64>()
            .or_else(|_| {
                text.trim()
                    .parse::<f64>()
                    .map(|value| value.max(0.0) as u64)
            })
            .map_err(|_| D::Error::custom(format!("duration {text:?} is not a number"))),
        other => Err(D::Error::custom(format!(
            "duration must be a number, got {other}"
        ))),
    }
}

/// A track as returned by search, album, playlist and feed functions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,

    #[serde(default)]
    pub artists: OneOrMany<String>,

    #[serde(default)]
    pub album_name: Option<String>,

    #[serde(default)]
    pub album_artist: Option<String>,

    #[serde(default)]
    pub isrc: Option<String>,

    #[serde(default, deserialize_with = "deserialize_duration")]
    pub duration_ms: u64,

    #[serde(default)]
    pub track_number: Option<u32>,

    #[serde(default)]
    pub disc_number: Option<u32>,

    #[serde(default)]
    pub release_date: Option<String>,

    #[serde(default)]
    pub images: OneOrMany<String>,

    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

impl Track {
    /// Best available artwork URL, preferring the documented `images` field.
    pub fn artwork_url(&self) -> Option<&str> {
        self.images
            .first()
            .map(String::as_str)
            .or_else(|| self.extra.get("cover_url").and_then(Value::as_str))
    }

    pub fn artist_names(&self) -> String {
        self.artists.joined()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,

    #[serde(default)]
    pub artists: OneOrMany<String>,

    #[serde(default)]
    pub release_date: Option<String>,

    #[serde(default)]
    pub total_tracks: Option<u32>,

    #[serde(default)]
    pub images: OneOrMany<String>,

    #[serde(default)]
    pub album_type: Option<String>,

    #[serde(default)]
    pub tracks: Vec<Track>,

    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,

    #[serde(default)]
    pub images: OneOrMany<String>,

    #[serde(default)]
    pub albums: Vec<Album>,

    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// Result of a lyrics provider's `fetchLyrics`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsResult {
    #[serde(default)]
    pub lines: Vec<LyricsLine>,

    /// `LINE_SYNCED`, `WORD_SYNCED` or `UNSYNCED`. Kept as a string so a new
    /// synchronisation mode is reported rather than rejected.
    #[serde(default)]
    pub sync_type: String,

    #[serde(default)]
    pub instrumental: bool,

    #[serde(default)]
    pub plain_lyrics: Option<String>,

    #[serde(default)]
    pub provider: Option<String>,

    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

impl LyricsResult {
    pub fn is_synced(&self) -> bool {
        matches!(self.sync_type.as_str(), "LINE_SYNCED" | "WORD_SYNCED")
    }

    /// True when there is something worth showing: either timed lines or text.
    pub fn has_content(&self) -> bool {
        self.instrumental
            || !self.lines.is_empty()
            || self
                .plain_lyrics
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLine {
    #[serde(default, deserialize_with = "deserialize_duration")]
    pub start_time_ms: u64,

    #[serde(default)]
    pub words: String,

    /// Absent or zero when the provider does not know the line end.
    #[serde(default, deserialize_with = "deserialize_duration")]
    pub end_time_ms: u64,

    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// Result of a download provider's `download`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadResult {
    pub success: bool,

    #[serde(default)]
    pub file_path: Option<String>,

    #[serde(default)]
    pub format: Option<String>,

    #[serde(default)]
    pub actual_bit_depth: Option<u32>,

    #[serde(default)]
    pub actual_sample_rate: Option<u32>,

    #[serde(default)]
    pub title: Option<String>,

    #[serde(default)]
    pub artist: Option<String>,

    #[serde(default)]
    pub album: Option<String>,

    #[serde(default)]
    pub album_artist: Option<String>,

    #[serde(default)]
    pub track_number: Option<u32>,

    #[serde(default)]
    pub disc_number: Option<u32>,

    #[serde(default)]
    pub release_date: Option<String>,

    #[serde(default)]
    pub cover_url: Option<String>,

    #[serde(default)]
    pub isrc: Option<String>,

    /// Documented failure key.
    #[serde(default)]
    pub error: Option<String>,

    /// Failure key used by shipped packages.
    #[serde(default)]
    pub error_message: Option<String>,

    /// Third spelling seen in the wild.
    #[serde(default)]
    pub message: Option<String>,

    /// One of the documented `error_type` values, or a package-specific one.
    #[serde(default)]
    pub error_type: Option<String>,

    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

impl DownloadResult {
    /// Failure text regardless of which key the package used.
    pub fn error_text(&self) -> Option<&str> {
        self.error_message
            .as_deref()
            .or(self.error.as_deref())
            .or(self.message.as_deref())
    }
}

/// Result of a download provider's `checkAvailability`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckAvailability {
    pub available: bool,

    #[serde(default)]
    pub track_id: Option<String>,

    #[serde(default)]
    pub reason: Option<String>,

    /// Opaque context a package wants echoed back into `download`.
    #[serde(default)]
    pub prepared_context: Option<Value>,

    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape produced by `spotify-web`'s search mapping, copied from
    /// `fixtures/sources/spotify-web.index.js`.
    const SPOTIFY_TRACK: &str = r#"{
        "id": "4uLU6hMCjMI75M1A2tKUQC",
        "spotify_id": "4uLU6hMCjMI75M1A2tKUQC",
        "name": "Never Gonna Give You Up",
        "artists": "Rick Astley",
        "album_name": "Whenever You Need Somebody",
        "album_artist": "Rick Astley",
        "artist_id": "0gxyHStUsqpMadRV0Di1Qt",
        "artist_url": "https://open.spotify.com/artist/0gxyHStUsqpMadRV0Di1Qt",
        "duration_ms": 213573,
        "images": "https://i.scdn.co/image/ab67616d0000b2739",
        "cover_url": "https://i.scdn.co/image/ab67616d0000b2739",
        "preview_url": "",
        "release_date": "1987-11-12",
        "album_type": "album",
        "track_number": 1,
        "total_tracks": 10,
        "disc_number": 1,
        "total_discs": 0,
        "external_urls": "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
        "isrc": "",
        "explicit": false,
        "album_id": "6XhjNHCyCDyyGJRM5mg40G",
        "label": "",
        "copyright": ""
    }"#;

    #[test]
    fn parses_a_real_spotify_shaped_track() {
        let track: Track = serde_json::from_str(SPOTIFY_TRACK).expect("track parses");

        assert_eq!(track.id, "4uLU6hMCjMI75M1A2tKUQC");
        assert_eq!(track.name, "Never Gonna Give You Up");
        assert_eq!(track.artist_names(), "Rick Astley");
        assert_eq!(track.duration_ms, 213_573);
        assert_eq!(track.track_number, Some(1));
        assert_eq!(track.disc_number, Some(1));
        assert_eq!(
            track.album_name.as_deref(),
            Some("Whenever You Need Somebody")
        );
        assert_eq!(track.release_date.as_deref(), Some("1987-11-12"));
        assert_eq!(track.images.as_slice().len(), 1);
        assert_eq!(track.artwork_url(), Some("https://i.scdn.co/image/ab67616d0000b2739"));
    }

    #[test]
    fn keeps_fields_the_host_does_not_model() {
        let track: Track = serde_json::from_str(SPOTIFY_TRACK).expect("track parses");

        for key in ["spotify_id", "artist_id", "explicit", "album_id", "total_tracks"] {
            assert!(track.extra.contains_key(key), "missing extra field {key}");
        }

        let reserialised = serde_json::to_value(&track).expect("serialises");
        assert_eq!(reserialised["spotify_id"], "4uLU6hMCjMI75M1A2tKUQC");
        assert_eq!(reserialised["explicit"], false);
        assert_eq!(reserialised["total_tracks"], 10);
    }

    #[test]
    fn accepts_images_as_array() {
        let json = r#"{
            "id": "x", "name": "y", "duration_ms": 1000,
            "images": ["https://a/1.jpg", "https://a/2.jpg"]
        }"#;

        let track: Track = serde_json::from_str(json).expect("array images parse");
        assert_eq!(track.images.as_slice(), ["https://a/1.jpg", "https://a/2.jpg"]);
        assert_eq!(track.artwork_url(), Some("https://a/1.jpg"));
    }

    #[test]
    fn accepts_artists_as_array() {
        let json = r#"{ "id": "x", "name": "y", "artists": ["A", "B"] }"#;
        let track: Track = serde_json::from_str(json).expect("array artists parse");
        assert_eq!(track.artist_names(), "A, B");
    }

    #[test]
    fn accepts_duration_as_numeric_string() {
        let json = r#"{ "id": "x", "name": "y", "duration_ms": "240000" }"#;
        let track: Track = serde_json::from_str(json).expect("string duration parses");
        assert_eq!(track.duration_ms, 240_000);
    }

    #[test]
    fn defaults_missing_optional_fields() {
        let json = r#"{ "id": "x", "name": "y" }"#;
        let track: Track = serde_json::from_str(json).expect("minimal track parses");

        assert_eq!(track.duration_ms, 0);
        assert!(track.images.is_empty());
        assert!(track.artists.is_empty());
        assert_eq!(track.artwork_url(), None);
        assert!(track.extra.is_empty());
    }

    #[test]
    fn rejects_track_without_id() {
        let json = r#"{ "name": "y" }"#;
        let error = serde_json::from_str::<Track>(json).expect_err("id is required");
        assert!(error.to_string().contains("id"), "error names the field: {error}");
    }

    #[test]
    fn rejects_track_with_wrong_field_type() {
        let json = r#"{ "id": 42, "name": "y" }"#;
        let error = serde_json::from_str::<Track>(json).expect_err("id must be a string");
        assert!(error.to_string().contains("id"), "error names the field: {error}");
    }

    #[test]
    fn rejects_duration_that_is_not_a_number() {
        let json = r#"{ "id": "x", "name": "y", "duration_ms": "forever" }"#;
        let error = serde_json::from_str::<Track>(json).expect_err("bad duration rejected");
        assert!(error.to_string().contains("duration"), "error explains: {error}");
    }

    #[test]
    fn parses_album_with_tracks() {
        let json = r#"{
            "id": "album1",
            "name": "Whenever You Need Somebody",
            "artists": "Rick Astley",
            "release_date": "1987-11-12",
            "total_tracks": 10,
            "images": "https://a/cover.jpg",
            "album_type": "album",
            "tracks": [
                { "id": "t1", "name": "One", "duration_ms": 1000 },
                { "id": "t2", "name": "Two", "duration_ms": 2000 }
            ]
        }"#;

        let album: Album = serde_json::from_str(json).expect("album parses");
        assert_eq!(album.tracks.len(), 2);
        assert_eq!(album.total_tracks, Some(10));
        assert_eq!(album.album_type.as_deref(), Some("album"));
        assert_eq!(album.images.first().map(String::as_str), Some("https://a/cover.jpg"));
    }

    #[test]
    fn parses_artist_with_albums() {
        let json = r#"{
            "id": "artist1",
            "name": "Rick Astley",
            "images": "https://a/artist.jpg",
            "albums": [{ "id": "a1", "name": "Album" }]
        }"#;

        let artist: Artist = serde_json::from_str(json).expect("artist parses");
        assert_eq!(artist.name, "Rick Astley");
        assert_eq!(artist.albums.len(), 1);
    }

    #[test]
    fn parses_synced_lyrics() {
        let json = r#"{
            "lines": [
                { "startTimeMs": 1200, "words": "First line", "endTimeMs": 4500 },
                { "startTimeMs": 4500, "words": "Second line" }
            ],
            "syncType": "LINE_SYNCED",
            "instrumental": false,
            "plainLyrics": "",
            "provider": "My Lyrics Source"
        }"#;

        let lyrics: LyricsResult = serde_json::from_str(json).expect("lyrics parse");
        assert!(lyrics.is_synced());
        assert!(lyrics.has_content());
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].start_time_ms, 1200);
        assert_eq!(lyrics.lines[0].end_time_ms, 4500);
        assert_eq!(lyrics.lines[1].end_time_ms, 0);
        assert_eq!(lyrics.provider.as_deref(), Some("My Lyrics Source"));
    }

    #[test]
    fn parses_unsynced_lyrics() {
        let json = r#"{
            "lines": [],
            "syncType": "UNSYNCED",
            "instrumental": false,
            "plainLyrics": "Line one\nLine two",
            "provider": "My Lyrics Source"
        }"#;

        let lyrics: LyricsResult = serde_json::from_str(json).expect("plain lyrics parse");
        assert!(!lyrics.is_synced());
        assert!(lyrics.has_content());
        assert_eq!(lyrics.plain_lyrics.as_deref(), Some("Line one\nLine two"));
    }

    #[test]
    fn parses_instrumental_lyrics() {
        let json = r#"{ "lines": [], "syncType": "LINE_SYNCED", "instrumental": true }"#;
        let lyrics: LyricsResult = serde_json::from_str(json).expect("instrumental parses");
        assert!(lyrics.instrumental);
        assert!(lyrics.has_content());
    }

    #[test]
    fn reports_empty_lyrics_as_without_content() {
        let json = r#"{ "lines": [], "syncType": "UNSYNCED", "plainLyrics": "   " }"#;
        let lyrics: LyricsResult = serde_json::from_str(json).expect("empty lyrics parse");
        assert!(!lyrics.has_content());
    }

    #[test]
    fn parses_download_success() {
        let json = r#"{
            "success": true,
            "file_path": "/music/track.flac",
            "format": "flac",
            "actual_bit_depth": 24,
            "actual_sample_rate": 96000,
            "title": "Track Name",
            "artist": "Artist Name",
            "cover_url": "https://a/cover.jpg"
        }"#;

        let result: DownloadResult = serde_json::from_str(json).expect("success parses");
        assert!(result.success);
        assert_eq!(result.file_path.as_deref(), Some("/music/track.flac"));
        assert_eq!(result.actual_bit_depth, Some(24));
        assert_eq!(result.error_text(), None);
    }

    #[test]
    fn reads_failure_text_from_any_key() {
        let shipped = r#"{ "success": false, "error_message": "Invalid track ID", "error_type": "not_found" }"#;
        let documented = r#"{ "success": false, "error": "Track not found", "error_type": "not_found" }"#;
        let third = r#"{ "success": false, "message": "Rate limited" }"#;

        let shipped: DownloadResult = serde_json::from_str(shipped).expect("parses");
        assert_eq!(shipped.error_text(), Some("Invalid track ID"));
        assert_eq!(shipped.error_type.as_deref(), Some("not_found"));

        let documented: DownloadResult = serde_json::from_str(documented).expect("parses");
        assert_eq!(documented.error_text(), Some("Track not found"));

        let third: DownloadResult = serde_json::from_str(third).expect("parses");
        assert_eq!(third.error_text(), Some("Rate limited"));
    }

    #[test]
    fn parses_check_availability_hit() {
        let json = r#"{
            "available": true,
            "track_id": "B00XYZ",
            "prepared_context": { "web_metadata": { "asin": "B00XYZ" } }
        }"#;

        let availability: CheckAvailability = serde_json::from_str(json).expect("parses");
        assert!(availability.available);
        assert_eq!(availability.track_id.as_deref(), Some("B00XYZ"));
        assert!(availability.prepared_context.is_some());
    }

    #[test]
    fn parses_check_availability_miss() {
        let json = r#"{ "available": false, "reason": "not_found_on_amazon" }"#;
        let availability: CheckAvailability = serde_json::from_str(json).expect("parses");
        assert!(!availability.available);
        assert_eq!(availability.reason.as_deref(), Some("not_found_on_amazon"));
        assert_eq!(availability.track_id, None);
    }

    #[test]
    fn serialises_single_image_back_to_a_string() {
        let track = Track {
            id: "x".into(),
            name: "y".into(),
            images: OneOrMany(vec!["https://a/1.jpg".into()]),
            ..serde_json::from_str::<Track>(r#"{ "id": "x", "name": "y" }"#).expect("base track")
        };

        let value = serde_json::to_value(&track).expect("serialises");
        assert_eq!(value["images"], "https://a/1.jpg");
    }
}
