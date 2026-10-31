//! The library index: one SQLite file the user owns, migrated in place.
//!
//! Queries are built from typed inputs and bound as parameters; nothing user-typed
//! reaches the SQL string. Sorting is a closed enum, so a column name can never be
//! injected either.

use std::path::Path;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, ToSql};

use crate::error::CoreResult;
use crate::model::{
    LibraryFolder, LibraryStats, Playlist, SmartField, SmartOp, SmartRule, SmartRules, SortField,
    SortOrder, Track, TrackGroup,
};

const SCHEMA_VERSION: i64 = 1;

/// Distance used to vacate occupied playlist positions before a reorder. Any value
/// larger than a real playlist's length works; this one leaves room for 100k rows.
const POSITION_PARK_OFFSET: i64 = 1_000_000;

pub struct LibraryDb {
    connection: Connection,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackQuery {
    pub search: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub format: Option<String>,
    pub favorites_only: bool,
    pub sort: SortField,
    pub order: SortOrder,
    pub limit: u32,
    pub offset: u32,
}

impl LibraryDb {
    /// Opens (or creates) the index file and applies migrations.
    pub fn open(path: &Path) -> CoreResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        Self::prepare(connection)
    }

    /// In-memory index, used by tests and by the first run before a data directory exists.
    pub fn in_memory() -> CoreResult<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> CoreResult<Self> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { connection };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> CoreResult<()> {
        let current: i64 =
            self.connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if current >= SCHEMA_VERSION {
            return Ok(());
        }

        self.connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tracks (
                id             INTEGER PRIMARY KEY,
                path           TEXT NOT NULL UNIQUE,
                title          TEXT NOT NULL,
                artist         TEXT NOT NULL DEFAULT '',
                album_artist   TEXT NOT NULL DEFAULT '',
                album          TEXT NOT NULL DEFAULT '',
                genre          TEXT NOT NULL DEFAULT '',
                year           INTEGER,
                track_no       INTEGER,
                disc_no        INTEGER,
                duration_ms    INTEGER NOT NULL DEFAULT 0,
                sample_rate    INTEGER,
                channels       INTEGER,
                bit_depth      INTEGER,
                bitrate_kbps   INTEGER,
                format         TEXT NOT NULL DEFAULT '',
                size_bytes     INTEGER NOT NULL DEFAULT 0,
                mtime          INTEGER NOT NULL DEFAULT 0,
                added_at       INTEGER NOT NULL,
                play_count     INTEGER NOT NULL DEFAULT 0,
                last_played_at INTEGER,
                favorite       INTEGER NOT NULL DEFAULT 0,
                art_id         TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_tracks_artist ON tracks(artist);
            CREATE INDEX IF NOT EXISTS ix_tracks_album  ON tracks(album);
            CREATE INDEX IF NOT EXISTS ix_tracks_title  ON tracks(title);

            CREATE TABLE IF NOT EXISTS folders (
                id       INTEGER PRIMARY KEY,
                path     TEXT NOT NULL UNIQUE,
                added_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS playlists (
                id         INTEGER PRIMARY KEY,
                name       TEXT NOT NULL,
                kind       TEXT NOT NULL DEFAULT 'manual',
                rules_json TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS playlist_items (
                playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
                track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
                position    INTEGER NOT NULL,
                PRIMARY KEY (playlist_id, position)
            );

            CREATE TABLE IF NOT EXISTS lyrics (
                track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
                source     TEXT NOT NULL,
                content    TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS recent_plays (
                id        INTEGER PRIMARY KEY,
                track_id  INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
                played_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS ix_recent_played ON recent_plays(played_at DESC);
            "#,
        )?;
        self.connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    pub fn track_count(&self) -> CoreResult<usize> {
        let count: i64 = self.connection.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Inserts or refreshes a row. Returns true when the row is new.
    pub fn upsert_track(&self, track: &Track) -> CoreResult<bool> {
        let existed: bool = self
            .connection
            .query_row("SELECT 1 FROM tracks WHERE path = ?1", params![track.path], |_| Ok(true))
            .optional()?
            .unwrap_or(false);

        self.connection.execute(
            r#"
            INSERT INTO tracks (
                path, title, artist, album_artist, album, genre, year, track_no, disc_no,
                duration_ms, sample_rate, channels, bit_depth, bitrate_kbps, format,
                size_bytes, mtime, added_at, play_count, last_played_at, favorite, art_id
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,0,NULL,0,?19)
            ON CONFLICT(path) DO UPDATE SET
                title = excluded.title,
                artist = excluded.artist,
                album_artist = excluded.album_artist,
                album = excluded.album,
                genre = excluded.genre,
                year = excluded.year,
                track_no = excluded.track_no,
                disc_no = excluded.disc_no,
                duration_ms = excluded.duration_ms,
                sample_rate = excluded.sample_rate,
                channels = excluded.channels,
                bit_depth = excluded.bit_depth,
                bitrate_kbps = excluded.bitrate_kbps,
                format = excluded.format,
                size_bytes = excluded.size_bytes,
                mtime = excluded.mtime,
                art_id = COALESCE(excluded.art_id, tracks.art_id)
            "#,
            params![
                track.path,
                track.title,
                track.artist,
                track.album_artist,
                track.album,
                track.genre,
                track.year,
                track.track_no,
                track.disc_no,
                track.duration_ms as i64,
                track.sample_rate,
                track.channels,
                track.bit_depth,
                track.bitrate_kbps,
                track.format,
                track.size_bytes as i64,
                track.mtime,
                track.added_at,
                track.art_id,
            ],
        )?;
        Ok(!existed)
    }

    pub fn get_track(&self, id: i64) -> CoreResult<Option<Track>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {TRACK_COLUMNS}, (l.track_id IS NOT NULL) AS has_lyrics \
             FROM tracks t LEFT JOIN lyrics l ON l.track_id = t.id WHERE t.id = ?1"
        ))?;
        let track = statement.query_row(params![id], row_to_track).optional()?;
        Ok(track)
    }

    pub fn track_by_path(&self, path: &str) -> CoreResult<Option<Track>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {TRACK_COLUMNS}, (l.track_id IS NOT NULL) AS has_lyrics \
             FROM tracks t LEFT JOIN lyrics l ON l.track_id = t.id WHERE t.path = ?1"
        ))?;
        let track = statement.query_row(params![path], row_to_track).optional()?;
        Ok(track)
    }

