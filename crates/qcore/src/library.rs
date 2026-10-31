//! The library service: index, artwork and lyrics in one handle the app can share.

use std::path::{Path, PathBuf};

use crate::art::ArtCache;
use crate::db::LibraryDb;
use crate::error::CoreResult;
use crate::lrc;
use crate::model::{Lyrics, LyricsSource, ScanProgress, Track};
use crate::scan::Scanner;
use crate::tags;

pub struct Library {
    db: LibraryDb,
    art: ArtCache,
    root: PathBuf,
}

impl Library {
    /// Opens (or creates) a library rooted at `root`, which holds the index and the
    /// artwork cache.
    pub fn open(root: PathBuf) -> CoreResult<Self> {
        std::fs::create_dir_all(&root)?;
        let db = LibraryDb::open(&root.join("library.db"))?;
        let art = ArtCache::new(root.join("artwork"))?;
        Ok(Self { db, art, root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db(&self) -> &LibraryDb {
        &self.db
    }

    pub fn art(&self) -> &ArtCache {
        &self.art
    }

    pub fn scan(
        &self,
        folder: &Path,
        report: impl FnMut(&ScanProgress),
    ) -> CoreResult<ScanProgress> {
        let scanner = Scanner::new(&self.db, &self.art);
        let progress = scanner.scan(folder, report)?;
        self.db.set_setting("lastScanAt", &crate::now_seconds().to_string())?;
        Ok(progress)
    }

    /// Resolves lyrics from the most specific source available. A hand-edited sheet
    /// always wins over a sidecar file, which wins over the file's own tag.
    pub fn lyrics_for(&self, track_id: i64) -> CoreResult<Lyrics> {
        if let Some((source, content)) = self.db.stored_lyrics(track_id)? {
            if source == "manual" {
                return Ok(lrc::parse(track_id, LyricsSource::Manual, &content));
            }
        }

        let Some(track) = self.db.get_track(track_id)? else {
            return Ok(Lyrics {
                track_id,
                source: LyricsSource::None,
                lines: Vec::new(),
                synced: false,
                warning: Some("this track is no longer in the library".to_string()),
            });
        };

        if let Some((path, content)) = read_sidecar(Path::new(&track.path)) {
            let mut lyrics = lrc::parse(track_id, LyricsSource::Sidecar, &content);
            if let Some(warning) = lyrics.warning.take() {
                lyrics.warning = Some(format!("{}: {warning}", file_name(&path)));
            }
            return Ok(lyrics);
        }

        if let Ok(facts) = tags::read_facts(Path::new(&track.path)) {
            if let Some(content) = facts.lyrics {
                let mut lyrics = lrc::parse(track_id, LyricsSource::Embedded, &content);
                lyrics.warning = lyrics.warning.take();
                return Ok(lyrics);
            }
        }

        Ok(Lyrics {
            track_id,
            source: LyricsSource::None,
            lines: Vec::new(),
            synced: false,
            warning: None,
        })
    }

    pub fn save_lyrics(&self, track_id: i64, content: &str) -> CoreResult<Lyrics> {
        self.db.save_lyrics(track_id, "manual", content, crate::now_seconds())?;
        Ok(lrc::parse(track_id, LyricsSource::Manual, content))
    }

    /// Copies a `.lrc`/`.txt` file next to the audio file and stores the parse.
    pub fn import_lyrics(&self, track_id: i64, source: &Path) -> CoreResult<Lyrics> {
        let content = std::fs::read_to_string(source)
            .map_err(|error| crate::CoreError::io(source, error))?;
        let track = self
            .db
            .get_track(track_id)?
            .ok_or_else(|| crate::CoreError::Invalid("track is not in the library".to_string()))?;

        let audio = Path::new(&track.path);
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("lrc")
            .to_ascii_lowercase();
        let target = audio.with_extension(extension);
        if source != target {
            std::fs::write(&target, &content).map_err(|error| crate::CoreError::io(&target, error))?;
        }
        Ok(lrc::parse(track_id, LyricsSource::Sidecar, &content))
    }

    pub fn export_lyrics(&self, track_id: i64, destination: &Path) -> CoreResult<()> {
        let lyrics = self.lyrics_for(track_id)?;
        let document = lrc::to_lrc(&lyrics.lines);
        std::fs::write(destination, document)
            .map_err(|error| crate::CoreError::io(destination, error))
    }

    /// Loads a persisted setting and falls back to the supplied default.
    pub fn setting<T: serde::de::DeserializeOwned>(&self, key: &str, fallback: T) -> T {
        self.db
            .get_setting(key)
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or(fallback)
    }

    pub fn set_setting<T: serde::Serialize>(&self, key: &str, value: &T) -> CoreResult<()> {
        let encoded = serde_json::to_string(value)
            .map_err(|error| crate::CoreError::Invalid(error.to_string()))?;
        self.db.set_setting(key, &encoded)
    }

    /// Prunes artwork no track refers to. Run after a scan that removed files.
    pub fn prune_artwork(&self) -> CoreResult<usize> {
        let tracks = self.db.list_tracks(&crate::db::TrackQuery { limit: 100_000, ..Default::default() })?;
        let referenced: Vec<String> = tracks.into_iter().filter_map(|track| track.art_id).collect();
        Ok(self.art.prune(&referenced))
    }

    pub fn track(&self, track_id: i64) -> CoreResult<Option<Track>> {
        self.db.get_track(track_id)
    }

    /// Re-reads one file after its tags were edited and refreshes its row, without
    /// rescanning the whole folder.
    pub fn reindex_file(&self, path: &Path) -> CoreResult<Track> {
        let facts = tags::read_facts(path)?;
        let art_id = match &facts.picture {
            Some(picture) => Some(self.art.store(picture)?),
            None => self
                .db
                .track_by_path(&path.display().to_string())?
                .and_then(|track| track.art_id),
        };

        let existing = self.db.track_by_path(&path.display().to_string())?;
        let mut track = crate::scan::track_from_facts(&facts, art_id, crate::now_seconds());
        if let Some(existing) = existing {
            track.added_at = existing.added_at;
            track.play_count = existing.play_count;
            track.last_played_at = existing.last_played_at;
            track.favorite = existing.favorite;
        }

        self.db.upsert_track(&track)?;
        self.db
            .track_by_path(&path.display().to_string())?
            .ok_or_else(|| crate::CoreError::Invalid("track vanished after reindex".to_string()))
    }
}

fn read_sidecar(audio: &Path) -> Option<(PathBuf, String)> {
    for extension in ["lrc", "txt"] {
        let candidate = audio.with_extension(extension);
        if candidate.is_file() {
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                return Some((candidate, content));
            }
        }
    }
    None
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "lyrics".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_lyrics_win_over_sidecar() {
        let directory = tempfile::tempdir().expect("temp dir");
        let library = Library::open(directory.path().to_path_buf()).expect("library");

        let audio = directory.path().join("song.wav");
        crate::deck::tests_support::write_test_wav(&audio, 0.2, 300.0).expect("wav");
        std::fs::write(audio.with_extension("lrc"), "[00:01.00]from the sidecar\n").expect("lrc");

        library
            .scan(directory.path(), |_| {})
            .expect("scan");
        let track = library.db().list_tracks(&Default::default()).expect("list")[0].clone();

        let sidecar = library.lyrics_for(track.id).expect("lyrics");
        assert_eq!(sidecar.source, LyricsSource::Sidecar);
        assert_eq!(sidecar.lines[0].text, "from the sidecar");

        library.save_lyrics(track.id, "[00:02.00]typed by hand\n").expect("save");
        let manual = library.lyrics_for(track.id).expect("lyrics");
        assert_eq!(manual.source, LyricsSource::Manual);
        assert_eq!(manual.lines[0].text, "typed by hand");
    }

    #[test]
    fn importing_a_lyric_file_writes_a_sidecar_next_to_the_audio() {
        let directory = tempfile::tempdir().expect("temp dir");
        let library = Library::open(directory.path().to_path_buf()).expect("library");
        let audio = directory.path().join("song.wav");
        crate::deck::tests_support::write_test_wav(&audio, 0.2, 300.0).expect("wav");
        library.scan(directory.path(), |_| {}).expect("scan");
        let track = library.db().list_tracks(&Default::default()).expect("list")[0].clone();

        let imported = directory.path().join("incoming.lrc");
        std::fs::write(&imported, "[00:03.00]imported line\n").expect("write");
        let lyrics = library.import_lyrics(track.id, &imported).expect("import");
        assert_eq!(lyrics.lines[0].text, "imported line");
        assert!(audio.with_extension("lrc").is_file());
    }

    #[test]
    fn settings_round_trip_with_types() {
        let directory = tempfile::tempdir().expect("temp dir");
        let library = Library::open(directory.path().to_path_buf()).expect("library");
        library.set_setting("volume", &0.62f32).expect("set");
        let volume: f32 = library.setting("volume", 1.0);
        assert!((volume - 0.62).abs() < f32::EPSILON);
        let missing: u32 = library.setting("nothing-here", 7);
        assert_eq!(missing, 7);
    }
}
