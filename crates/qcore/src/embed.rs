//! Writing metadata back into audio files.
//!
//! Embedding is the stage after a download: the host has title, artist, album,
//! artwork and lyrics in memory, and the file on disk has none of it. One pass
//! writes the whole set and saves once, because every `save_to_path` rewrites the
//! container's tag block and doing that per field would be needlessly destructive
//! on large FLAC files with embedded art.
//!
//! Three format differences are handled explicitly rather than papered over:
//!
//! - **Vorbis comments (FLAC, OGG, OPUS)** carry `LYRICS` and `UNSYNCEDLYRICS`
//!   as separate fields, so a synced set is written twice: the LRC document that
//!   players follow, and a plain sheet for players that ignore timestamps.
//! - **ID3v2 (MP3)** has no `LYRICS` field. Timed lyrics live in a binary `SYLT`
//!   frame, so they are written by a second pass over the same file. `USLT`
//!   receives the plain sheet.
//! - **MP4 (`©lyr`)** maps one atom to both keys, so only one is written; writing
//!   both would have the second overwrite the first.
//!
//! The sidecar `.lrc` is written whenever the set is synced, next to the audio
//! file, because that is what a user's other players and editors read.

use std::borrow::Cow;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::id3::v2::{
    BinaryFrame, Frame, FrameId, SyncTextContentType, SynchronizedTextFrame, TimestampFormat,
};
use lofty::mpeg::MpegFile;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::probe::Probe;
use lofty::tag::{Tag, TagType};
use lofty::TextEncoding;

use crate::error::{CoreError, CoreResult};
use crate::lrc;
use crate::model::LyricLine;

/// Cover art to embed, with the content type the file should declare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub bytes: Vec<u8>,
    pub mime: String,
}

impl Artwork {
    pub fn new(bytes: impl Into<Vec<u8>>, mime: impl Into<String>) -> Self {
        Self {
            bytes: bytes.into(),
            mime: mime.into(),
        }
    }
}

/// Everything one embed pass should write. `None` means "leave what is there".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbedSpec {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub isrc: Option<String>,
    pub artwork: Option<Artwork>,
    /// Empty means "no lyrics to write". Order and stamps are preserved.
    pub lyrics: Vec<LyricLine>,
}

impl EmbedSpec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_lyrics(mut self, lines: Vec<LyricLine>) -> Self {
        self.lyrics = lines;
        self
    }

    pub fn with_artwork(mut self, artwork: Artwork) -> Self {
        self.artwork = Some(artwork);
        self
    }
}

/// What the lyric half of the pass actually produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedLyrics {
    /// No lyrics were supplied, or every line was blank.
    None,
    /// A plain sheet was embedded; no `.lrc` was written.
    Plain,
    /// A timed set was embedded and a `.lrc` sidecar was written.
    Synced,
}

/// What one embed pass did, for the caller to report and for tests to assert on.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedReport {
    pub path: PathBuf,
    /// Bytes of artwork written; zero when no artwork was supplied.
    pub artwork_bytes: usize,
    pub lyrics: EmbeddedLyrics,
    pub sidecar: Option<PathBuf>,
}

/// Write `spec` into the file's primary tag, then add a `.lrc` sidecar when the
/// lyrics carry timestamps.
pub fn embed(path: &Path, spec: &EmbedSpec) -> CoreResult<EmbedReport> {
    ensure_writable(path)?;

    let mut tagged = read_tagged(path)?;
    let (lyrics, tag_type) = {
        let tag = primary_tag_mut(&mut tagged, path)?;
        write_fields(tag, spec);
        write_artwork(tag, spec);
        let lyrics = write_lyrics(tag, &spec.lyrics);
        (lyrics, tag.tag_type())
    };

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|error| tag_error(path, error))?;

    // `SYLT` has no `ItemKey`, so a timed ID3 set needs a second, narrow pass.
    if lyrics == EmbeddedLyrics::Synced && tag_type == TagType::Id3v2 {
        write_synchronized_frame(path, &spec.lyrics)?;
    }

    let sidecar = if lyrics == EmbeddedLyrics::Synced {
        Some(write_sidecar(path, &spec.lyrics)?)
    } else {
        None
    };

    Ok(EmbedReport {
        path: path.to_path_buf(),
        artwork_bytes: spec.artwork.as_ref().map(|art| art.bytes.len()).unwrap_or(0),
        lyrics,
        sidecar,
    })
}