    pub fn list_tracks(&self, query: &TrackQuery) -> CoreResult<Vec<Track>> {
        let (sql, values) = build_track_query(query, false)?;
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), row_to_track)?;
        let mut tracks = Vec::new();
        for row in rows {
            tracks.push(row?);
        }
        Ok(tracks)
    }

    pub fn count_tracks(&self, query: &TrackQuery) -> CoreResult<usize> {
        let (sql, values) = build_track_query(query, true)?;
        let mut statement = self.connection.prepare(&sql)?;
        let count: i64 = statement.query_row(params_from_iter(values.iter()), |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn distinct(&self, column: &str) -> CoreResult<Vec<String>> {        let column = match column {
            "artist" | "album" | "genre" | "format" | "album_artist" => column,
            other => return Err(crate::error::CoreError::Invalid(format!("unknown column {other}"))),
        };
        let sql = format!(
            "SELECT DISTINCT {column} FROM tracks WHERE {column} <> '' ORDER BY {column} COLLATE NOCASE"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut values = Vec::new();
        for row in rows {
            values.push(row?);
        }
        Ok(values)
    }

    /// One row per album or artist, with the totals the browse lists show. Grouping in
    /// SQL keeps the window from pulling every track just to count them. The artist
    /// expression mirrors `Track::display_artist`: a file with no album-artist tag
    /// belongs to its artist, not to a nameless group.
    pub fn groups(&self, column: &str) -> CoreResult<Vec<TrackGroup>> {
        const PERFORMER: &str = "COALESCE(NULLIF(album_artist, ''), artist)";

        let (key, subtitle) = match column {
            "album" => ("album", PERFORMER),
            "artist" => (PERFORMER, PERFORMER),
            other => {
                return Err(crate::error::CoreError::Invalid(format!(
                    "cannot group by {other}"
                )))
            }
        };

        let sql = format!(
            "SELECT {key} AS name, MIN({subtitle}) AS subtitle, COUNT(*) AS track_count, \
             SUM(duration_ms) AS duration_ms, MIN(art_id) AS art_id \
             FROM tracks WHERE {key} <> '' GROUP BY {key} ORDER BY {key} COLLATE NOCASE"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map([], |row| {
            Ok(TrackGroup {
                name: row.get(0)?,
                subtitle: if column == "album" { row.get(1)? } else { String::new() },
                track_count: row.get::<_, i64>(2)? as usize,
                duration_ms: row.get::<_, i64>(3)? as u64,
                art_id: row.get(4)?,
            })
        })?;
        collect(rows)
    }

    pub fn set_favorite(&self, track_id: i64, favorite: bool) -> CoreResult<()> {
        self.connection.execute(
            "UPDATE tracks SET favorite = ?2 WHERE id = ?1",
            params![track_id, favorite],
        )?;
        Ok(())
    }

    pub fn record_play(&self, track_id: i64, played_at: i64) -> CoreResult<()> {
        self.connection.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played_at = ?2 WHERE id = ?1",
            params![track_id, played_at],
        )?;
        self.connection.execute(
            "INSERT INTO recent_plays (track_id, played_at) VALUES (?1, ?2)",
            params![track_id, played_at],
        )?;
        self.connection.execute(
            "DELETE FROM recent_plays WHERE id NOT IN (SELECT id FROM recent_plays ORDER BY played_at DESC LIMIT 500)",
            [],
        )?;
        Ok(())
    }

    pub fn recent_tracks(&self, limit: u32) -> CoreResult<Vec<Track>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {TRACK_COLUMNS}, 0 AS has_lyrics FROM tracks t \
             WHERE t.last_played_at IS NOT NULL \
             ORDER BY t.last_played_at DESC LIMIT ?1"
        ))?;
        let rows = statement.query_map(params![limit], row_to_track)?;
        collect(rows)
    }

    pub fn recently_added(&self, limit: u32) -> CoreResult<Vec<Track>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {TRACK_COLUMNS}, 0 AS has_lyrics FROM tracks t ORDER BY t.added_at DESC LIMIT ?1"
        ))?;
        let rows = statement.query_map(params![limit], row_to_track)?;
        collect(rows)
    }

    /// Files that are indexed under a folder but no longer exist on disk.
    pub fn remove_missing(&self, root: &str) -> CoreResult<usize> {
        let mut statement = self
            .connection
            .prepare("SELECT id, path FROM tracks WHERE path LIKE ?1")?;
        let rows = statement.query_map(params![format!("{root}%")], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut stale = Vec::new();
        for row in rows {
            let (id, path) = row?;
            if !Path::new(&path).exists() {
                stale.push(id);
            }
        }

        for id in &stale {
            self.connection.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
        }
        Ok(stale.len())
    }

    // ---- folders -------------------------------------------------------

    pub fn add_folder(&self, path: &str, added_at: i64) -> CoreResult<i64> {
        self.connection.execute(
            "INSERT INTO folders (path, added_at) VALUES (?1, ?2) ON CONFLICT(path) DO NOTHING",
            params![path, added_at],
        )?;
        let id = self.connection.query_row(
            "SELECT id FROM folders WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn remove_folder(&self, id: i64) -> CoreResult<Option<String>> {
        let path: Option<String> = self
            .connection
            .query_row("SELECT path FROM folders WHERE id = ?1", params![id], |row| row.get(0))
            .optional()?;
        if let Some(path) = &path {
            self.connection.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
            self.connection.execute(
                "DELETE FROM tracks WHERE path LIKE ?1",
                params![format!("{path}%")],
            )?;
        }
        Ok(path)
    }

    pub fn folders(&self) -> CoreResult<Vec<LibraryFolder>> {
        let mut statement = self.connection.prepare(
            "SELECT f.id, f.path, f.added_at, \
             (SELECT COUNT(*) FROM tracks t WHERE t.path LIKE f.path || '%') \
             FROM folders f ORDER BY f.path COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(LibraryFolder {
                id: row.get(0)?,
                path: row.get(1)?,
                added_at: row.get(2)?,
                track_count: row.get::<_, i64>(3)? as usize,
            })
        })?;
        collect(rows)
    }

    pub fn stats(&self) -> CoreResult<LibraryStats> {
        let (tracks, duration, bytes): (i64, i64, i64) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(duration_ms),0), COALESCE(SUM(size_bytes),0) FROM tracks",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let (albums, artists): (i64, i64) = self.connection.query_row(
            "SELECT COUNT(DISTINCT album), COUNT(DISTINCT album_artist) FROM tracks \
             WHERE album <> '' AND album_artist <> ''",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let last_scan_at: Option<i64> = self
            .connection
            .query_row("SELECT value FROM settings WHERE key = 'lastScanAt'", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .and_then(|value| value.parse().ok());

        Ok(LibraryStats {
            tracks: tracks as usize,
            albums: albums as usize,
            artists: artists as usize,
            total_duration_ms: duration as u64,
            total_bytes: bytes as u64,
            last_scan_at,
            scanning: false,
        })
    }

    // ---- playlists -----------------------------------------------------

    pub fn create_playlist(
        &self,
        name: &str,
        kind: &str,
        rules: Option<&SmartRules>,
        now: i64,
    ) -> CoreResult<i64> {
        let rules_json = rules
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| crate::error::CoreError::Invalid(error.to_string()))?;
        self.connection.execute(
            "INSERT INTO playlists (name, kind, rules_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![name, kind, rules_json, now],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn rename_playlist(&self, id: i64, name: &str, now: i64) -> CoreResult<()> {
        self.connection.execute(
            "UPDATE playlists SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, now],
        )?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: i64) -> CoreResult<()> {
        self.connection.execute("DELETE FROM playlists WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_smart_rules(&self, id: i64, rules: &SmartRules, now: i64) -> CoreResult<()> {
        let json = serde_json::to_string(rules)
            .map_err(|error| crate::error::CoreError::Invalid(error.to_string()))?;
        self.connection.execute(
            "UPDATE playlists SET rules_json = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, json, now],
        )?;
        Ok(())
    }

    pub fn playlists(&self) -> CoreResult<Vec<Playlist>> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.name, p.kind, p.rules_json, p.created_at, p.updated_at, \
             (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id) \
             FROM playlists p ORDER BY p.created_at ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let rules_json: Option<String> = row.get(3)?;
            Ok(Playlist {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                rules: rules_json.and_then(|json| serde_json::from_str(&json).ok()),
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                track_count: row.get::<_, i64>(6)? as usize,
            })
        })?;
        collect(rows)
    }

    pub fn playlist_tracks(&self, playlist_id: i64) -> CoreResult<Vec<Track>> {
        let rules: Option<String> = self
            .connection
            .query_row(
                "SELECT rules_json FROM playlists WHERE id = ?1",
                params![playlist_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        if let Some(rules_json) = rules {
            if let Ok(rules) = serde_json::from_str::<SmartRules>(&rules_json) {
                return self.smart_tracks(&rules);
            }
        }

        let mut statement = self.connection.prepare(&format!(
            "SELECT {TRACK_COLUMNS}, (l.track_id IS NOT NULL) AS has_lyrics \
             FROM playlist_items i \
             JOIN tracks t ON t.id = i.track_id \
             LEFT JOIN lyrics l ON l.track_id = t.id \
             WHERE i.playlist_id = ?1 ORDER BY i.position ASC"
        ))?;
        let rows = statement.query_map(params![playlist_id], row_to_track)?;
        collect(rows)
    }

    /// Resolves a smart playlist's rules into rows. Unknown or mistyped rule values
    /// are ignored rather than failing the whole view.
    pub fn smart_tracks(&self, rules: &SmartRules) -> CoreResult<Vec<Track>> {
        if rules.rules.is_empty() {
            return Ok(Vec::new());
        }

        let now = crate::now_seconds();
        let mut clauses = Vec::new();
        let mut values: Vec<Box<dyn ToSql>> = Vec::new();

        for rule in &rules.rules {
            let clause = smart_clause(rule, now, &mut values);
            if let Some(clause) = clause {
                clauses.push(clause);
            }
        }

        if clauses.is_empty() {
            return Ok(Vec::new());
        }

        let joiner = if rules.match_any { " OR " } else { " AND " };
        let sql = format!(
            "SELECT {TRACK_COLUMNS}, 0 AS has_lyrics FROM tracks t WHERE ({}) \
             ORDER BY t.artist COLLATE NOCASE, t.album COLLATE NOCASE, t.track_no ASC LIMIT ?",
            clauses.join(joiner)
        );
        values.push(Box::new(rules.limit.clamp(1, 2000) as i64));

        let mut statement = self.connection.prepare(&sql)?;
        let borrowed: Vec<&dyn ToSql> = values.iter().map(|value| value.as_ref()).collect();
        let rows = statement.query_map(params_from_iter(borrowed), row_to_track)?;
        collect(rows)
    }

    pub fn add_to_playlist(&self, playlist_id: i64, track_ids: &[i64]) -> CoreResult<()> {
        let next: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )?;
        for (offset, track_id) in track_ids.iter().enumerate() {
            self.connection.execute(
                "INSERT OR REPLACE INTO playlist_items (playlist_id, position, track_id) \
                 VALUES (?1, ?2, ?3)",
                params![playlist_id, next + offset as i64, track_id],
            )?;
        }
        Ok(())
    }

    pub fn remove_from_playlist(&self, playlist_id: i64, position: i64) -> CoreResult<()> {
        self.connection.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
            params![playlist_id, position],
        )?;
        self.reindex_playlist(playlist_id)?;
        Ok(())
    }

    pub fn move_playlist_item(&self, playlist_id: i64, from: i64, to: i64) -> CoreResult<()> {
        let mut rowids = self.playlist_rowids(playlist_id)?;
        let Some(index) = usize::try_from(from).ok().filter(|index| *index < rowids.len()) else {
            return Ok(());
        };

        let rowid = rowids.remove(index);
        let target = usize::try_from(to).unwrap_or(0).min(rowids.len());
        rowids.insert(target, rowid);
        self.write_playlist_positions(playlist_id, &rowids)
    }

    fn reindex_playlist(&self, playlist_id: i64) -> CoreResult<()> {
        let rowids = self.playlist_rowids(playlist_id)?;
        self.write_playlist_positions(playlist_id, &rowids)
    }

    fn playlist_rowids(&self, playlist_id: i64) -> CoreResult<Vec<i64>> {
        let mut statement = self.connection.prepare(
            "SELECT rowid FROM playlist_items WHERE playlist_id = ?1 ORDER BY position ASC",
        )?;
        let rows = statement.query_map(params![playlist_id], |row| row.get::<_, i64>(0))?;
        collect(rows)
    }

    /// Writes `0..n` positions for the given rows. Positions are parked out of the
    /// way first because `(playlist_id, position)` is unique: a plain shift would
    /// reject the row that is about to land on a position another row still holds.
    fn write_playlist_positions(&self, playlist_id: i64, rowids: &[i64]) -> CoreResult<()> {
        self.connection.execute(
            "UPDATE playlist_items SET position = position + ?2 WHERE playlist_id = ?1",
            params![playlist_id, POSITION_PARK_OFFSET],
        )?;

        for (position, rowid) in rowids.iter().enumerate() {
            self.connection.execute(
                "UPDATE playlist_items SET position = ?2 WHERE rowid = ?1",
                params![rowid, position as i64],
            )?;
        }
        Ok(())
    }

    // ---- lyrics --------------------------------------------------------

    pub fn save_lyrics(&self, track_id: i64, source: &str, content: &str, now: i64) -> CoreResult<()> {
        self.connection.execute(
            "INSERT INTO lyrics (track_id, source, content, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(track_id) DO UPDATE SET source = excluded.source, \
             content = excluded.content, updated_at = excluded.updated_at",
            params![track_id, source, content, now],
        )?;
        Ok(())
    }

    pub fn stored_lyrics(&self, track_id: i64) -> CoreResult<Option<(String, String)>> {
        let row = self
            .connection
            .query_row(
                "SELECT source, content FROM lyrics WHERE track_id = ?1",
                params![track_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    // ---- settings ------------------------------------------------------

    pub fn set_setting(&self, key: &str, value: &str) -> CoreResult<()> {
        self.connection.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self
            .connection
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0))
            .optional()?)
    }
}

