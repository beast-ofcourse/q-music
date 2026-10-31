//! Reading and writing file tags.
//!
//! Metadata is the library's source of truth, so this module is deliberately
//! forgiving: a file with broken frames still contributes its path, size and
//! duration, and the caller only loses the fields that could not be read.

use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey, TagExt};
use lofty::probe::Probe;
use lofty::tag::Tag;

use crate::error::{CoreError, CoreResult};

/// Everything the index needs from one file, before it is merged with the database row.
#[derive(Debug, Clone)]
pub struct FileFacts {
    pub path: PathBuf,
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
    pub format: String,
    pub size_bytes: u64,
    pub mtime: i64,
    pub lyrics: Option<String>,
    pub picture: Option<Picture>,
}

#[derive(Debug, Clone)]
pub struct Picture {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// Stable identifier derived from the bytes, used as the art cache key.
    pub cache_key: String,
}

/// Fields a user may edit. `None` means "leave untouched".
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TagEdit {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
}

impl TagEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.album_artist.is_none()
            && self.genre.is_none()
            && self.year.is_none()
            && self.track_no.is_none()
            && self.disc_no.is_none()
    }
}

/// Extensions the decoder is expected to handle. Kept in one place so the scanner
/// and the file picker never disagree.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "m4b", "aac", "mp4", "wav", "wave", "aiff", "aif", "aifc", "ogg", "oga",
    "opus", "alac", "caf", "wv", "ape", "mka", "mkv", "m4r", "spx",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            let lowered = extension.to_ascii_lowercase();
            SUPPORTED_EXTENSIONS.contains(&lowered.as_str())
        })
        .unwrap_or(false)
}

/// Reads facts from a file. Audio properties are required; tags are best effort.
pub fn read_facts(path: &Path) -> CoreResult<FileFacts> {
    let metadata = std::fs::metadata(path).map_err(|error| CoreError::io(path, error))?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let tagged = Probe::open(path)
        .map_err(|error| CoreError::Tags { path: path.to_path_buf(), message: error.to_string() })?
        .read()
        .map_err(|error| CoreError::Tags { path: path.to_path_buf(), message: error.to_string() })?;

    let properties = tagged.properties();
    let duration_ms = properties.duration().as_millis() as u64;
    let sample_rate = properties.sample_rate();
    let channels = properties.channels().map(u32::from);
    let bit_depth = properties.bit_depth().map(u32::from);
    let bitrate_kbps = properties.audio_bitrate();

    let format = detect_format(path, &tagged);
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let mut facts = FileFacts {
        path: path.to_path_buf(),
        title: String::new(),
        artist: String::new(),
        album_artist: String::new(),
        album: String::new(),
        genre: String::new(),
        year: None,
        track_no: None,
        disc_no: None,
        duration_ms,
        sample_rate,
        channels,
        bit_depth,
        bitrate_kbps,
        format,
        size_bytes: metadata.len(),
        mtime,
        lyrics: None,
        picture: None,
    };

    if let Some(tag) = tag {
        facts.title = tag.title().map(|value| value.to_string()).unwrap_or_default();
        facts.artist = tag.artist().map(|value| value.to_string()).unwrap_or_default();
        facts.album = tag.album().map(|value| value.to_string()).unwrap_or_default();
        facts.genre = tag.genre().map(|value| value.to_string()).unwrap_or_default();
        facts.year = tag.date().and_then(|date| i32::try_from(date.year).ok());
        facts.track_no = tag.track();
        facts.disc_no = tag.disk();
        facts.album_artist = tag
            .get_string(ItemKey::AlbumArtist)
            .map(|value| value.to_string())
            .unwrap_or_default();
        // A dedicated lyrics field is trusted as-is; only a comment has to look
        // like a lyric sheet before it is treated as one.
        facts.lyrics = tag
            .get_string(ItemKey::Lyrics)
            .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                tag.get_string(ItemKey::Comment)
                    .map(|value| value.to_string())
                    .filter(|value| looks_like_lyrics(value))
            });

        if let Some(picture) = tag.pictures().first() {
            let bytes = picture.data().to_vec();
            if !bytes.is_empty() {
                facts.picture = Some(Picture {
                    cache_key: fingerprint(&bytes),
                    mime: picture
                        .mime_type()
                        .map(|mime| mime.to_string())
                        .unwrap_or_else(|| "image/jpeg".to_string()),
                    bytes,
                });
            }
        }
    }

    if facts.title.trim().is_empty() {
        facts.title = fallback_title(path);
    }
    if facts.album_artist.trim().is_empty() {
        facts.album_artist = facts.artist.clone();
    }

    Ok(facts)
}

