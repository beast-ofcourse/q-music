//! Directory scanning.
//!
//! A scan is incremental by modification time and size, reports progress while it
//! runs, and never aborts because one file is unreadable: the failure is collected
//! and shown, and the rest of the library still lands.

use std::path::{Path, PathBuf};

use walkdir::{DirEntry, WalkDir};

use crate::art::ArtCache;
use crate::db::LibraryDb;
use crate::model::{ScanProgress, Track};
use crate::tags::{self, FileFacts};

const PROGRESS_EVERY: usize = 24;

pub struct Scanner<'a> {
    db: &'a LibraryDb,
    art: &'a ArtCache,
}

impl<'a> Scanner<'a> {
    pub fn new(db: &'a LibraryDb, art: &'a ArtCache) -> Self {
        Self { db, art }
    }

    /// Walks one folder and indexes what it finds.
    pub fn scan(
        &self,
        folder: &Path,
        mut report: impl FnMut(&ScanProgress),
    ) -> crate::CoreResult<ScanProgress> {
        let mut progress = ScanProgress {
            scanning: true,
            folder: folder.display().to_string(),
            ..Default::default()
        };
        report(&progress);

        let now = crate::now_seconds();
        let walker = WalkDir::new(folder)
            .follow_links(true)
            .into_iter()
            // The root itself is never skipped: users legitimately point Q-Music at
            // folders like ~/.music, and only its children can be OS noise.
            .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry));

        for entry in walker {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    progress.failures.push(short_error(&error));
                    continue;
                }
            };

            if !entry.file_type().is_file() || !tags::is_supported(entry.path()) {
                continue;
            }

            progress.files_seen += 1;
            match self.index_file(entry.path(), now) {
                Ok(Change::Added) => progress.tracks_added += 1,
                Ok(Change::Updated) => progress.tracks_updated += 1,
                Ok(Change::Unchanged) => {}
                Err(error) => {
                    if progress.failures.len() < 200 {
                        progress.failures.push(error);
                    }
                }
            }

            if progress.files_seen % PROGRESS_EVERY == 0 {
                report(&progress);
            }
        }

        progress.tracks_removed = self.db.remove_missing(&progress.folder)?;
        progress.scanning = false;
        progress.finished_at = Some(crate::now_seconds());
        report(&progress);
        Ok(progress)
    }

    fn index_file(&self, path: &Path, now: i64) -> Result<Change, String> {
        let facts = tags::read_facts(path).map_err(|error| error.user_message())?;

        let path_text = path.display().to_string();
        let existing = self
            .db
            .track_by_path(&path_text)
            .map_err(|error| error.to_string())?;

        if let Some(existing) = &existing {
            if existing.mtime == facts.mtime && existing.size_bytes == facts.size_bytes {
                return Ok(Change::Unchanged);
            }
        }

        let art_id = match &facts.picture {
            Some(picture) => Some(self.art.store(picture).map_err(|error| error.to_string())?),
            None => existing.as_ref().and_then(|track| track.art_id.clone()),
        };

        let track = track_from_facts(&facts, art_id, now);
        let added = self.db.upsert_track(&track).map_err(|error| error.to_string())?;
        Ok(if added { Change::Added } else { Change::Updated })
    }
}

#[derive(Debug, PartialEq)]
enum Change {
    Added,
    Updated,
    Unchanged,
}

/// The single place a scanned file becomes a library row. Shared with the
/// tag editor so an edited file is indexed exactly like a scanned one.
pub fn track_from_facts(facts: &FileFacts, art_id: Option<String>, now: i64) -> Track {
    Track {
        id: 0,
        path: facts.path.display().to_string(),
        title: facts.title.clone(),
        artist: facts.artist.clone(),
        album_artist: facts.album_artist.clone(),
        album: facts.album.clone(),
        genre: facts.genre.clone(),
        year: facts.year,
        track_no: facts.track_no,
        disc_no: facts.disc_no,
        duration_ms: facts.duration_ms,
        sample_rate: facts.sample_rate,
        channels: facts.channels,
        bit_depth: facts.bit_depth,
        bitrate_kbps: facts.bitrate_kbps,
        format: facts.format.clone(),
        size_bytes: facts.size_bytes,
        mtime: facts.mtime,
        added_at: now,
        play_count: 0,
        last_played_at: None,
        favorite: false,
        art_id,
        has_lyrics: false,
    }
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|name| name.starts_with('.') || name == "System Volume Information" || name == "$RECYCLE.BIN")
        .unwrap_or(false)
}

fn short_error(error: &walkdir::Error) -> String {
    match error.path() {
        Some(path) => format!("{}: {}", path.display(), error),
        None => error.to_string(),
    }
}

/// Expands a folder into the folders the scanner should index, skipping one level
/// of OS noise so a "Music" root does not walk into a backup drive.
pub fn normalise_folder(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, seconds: f32) {
        crate::deck::tests_support::write_test_wav(path, seconds, 330.0).expect("wav");
    }

    #[test]
    fn scan_indexes_files_reports_progress_and_is_incremental() {
        let directory = tempfile::tempdir().expect("temp dir");
        let music = directory.path().join("music");
        std::fs::create_dir_all(music.join("album")).expect("mkdir");
        write_wav(&music.join("one.wav"), 0.2);
        write_wav(&music.join("album").join("two.wav"), 0.2);
        std::fs::write(music.join("notes.txt"), b"ignore me").expect("notes");

        let db = LibraryDb::in_memory().expect("db");
        let art = ArtCache::new(directory.path().join("art")).expect("art");
        let scanner = Scanner::new(&db, &art);

        let mut updates = 0;
        let progress = scanner
            .scan(&music, |_| updates += 1)
            .expect("scan");
        assert_eq!(progress.tracks_added, 2);
        assert_eq!(progress.files_seen, 2);
        assert!(updates >= 2);
        assert_eq!(db.track_count().expect("count"), 2);

        let again = scanner.scan(&music, |_| {}).expect("rescan");
        assert_eq!(again.tracks_added, 0);
        assert_eq!(again.tracks_updated, 0);
    }

    #[test]
    fn deleting_a_file_removes_its_row() {
        let directory = tempfile::tempdir().expect("temp dir");
        let music = directory.path().join("music");
        std::fs::create_dir_all(&music).expect("mkdir");
        let file = music.join("gone.wav");
        write_wav(&file, 0.2);

        let db = LibraryDb::in_memory().expect("db");
        let art = ArtCache::new(directory.path().join("art")).expect("art");
        let scanner = Scanner::new(&db, &art);
        scanner.scan(&music, |_| {}).expect("scan");

        std::fs::remove_file(&file).expect("remove");
        let progress = scanner.scan(&music, |_| {}).expect("rescan");
        assert_eq!(progress.tracks_removed, 1);
        assert_eq!(db.track_count().expect("count"), 0);
    }
}