const TRACK_COLUMNS: &str = "t.id, t.path, t.title, t.artist, t.album_artist, t.album, t.genre, \
     t.year, t.track_no, t.disc_no, t.duration_ms, t.sample_rate, t.channels, t.bit_depth, \
     t.bitrate_kbps, t.format, t.size_bytes, t.added_at, t.play_count, t.last_played_at, \
     t.favorite, t.art_id, t.mtime";

fn row_to_track(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album_artist: row.get(4)?,
        album: row.get(5)?,
        genre: row.get(6)?,
        year: row.get(7)?,
        track_no: row.get(8)?,
        disc_no: row.get(9)?,
        duration_ms: row.get::<_, i64>(10)?.max(0) as u64,
        sample_rate: row.get(11)?,
        channels: row.get(12)?,
        bit_depth: row.get(13)?,
        bitrate_kbps: row.get(14)?,
        format: row.get(15)?,
        size_bytes: row.get::<_, i64>(16)?.max(0) as u64,
        added_at: row.get(17)?,
        play_count: row.get(18)?,
        last_played_at: row.get(19)?,
        favorite: row.get::<_, i64>(20)? != 0,
        art_id: row.get(21)?,
        mtime: row.get(22)?,
        has_lyrics: row.get::<_, i64>(23).unwrap_or(0) != 0,
    })
}