/// Title from the filename, stripped of a leading track number.
fn fallback_title(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string());
    let trimmed = stem.trim_start_matches(|character: char| character.is_ascii_digit());
    let trimmed = trimmed.trim_start_matches([' ', '-', '_', '.']);
    if trimmed.is_empty() {
        stem
    } else {
        trimmed.to_string()
    }
}

fn detect_format(path: &Path, tagged: &lofty::file::TaggedFile) -> String {
    use lofty::file::FileType;
    match tagged.file_type() {
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MP3",
        FileType::Mp4 => {
            // M4A containers hold either AAC or ALAC; the extension tells them apart
            // when the user named the file carefully, otherwise report the container.
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("alac"))
                .unwrap_or(false)
            {
                "ALAC"
            } else {
                "AAC"
            }
        }
        FileType::Aac => "AAC",
        FileType::Wav => "WAV",
        FileType::Aiff => "AIFF",
        FileType::Opus => "OPUS",
        FileType::Vorbis => "VORBIS",
        FileType::Speex => "SPEEX",
        FileType::WavPack => "WAVPACK",
        FileType::Ape => "APE",
        FileType::Mpc => "MPC",
        other => return format!("{other:?}").to_uppercase(),
    }
    .to_string()
}

/// Writes edited fields back into the file's primary tag.
pub fn write_tags(path: &Path, edit: &TagEdit) -> CoreResult<()> {
    if edit.is_empty() {
        return Ok(());
    }

    let mut tagged = Probe::open(path)
        .map_err(|error| CoreError::Tags { path: path.to_path_buf(), message: error.to_string() })?
        .read()
        .map_err(|error| CoreError::Tags { path: path.to_path_buf(), message: error.to_string() })?;

    if tagged.primary_tag().is_none() && tagged.first_tag().is_none() {
        let tag_type = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(tag_type));
    }

    let tag = if tagged.primary_tag().is_some() {
        tagged.primary_tag_mut()
    } else {
        tagged.first_tag_mut()
    }
    .ok_or_else(|| CoreError::Tags {
        path: path.to_path_buf(),
        message: "no writable tag".to_string(),
    })?;

    if let Some(title) = &edit.title {
        tag.set_title(title.clone());
    }
    if let Some(artist) = &edit.artist {
        tag.set_artist(artist.clone());
    }
    if let Some(album) = &edit.album {
        tag.set_album(album.clone());
    }
    if let Some(genre) = &edit.genre {
        tag.set_genre(genre.clone());
    }
    if let Some(year) = edit.year {
        let year = u16::try_from(year.max(0)).unwrap_or(u16::MAX);
        tag.set_date(lofty::tag::items::Timestamp { year, ..Default::default() });
    }
    if let Some(track) = edit.track_no {
        tag.set_track(track);
    }
    if let Some(disc) = edit.disc_no {
        tag.set_disk(disc);
    }
    if let Some(album_artist) = &edit.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, album_artist.clone());
    }

    tag.save_to_path(path, WriteOptions::default()).map_err(|error| CoreError::Tags {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;

    Ok(())
}

/// FNV-1a over the image bytes. Short, stable, and cheap enough to run on every
/// scan for every file with art.
pub fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Comment tags are frequently release notes rather than lyrics. Require a stamp
/// or several lines before treating a comment as lyrics.
fn looks_like_lyrics(text: &str) -> bool {
    let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
    lines >= 3 || text.contains("[00:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_filter_accepts_lossless_and_lossy() {
        assert!(is_supported(Path::new("song.FLAC")));
        assert!(is_supported(Path::new("song.m4a")));
        assert!(is_supported(Path::new("song.aiff")));
        assert!(!is_supported(Path::new("cover.jpg")));
        assert!(!is_supported(Path::new("no-extension")));
    }

    #[test]
    fn filename_fallback_strips_track_numbers() {
        assert_eq!(fallback_title(Path::new("03 - Night Ferry.flac")), "Night Ferry");
        assert_eq!(fallback_title(Path::new("Untitled.wav")), "Untitled");
    }

    #[test]
    fn fingerprints_are_stable_and_distinct() {
        assert_eq!(fingerprint(b"abc"), fingerprint(b"abc"));
        assert_ne!(fingerprint(b"abc"), fingerprint(b"abd"));
    }

    #[test]
    fn comments_need_three_lines_or_a_stamp() {
        assert!(!looks_like_lyrics("thanks for listening"));
        assert!(looks_like_lyrics("a\nb\nc"));
        assert!(looks_like_lyrics("[00:01.00]one"));
    }
}