/// A read-only file cannot take tags, and saying so is better than a raw
/// "Access is denied" from the writer.
fn ensure_writable(path: &Path) -> CoreResult<()> {
    let metadata = fs::metadata(path).map_err(|error| CoreError::io(path, error))?;
    if metadata.permissions().readonly() {
        return Err(CoreError::ReadOnly {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn read_tagged(path: &Path) -> CoreResult<TaggedFile> {
    Probe::open(path)
        .map_err(|error| tag_error(path, error))?
        .read()
        .map_err(|error| tag_error(path, error))
}

/// The tag to write, creating an empty one of the file's own type when the
/// container has none.
fn primary_tag_mut<'a>(tagged: &'a mut TaggedFile, path: &Path) -> CoreResult<&'a mut Tag> {
    if tagged.primary_tag().is_none() && tagged.first_tag().is_none() {
        let tag_type = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(tag_type));
    }

    if tagged.primary_tag().is_some() {
        tagged.primary_tag_mut()
    } else {
        tagged.first_tag_mut()
    }
    .ok_or_else(|| CoreError::Tags {
        path: path.to_path_buf(),
        message: "the container has no tag this host can write".to_string(),
    })
}

fn write_fields(tag: &mut Tag, spec: &EmbedSpec) {
    if let Some(title) = &spec.title {
        tag.set_title(title.clone());
    }
    if let Some(artist) = &spec.artist {
        tag.set_artist(artist.clone());
    }
    if let Some(album) = &spec.album {
        tag.set_album(album.clone());
    }
    if let Some(album_artist) = &spec.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, album_artist.clone());
    }
    if let Some(genre) = &spec.genre {
        tag.set_genre(genre.clone());
    }
    if let Some(year) = spec.year {
        tag.set_date(lofty::tag::items::Timestamp {
            year: u16::try_from(year.max(0)).unwrap_or(u16::MAX),
            ..Default::default()
        });
    }
    if let Some(track) = spec.track_no {
        tag.set_track(track);
    }
    if let Some(disc) = spec.disc_no {
        tag.set_disk(disc);
    }
    if let Some(isrc) = &spec.isrc {
        tag.insert_text(ItemKey::Isrc, isrc.clone());
    }
}

fn write_artwork(tag: &mut Tag, spec: &EmbedSpec) {
    let Some(artwork) = &spec.artwork else {
        return;
    };
    if artwork.bytes.is_empty() {
        return;
    }

    // The front cover is a single slot. Leaving the old one in place would give
    // the file two, and readers disagree about which wins.
    tag.remove_picture_type(PictureType::CoverFront);
    tag.push_picture(
        Picture::unchecked(artwork.bytes.clone())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::from_str(&artwork.mime))
            .description("Cover")
            .build(),
    );
}

/// Write whatever lyric keys the container actually has, and report which form
/// was used so the caller knows whether a sidecar follows.
fn write_lyrics(tag: &mut Tag, lines: &[LyricLine]) -> EmbeddedLyrics {
    let usable: Vec<&LyricLine> = lines
        .iter()
        .filter(|line| !line.text.trim().is_empty())
        .collect();

    if usable.is_empty() {
        return EmbeddedLyrics::None;
    }

    let synced = usable.iter().all(|line| line.time_ms.is_some());
    let plain = plain_sheet(&usable);

    if synced {
        let timed = lrc::to_lrc(lines);
        match tag.tag_type() {
            // ID3v2 has USLT for a plain sheet and a binary SYLT frame for a
            // timed one, which `embed` writes separately.
            TagType::Id3v2 => {
                tag.insert_text(ItemKey::UnsyncLyrics, plain);
            }
            // `©lyr` serves both keys; the timed text is the richer one.
            TagType::Mp4Ilst => {
                tag.insert_text(ItemKey::Lyrics, timed);
            }
            _ => {
                tag.insert_text(ItemKey::Lyrics, timed);
                tag.insert_text(ItemKey::UnsyncLyrics, plain);
            }
        }
        EmbeddedLyrics::Synced
    } else {
        tag.insert_text(ItemKey::UnsyncLyrics, plain);
        EmbeddedLyrics::Plain
    }
}