fn collect<T>(rows: rusqlite::MappedRows<'_, impl FnMut(&Row<'_>) -> rusqlite::Result<T>>) -> CoreResult<Vec<T>> {
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

fn build_track_query(query: &TrackQuery, counting: bool) -> CoreResult<(String, Vec<Value>)> {
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();

    if let Some(search) = query.search.as_ref().filter(|text| !text.trim().is_empty()) {
        clauses.push(
            "(t.title LIKE ?1 OR t.artist LIKE ?1 OR t.album LIKE ?1 OR t.album_artist LIKE ?1 \
             OR t.genre LIKE ?1 OR t.path LIKE ?1)"
                .to_string(),
        );
        values.push(Value::Text(format!("%{}%", search.trim())));
    }

    push_equality(&mut clauses, &mut values, "t.artist", query.artist.as_ref());
    push_equality(&mut clauses, &mut values, "t.album", query.album.as_ref());
    push_equality(&mut clauses, &mut values, "t.genre", query.genre.as_ref());
    push_equality(&mut clauses, &mut values, "t.format", query.format.as_ref());

    if query.favorites_only {
        clauses.push("t.favorite = 1".to_string());
    }

    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };

    if counting {
        return Ok((format!("SELECT COUNT(*) FROM tracks t{where_clause}"), values));
    }

    let order = match query.order {
        SortOrder::Asc => "ASC",
        SortOrder::Desc => "DESC",
    };
    let order_clause = match query.sort {
        SortField::Title => format!("t.title COLLATE NOCASE {order}"),
        SortField::Artist => {
            format!("t.album_artist COLLATE NOCASE {order}, t.album COLLATE NOCASE {order}, t.disc_no {order}, t.track_no {order}")
        }
        SortField::Album => {
            format!("t.album COLLATE NOCASE {order}, t.disc_no {order}, t.track_no {order}")
        }
        SortField::Added => format!("t.added_at {order}"),
        SortField::Duration => format!("t.duration_ms {order}"),
        SortField::Year => format!("t.year {order}, t.track_no ASC"),
        SortField::PlayCount => format!("t.play_count {order}, t.title COLLATE NOCASE ASC"),
        SortField::Format => format!("t.format {order}, t.title COLLATE NOCASE ASC"),
    };

    let limit = if query.limit == 0 { 5_000 } else { query.limit.min(100_000) };
    let sql = format!(
        "SELECT {TRACK_COLUMNS}, (l.track_id IS NOT NULL) AS has_lyrics FROM tracks t \
         LEFT JOIN lyrics l ON l.track_id = t.id{where_clause} \
         ORDER BY {order_clause} LIMIT {limit} OFFSET {}",
        query.offset
    );
    Ok((sql, values))
}