fn plain_sheet(lines: &[&LyricLine]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line.text.trim());
        out.push('\n');
    }
    out
}

/// Insert a real `SYLT` frame so players that follow timestamps have them.
fn write_synchronized_frame(path: &Path, lines: &[LyricLine]) -> CoreResult<()> {
    let content: Vec<(u32, String)> = lines
        .iter()
        .filter(|line| !line.text.trim().is_empty())
        .filter_map(|line| {
            let stamp = line.time_ms?;
            Some((u32::try_from(stamp).unwrap_or(u32::MAX), line.text.clone()))
        })
        .collect();

    if content.is_empty() {
        return Ok(());
    }

    let file = File::open(path).map_err(|error| CoreError::io(path, error))?;
    let mut reader = BufReader::new(file);
    let mut mpeg = MpegFile::read_from(&mut reader, ParseOptions::default())
        .map_err(|error| tag_error(path, error))?;

    let frame = SynchronizedTextFrame::new(
        TextEncoding::UTF8,
        *b"eng",
        TimestampFormat::MS,
        SyncTextContentType::Lyrics,
        None,
        content,
    );
    let bytes = frame
        .as_bytes(WriteOptions::default())
        .map_err(|error| tag_error(path, error))?;

    let id3 = mpeg.id3v2_mut().ok_or_else(|| CoreError::Tags {
        path: path.to_path_buf(),
        message: "the MP3 has no ID3v2 tag to hold a SYLT frame".to_string(),
    })?;
    id3.insert(Frame::Binary(BinaryFrame::new(
        FrameId::Valid(Cow::Borrowed("SYLT")),
        bytes,
    )));

    mpeg.save_to_path(path, WriteOptions::default())
        .map_err(|error| tag_error(path, error))
}

fn write_sidecar(path: &Path, lines: &[LyricLine]) -> CoreResult<PathBuf> {
    let sidecar = path.with_extension("lrc");
    fs::write(&sidecar, lrc::to_lrc(lines)).map_err(|error| CoreError::io(&sidecar, error))?;
    Ok(sidecar)
}