fn push_equality(
    clauses: &mut Vec<String>,
    values: &mut Vec<Value>,
    column: &str,
    filter: Option<&String>,
) {
    if let Some(value) = filter.filter(|text| !text.trim().is_empty()) {
        clauses.push(format!("{column} = ?"));
        values.push(Value::Text(value.clone()));
    }
}

/// Translates one rule into a SQL fragment. Invalid values yield `None`, which
/// silently narrows the playlist instead of breaking it.
fn smart_clause(
    rule: &SmartRule,
    now: i64,
    values: &mut Vec<Box<dyn ToSql>>,
) -> Option<String> {
    let text = rule.value.trim();
    let numeric = text.parse::<i64>().ok();

    let mut comparison = |column: &str, operator: &str, value: Box<dyn ToSql>| {
        values.push(value);
        Some(format!("t.{column} {operator} ?"))
    };

    match (rule.field, rule.op) {
        (SmartField::Favorite, _) => {
            let wanted = matches!(text.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
            Some(if wanted { "t.favorite = 1".to_string() } else { "t.favorite = 0".to_string() })
        }
        (SmartField::Artist, SmartOp::Is) => comparison("artist", "=", Box::new(text.to_string())),
        (SmartField::Artist, SmartOp::Contains) => {
            comparison("artist", "LIKE", Box::new(format!("%{text}%")))
        }
        (SmartField::Artist, SmartOp::IsNot) => comparison("artist", "<>", Box::new(text.to_string())),
        (SmartField::Album, SmartOp::Is) => comparison("album", "=", Box::new(text.to_string())),
        (SmartField::Album, SmartOp::Contains) => {
            comparison("album", "LIKE", Box::new(format!("%{text}%")))
        }
        (SmartField::Album, SmartOp::IsNot) => comparison("album", "<>", Box::new(text.to_string())),
        (SmartField::Genre, SmartOp::Is) => comparison("genre", "=", Box::new(text.to_string())),
        (SmartField::Genre, SmartOp::Contains) => {
            comparison("genre", "LIKE", Box::new(format!("%{text}%")))
        }
        (SmartField::Genre, SmartOp::IsNot) => comparison("genre", "<>", Box::new(text.to_string())),
        (SmartField::Title, SmartOp::Contains) => {
            comparison("title", "LIKE", Box::new(format!("%{text}%")))
        }
        (SmartField::Format, SmartOp::Is) => comparison("format", "=", Box::new(text.to_string())),
        (SmartField::Format, SmartOp::IsNot) => comparison("format", "<>", Box::new(text.to_string())),
        (SmartField::Year, op) => {
            let value = numeric?;
            comparison("year", operator_for(op)?, Box::new(value))
        }
        (SmartField::PlayCount, op) => {
            let value = numeric?;
            comparison("play_count", operator_for(op)?, Box::new(value))
        }
        (SmartField::DurationSeconds, op) => {
            let value = numeric? * 1000;
            comparison("duration_ms", operator_for(op)?, Box::new(value))
        }
        (SmartField::AddedDays, SmartOp::InLast) => {
            let days = numeric?;
            values.push(Box::new(now - days * 86_400));
            Some("t.added_at >= ?".to_string())
        }
        (SmartField::AddedDays, op) => {
            let value = numeric? * 86_400;
            comparison("added_at", operator_for(op)?, Box::new(value))
        }
        _ => None,
    }
}

fn operator_for(op: SmartOp) -> Option<&'static str> {
    match op {
        SmartOp::GreaterThan => Some(">"),
        SmartOp::LessThan => Some("<"),
        SmartOp::Is => Some("="),
        SmartOp::InLast => Some(">="),
        SmartOp::IsNot => Some("<>"),
        SmartOp::Contains => Some("LIKE"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track(id_path: &str, title: &str, artist: &str, album: &str) -> Track {
        Track {
            id: 0,
            path: id_path.to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            album_artist: artist.to_string(),
            album: album.to_string(),
            genre: "Ambient".to_string(),
            year: Some(2021),
            track_no: Some(1),
            disc_no: Some(1),
            duration_ms: 240_000,
            sample_rate: Some(44_100),
            channels: Some(2),
            bit_depth: Some(24),
            bitrate_kbps: Some(900),
            format: "FLAC".to_string(),
            size_bytes: 1_000,
            mtime: 1_700_000_000,
            added_at: 1_700_000_000,
            play_count: 0,
            last_played_at: None,
            favorite: false,
            art_id: None,
            has_lyrics: false,
        }
    }

    fn seeded() -> LibraryDb {
        let db = LibraryDb::in_memory().expect("in-memory library");
        db.upsert_track(&sample_track("a/one.flac", "Night Ferry", "Kaito Mori", "Harbour"))
            .expect("insert");
        db.upsert_track(&sample_track("a/two.flac", "Copper Rain", "Kaito Mori", "Harbour"))
            .expect("insert");
        db.upsert_track(&sample_track("b/three.mp3", "Low Ceiling", "The Fathoms", "Rooms"))
            .expect("insert");
        db
    }

    #[test]
    fn upsert_is_idempotent_and_reports_new_rows() {
        let db = seeded();
        assert_eq!(db.track_count().expect("count"), 3);
        let added = db
            .upsert_track(&sample_track("a/one.flac", "Night Ferry", "Kaito Mori", "Harbour"))
            .expect("upsert again");
        assert!(!added);
        assert_eq!(db.track_count().expect("count"), 3);
    }

    #[test]
    fn search_spans_title_artist_and_album() {
        let db = seeded();
        let query = TrackQuery { search: Some("harbour".into()), ..Default::default() };
        assert_eq!(db.list_tracks(&query).expect("search").len(), 2);

        let query = TrackQuery { search: Some("fathoms".into()), ..Default::default() };
        assert_eq!(db.list_tracks(&query).expect("search").len(), 1);

        let query = TrackQuery { search: Some("zzz".into()), ..Default::default() };
        assert_eq!(db.count_tracks(&query).expect("count"), 0);
    }

    #[test]
    fn artist_sort_groups_albums_and_track_numbers() {
        let db = seeded();
        let tracks = db
            .list_tracks(&TrackQuery { sort: SortField::Artist, ..Default::default() })
            .expect("list");
        assert_eq!(tracks[0].artist, "Kaito Mori");
        assert_eq!(tracks[2].artist, "The Fathoms");
    }

    #[test]
    fn groups_report_totals_per_album_and_artist() {
        let db = seeded();
        let albums = db.groups("album").expect("albums");
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].name, "Harbour");
        assert_eq!(albums[0].track_count, 2);
        assert_eq!(albums[0].subtitle, "Kaito Mori", "an album row names its artist");
        assert_eq!(albums[0].duration_ms, 480_000);

        let artists = db.groups("artist").expect("artists");
        assert_eq!(artists.len(), 2);
        assert!(artists.iter().all(|group| group.subtitle.is_empty()));
        assert_eq!(artists.iter().map(|group| group.track_count).sum::<usize>(), 3);
    }

    #[test]
    fn artist_groups_fall_back_to_the_track_artist() {
        let db = seeded();
        let mut untagged = sample_track("c/four.flac", "No Album Artist", "Nell Voss", "Singles");
        untagged.album_artist = String::new();
        db.upsert_track(&untagged).expect("insert");

        let artists = db.groups("artist").expect("artists");
        assert!(
            artists.iter().any(|group| group.name == "Nell Voss"),
            "a file with no album-artist tag must not disappear from the artist list"
        );

        let albums = db.groups("album").expect("albums");
        let singles = albums.iter().find(|group| group.name == "Singles").expect("album");
        assert_eq!(singles.subtitle, "Nell Voss");
    }

    #[test]
    fn favorites_round_trip() {
        let db = seeded();
        let id = db.list_tracks(&TrackQuery::default()).expect("list")[0].id;
        db.set_favorite(id, true).expect("favorite");
        let favorites = db
            .list_tracks(&TrackQuery { favorites_only: true, ..Default::default() })
            .expect("list");
        assert_eq!(favorites.len(), 1);
    }

    #[test]
    fn play_history_updates_counts_and_recent() {
        let db = seeded();
        let id = db.list_tracks(&TrackQuery::default()).expect("list")[0].id;
        db.record_play(id, 1_700_000_500).expect("play");
        let track = db.get_track(id).expect("get").expect("row");
        assert_eq!(track.play_count, 1);
        assert_eq!(db.recent_tracks(10).expect("recent").len(), 1);
    }

    #[test]
    fn playlists_keep_order_and_survive_reindex() {
        let db = seeded();
        let tracks = db.list_tracks(&TrackQuery::default()).expect("list");
        let playlist = db.create_playlist("Late shift", "manual", None, 1).expect("create");
        db.add_to_playlist(playlist, &[tracks[0].id, tracks[1].id, tracks[2].id]).expect("add");
        db.move_playlist_item(playlist, 2, 0).expect("move");

        let ordered = db.playlist_tracks(playlist).expect("tracks");
        assert_eq!(ordered[0].id, tracks[2].id);
        assert_eq!(ordered.len(), 3);
        assert_eq!(db.playlists().expect("playlists")[0].track_count, 3);
    }

    #[test]
    fn smart_playlist_filters_by_artist_and_duration() {
        let db = seeded();
        let rules = SmartRules {
            match_any: false,
            limit: 50,
            rules: vec![
                SmartRule {
                    field: SmartField::Artist,
                    op: SmartOp::Contains,
                    value: "Kaito".into(),
                },
                SmartRule {
                    field: SmartField::DurationSeconds,
                    op: SmartOp::GreaterThan,
                    value: "60".into(),
                },
            ],
        };
        let tracks = db.smart_tracks(&rules).expect("smart");
        assert_eq!(tracks.len(), 2);

        let mismatched = SmartRules {
            rules: vec![SmartRule {
                field: SmartField::Artist,
                op: SmartOp::Is,
                value: String::new(),
            }],
            ..Default::default()
        };
        assert!(db.smart_tracks(&mismatched).expect("smart").is_empty());
    }

    #[test]
    fn smart_playlist_ignores_non_numeric_values() {
        let db = seeded();
        let rules = SmartRules {
            rules: vec![SmartRule {
                field: SmartField::Year,
                op: SmartOp::GreaterThan,
                value: "recent".into(),
            }],
            ..Default::default()
        };
        assert!(db.smart_tracks(&rules).expect("smart").is_empty());
    }

    #[test]
    fn folders_and_stats_report_real_numbers() {
        let db = seeded();
        db.add_folder("C:/music", 1).expect("folder");
        let folders = db.folders().expect("folders");
        assert_eq!(folders.len(), 1);

        let stats = db.stats().expect("stats");
        assert_eq!(stats.tracks, 3);
        assert_eq!(stats.total_duration_ms, 720_000);
    }

    #[test]
    fn settings_round_trip() {
        let db = LibraryDb::in_memory().expect("db");
        assert_eq!(db.get_setting("theme").expect("get"), None);
        db.set_setting("theme", "carbon").expect("set");
        assert_eq!(db.get_setting("theme").expect("get").as_deref(), Some("carbon"));
    }
}