fn tag_error(path: &Path, error: impl std::fmt::Display) -> CoreError {
    CoreError::Tags {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags;

    /// The fixtures are real 0.35 s streams, not empty placeholders: Lofty has to
    /// parse a container before it can rewrite its tag block.
    fn fixture(extension: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/audio")
            .join(format!("seed.{extension}"))
    }

    const FORMATS: [&str; 5] = ["flac", "mp3", "m4a", "ogg", "opus"];

    fn copy_fixture(directory: &Path, extension: &str) -> PathBuf {
        let source = fixture(extension);
        assert!(
            source.is_file(),
            "missing audio fixture {}: run the fixture generator",
            source.display()
        );
        let target = directory.join(format!("track.{extension}"));
        fs::copy(&source, &target).expect("fixture copies");
        target
    }

    fn full_spec() -> EmbedSpec {
        EmbedSpec {
            title: Some("Night Ferry".to_string()),
            artist: Some("Harbour Lights".to_string()),
            album: Some("Tidal".to_string()),
            album_artist: Some("Harbour Lights".to_string()),
            genre: Some("Ambient".to_string()),
            year: Some(2024),
            track_no: Some(3),
            disc_no: Some(1),
            isrc: Some("GBAAA2400001".to_string()),
            artwork: None,
            lyrics: Vec::new(),
        }
    }

    fn synced_lyrics() -> Vec<LyricLine> {
        vec![
            LyricLine {
                time_ms: Some(1_000),
                text: "first line".to_string(),
            },
            LyricLine {
                time_ms: Some(4_500),
                text: "second line".to_string(),
            },
            LyricLine {
                time_ms: Some(9_250),
                text: "third line".to_string(),
            },
        ]
    }

    fn plain_lyrics() -> Vec<LyricLine> {
        vec![
            LyricLine {
                time_ms: None,
                text: "unmarked one".to_string(),
            },
            LyricLine {
                time_ms: None,
                text: "unmarked two".to_string(),
            },
        ]
    }

    #[test]
    fn every_format_round_trips_its_metadata() {
        for extension in FORMATS {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = copy_fixture(directory.path(), extension);

            let report = embed(&path, &full_spec()).expect("embeds");
            assert_eq!(report.path, path, "{extension}");

            let facts = tags::read_facts(&path).expect("reads back");
            assert_eq!(facts.title, "Night Ferry", "{extension} title");
            assert_eq!(facts.artist, "Harbour Lights", "{extension} artist");
            assert_eq!(facts.album, "Tidal", "{extension} album");
            assert_eq!(
                facts.album_artist, "Harbour Lights",
                "{extension} album artist"
            );
            assert_eq!(facts.genre, "Ambient", "{extension} genre");
            assert_eq!(facts.year, Some(2024), "{extension} year");
            assert_eq!(facts.track_no, Some(3), "{extension} track");
            assert_eq!(facts.disc_no, Some(1), "{extension} disc");
        }
    }

    #[test]
    fn isrc_is_written_for_every_format() {
        for extension in FORMATS {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = copy_fixture(directory.path(), extension);

            let mut spec = full_spec();
            spec.isrc = Some("GBAAA2400001".to_string());
            embed(&path, &spec).expect("embeds");

            // `FileFacts` has no ISRC because the index has no column for it, so
            // this reads the tag directly through the same Lofty accessor.
            let tagged = read_tagged(&path).expect("opens");
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
            let isrc = tag
                .and_then(|tag| tag.get_string(ItemKey::Isrc))
                .map(str::to_string);

            assert_eq!(
                isrc.as_deref(),
                Some("GBAAA2400001"),
                "{extension} isrc"
            );
        }
    }

    #[test]
    fn artwork_bytes_survive_the_round_trip() {
        // A minimal but structurally valid PNG header plus payload: Lofty stores
        // the bytes verbatim and only reads the MIME type we declare.
        let artwork = Artwork::new(
            vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4, 5],
            "image/png",
        );

        for extension in FORMATS {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = copy_fixture(directory.path(), extension);

            let mut spec = full_spec();
            spec.artwork = Some(artwork.clone());
            let report = embed(&path, &spec).expect("embeds");

            assert_eq!(
                report.artwork_bytes,
                artwork.bytes.len(),
                "{extension} report"
            );

            let facts = tags::read_facts(&path).expect("reads back");
            let picture = facts
                .picture
                .unwrap_or_else(|| panic!("{extension} lost its artwork"));
            assert_eq!(picture.bytes.len(), artwork.bytes.len(), "{extension} bytes");
            assert_eq!(picture.bytes, artwork.bytes, "{extension} content");
        }
    }

    #[test]
    fn synced_lyrics_are_embedded_and_a_sidecar_parses_back() {
        for extension in FORMATS {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = copy_fixture(directory.path(), extension);

            let mut spec = full_spec();
            spec.lyrics = synced_lyrics();
            let report = embed(&path, &spec).expect("embeds");

            assert_eq!(report.lyrics, EmbeddedLyrics::Synced, "{extension}");

            let sidecar = report
                .sidecar
                .unwrap_or_else(|| panic!("{extension} wrote no sidecar"));
            assert_eq!(sidecar, path.with_extension("lrc"));

            let text = fs::read_to_string(&sidecar).expect("sidecar readable");
            let parsed = lrc::parse(1, crate::model::LyricsSource::Sidecar, &text);
            assert_eq!(
                parsed.lines.len(),
                synced_lyrics().len(),
                "{extension} line count"
            );
            assert!(parsed.synced, "{extension} sidecar must be timed");
            assert_eq!(parsed.lines[1].time_ms, Some(4_500), "{extension} stamp");

            // The file itself carries the lyrics too, so a copy without the
            // sidecar still shows words.
            let facts = tags::read_facts(&path).expect("reads back");
            let embedded = facts
                .lyrics
                .unwrap_or_else(|| panic!("{extension} embedded no lyrics"));
            assert!(
                embedded.contains("second line"),
                "{extension} embedded text: {embedded:?}"
            );
        }
    }

    #[test]
    fn mp3_synced_lyrics_write_a_sylt_frame() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = copy_fixture(directory.path(), "mp3");

        let mut spec = full_spec();
        spec.lyrics = synced_lyrics();
        embed(&path, &spec).expect("embeds");

        let file = File::open(&path).expect("opens");
        let mut reader = BufReader::new(file);
        let mpeg = MpegFile::read_from(&mut reader, ParseOptions::default()).expect("parses");
        let id3 = mpeg.id3v2().expect("id3v2 present");

        let sylt = id3.get(&FrameId::Valid(Cow::Borrowed("SYLT")));
        let Some(Frame::Binary(frame)) = sylt else {
            panic!("no SYLT binary frame after embedding timed lyrics");
        };

        let parsed = SynchronizedTextFrame::parse(
            &frame.as_bytes(),
            lofty::id3::v2::FrameFlags::default(),
        )
        .expect("the frame we wrote parses back");
        assert_eq!(parsed.content.len(), 3);
        assert_eq!(parsed.content[0].0, 1_000, "millisecond timestamps");
        assert_eq!(parsed.content[2].1, "third line");
    }

    #[test]
    fn plain_lyrics_are_embedded_without_a_sidecar() {
        for extension in ["flac", "mp3"] {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = copy_fixture(directory.path(), extension);

            let mut spec = full_spec();
            spec.lyrics = plain_lyrics();
            let report = embed(&path, &spec).expect("embeds");

            assert_eq!(report.lyrics, EmbeddedLyrics::Plain, "{extension}");
            assert!(report.sidecar.is_none(), "{extension} wrote a sidecar");
            assert!(!path.with_extension("lrc").exists());

            let facts = tags::read_facts(&path).expect("reads back");
            let embedded = facts.lyrics.expect("lyrics present");
            assert!(embedded.contains("unmarked two"), "{embedded:?}");
        }
    }

    #[test]
    // The file is a private copy in a temp directory that this test created, so
    // clearing the read-only bit affects nobody else.
    #[allow(clippy::permissions_set_readonly_false)]
    fn writing_to_a_read_only_file_is_a_typed_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = copy_fixture(directory.path(), "flac");

        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).expect("marks read-only");

        let result = embed(&path, &full_spec());

        // Cleared before asserting so a failing assertion cannot leave the
        // temp directory behind with an undeletable file on Windows.
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).expect("restores permissions");

        match result {
            Err(CoreError::ReadOnly { path: reported }) => assert_eq!(reported, path),
            other => panic!("expected a typed read-only error, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_spec_leaves_the_file_readable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = copy_fixture(directory.path(), "ogg");

        let report = embed(&path, &EmbedSpec::new()).expect("embeds nothing");
        assert_eq!(report.lyrics, EmbeddedLyrics::None);
        assert_eq!(report.artwork_bytes, 0);
        assert!(report.sidecar.is_none());

        let facts = tags::read_facts(&path).expect("still readable");
        assert_eq!(facts.format, "VORBIS");
    }

    #[test]
    fn blank_lyric_lines_are_dropped_rather_than_written() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = copy_fixture(directory.path(), "flac");

        let mut spec = full_spec();
        spec.lyrics = vec![
            LyricLine {
                time_ms: Some(500),
                text: "   ".to_string(),
            },
            LyricLine {
                time_ms: None,
                text: String::new(),
            },
        ];

        let report = embed(&path, &spec).expect("embeds");
        assert_eq!(report.lyrics, EmbeddedLyrics::None);
        assert!(report.sidecar.is_none());
    }
}
