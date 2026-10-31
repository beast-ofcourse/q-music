//! Metadata, search and lyrics calls against one installed provider.
//!
//! One [`Calls`] value wraps a runtime, a provider name and a [`CallCache`].
//! Every method follows the same three steps:
//!
//! 1. ask the cache, keyed by provider + function + arguments;
//! 2. call the package through `adapter::call`, which turns an unimplemented
//!    function into a typed refusal instead of an exception;
//! 3. normalise the JSON with the adapter, then store the raw success value.
//!
//! Failures are never written to the cache. A provider that throws for one
//! query leaves no entry behind, so a second provider answering the same query
//! is unaffected and a retry after the fault is a fresh call.
//!
//! Every returned value carries the provider id. The UI renders a source chip
//! from it and never has to guess which package produced a track.

use crate::adapter::{self, AdapterError, Capabilities};
use crate::contract::{Album, Artist, LyricsResult, Track};
use crate::runtime::ExtensionRuntime;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Search arguments understood by every package family.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    /// `tracks`, `albums`, `artists`, `playlists`, or `all`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Provider-specific options passed through untouched.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// What one search hit turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchHit {
    Track(Box<Track>),
    Album(Box<Album>),
    Artist(Box<Artist>),
    /// A playlist uses the album shape: id, name, artwork, tracks.
    Playlist(Box<Album>),
    /// An `item_type` the host does not model, kept verbatim so a newer
    /// provider is visible rather than silently dropped.
    Unknown { item_type: String, value: Value },
}

impl SearchHit {
    pub fn item_type(&self) -> &str {
        match self {
            Self::Track(_) => "track",
            Self::Album(_) => "album",
            Self::Artist(_) => "artist",
            Self::Playlist(_) => "playlist",
            Self::Unknown { item_type, .. } => item_type,
        }
    }
}

/// A search result list. Mixed by design: `customSearch` returns tracks,
/// albums, artists and playlists in one flat array.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    pub provider: String,
    pub hits: Vec<SearchHit>,
}

impl SearchResults {
    /// Only the tracks, for callers that need a simple queue source.
    pub fn tracks(&self) -> Vec<&Track> {
        self.hits
            .iter()
            .filter_map(|hit| match hit {
                SearchHit::Track(track) => Some(track.as_ref()),
                _ => None,
            })
            .collect()
    }
}

/// One result plus the provider that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved<T> {
    pub provider: String,
    pub item: T,
}

/// What `handleUrl` resolved a link to.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedLink {
    Track(Box<Track>),
    Album(Box<Album>),
    Artist(Box<Artist>),
    Playlist(Box<Album>),
}

/// Arguments for `matchTrack`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MatchRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Arguments for `fetchLyrics`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LyricsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    pub track_name: String,
    pub artist_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CallError {
    #[error(transparent)]
    Adapter(#[from] AdapterError),

    #[error("call cache failed: {0}")]
    Cache(String),

    #[error("provider `{provider}` does not implement `{function}`")]
    Unsupported { provider: String, function: String },

    #[error("provider `{provider}` failed `{function}`: {message}")]
    Provider {
        provider: String,
        function: String,
        message: String,
    },

    #[error("provider `{provider}` returned unusable `{function}` data: {reason}")]
    Invalid {
        provider: String,
        function: String,
        reason: String,
    },
}

impl CallError {
    fn invalid(provider: &str, function: &str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            provider: provider.to_string(),
            function: function.to_string(),
            reason: reason.into(),
        }
    }
}

/// SQLite-backed cache for call results.
///
/// The key is `(provider, function, canonical arguments)`, so two providers
/// answering the same query keep separate entries and a different argument set
/// is a different entry. `ttl_seconds == 0` means entries never expire.
pub struct CallCache {
    connection: Connection,
    ttl_seconds: u64,
}

impl CallCache {
    pub fn open_in_memory(ttl_seconds: u64) -> Result<Self, CallError> {
        let connection = Connection::open_in_memory().map_err(cache_error)?;
        Self::from_connection(connection, ttl_seconds)
    }

    pub fn open(path: impl AsRef<Path>, ttl_seconds: u64) -> Result<Self, CallError> {
        let connection = Connection::open(path).map_err(cache_error)?;
        Self::from_connection(connection, ttl_seconds)
    }

    fn from_connection(connection: Connection, ttl_seconds: u64) -> Result<Self, CallError> {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS call_cache (
                     provider   TEXT NOT NULL,
                     function   TEXT NOT NULL,
                     query      TEXT NOT NULL,
                     payload    TEXT NOT NULL,
                     stored_at  INTEGER NOT NULL,
                     PRIMARY KEY (provider, function, query)
                 );",
            )
            .map_err(cache_error)?;

        Ok(Self {
            connection,
            ttl_seconds,
        })
    }

    pub fn ttl_seconds(&self) -> u64 {
        self.ttl_seconds
    }

    pub fn get(
        &self,
        provider: &str,
        function: &str,
        query: &str,
    ) -> Result<Option<Value>, CallError> {
        let row = self.connection.query_row(
            "SELECT payload, stored_at FROM call_cache
             WHERE provider = ?1 AND function = ?2 AND query = ?3",
            params![provider, function, query],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        );

        let (payload, stored_at) = match row {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(cache_error(error)),
        };

        if self.is_expired(stored_at) {
            self.delete(provider, function, query)?;
            return Ok(None);
        }

        let value = serde_json::from_str(&payload).map_err(|error| {
            CallError::Cache(format!("cached payload is not valid JSON: {error}"))
        })?;
        Ok(Some(value))
    }

    pub fn put(
        &mut self,
        provider: &str,
        function: &str,
        query: &str,
        value: &Value,
    ) -> Result<(), CallError> {
        let payload = serde_json::to_string(value)
            .map_err(|error| CallError::Cache(format!("cannot serialise result: {error}")))?;

        self.connection
            .execute(
                "INSERT OR REPLACE INTO call_cache
                     (provider, function, query, payload, stored_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![provider, function, query, payload, now_seconds()],
            )
            .map_err(cache_error)?;
        Ok(())
    }

    pub fn invalidate_provider(&mut self, provider: &str) -> Result<usize, CallError> {
        self.connection
            .execute(
                "DELETE FROM call_cache WHERE provider = ?1",
                params![provider],
            )
            .map_err(cache_error)
    }

    pub fn purge_expired(&mut self) -> Result<usize, CallError> {
        if self.ttl_seconds == 0 {
            return Ok(0);
        }

        let oldest = now_seconds().saturating_sub(self.ttl_seconds as i64);
        self.connection
            .execute(
                "DELETE FROM call_cache WHERE stored_at <= ?1",
                params![oldest],
            )
            .map_err(cache_error)
    }

    pub fn clear(&mut self) -> Result<(), CallError> {
        self.connection
            .execute("DELETE FROM call_cache", [])
            .map_err(cache_error)?;
        Ok(())
    }

    fn is_expired(&self, stored_at: i64) -> bool {
        self.ttl_seconds > 0 && now_seconds().saturating_sub(stored_at) > self.ttl_seconds as i64
    }

    fn delete(&self, provider: &str, function: &str, query: &str) -> Result<(), CallError> {
        self.connection
            .execute(
                "DELETE FROM call_cache
                 WHERE provider = ?1 AND function = ?2 AND query = ?3",
                params![provider, function, query],
            )
            .map_err(cache_error)?;
        Ok(())
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn cache_error(error: rusqlite::Error) -> CallError {
    CallError::Cache(error.to_string())
}

/// Calls against one provider, with a shared cache.
pub struct Calls<'a> {
    provider: &'a str,
    runtime: &'a ExtensionRuntime,
    cache: &'a mut CallCache,
    capabilities: Capabilities,
}

impl<'a> Calls<'a> {
    pub fn new(
        provider: &'a str,
        runtime: &'a ExtensionRuntime,
        cache: &'a mut CallCache,
    ) -> Self {
        let capabilities = Capabilities::from_runtime(runtime);
        Self {
            provider,
            runtime,
            cache,
            capabilities,
        }
    }

    pub fn provider(&self) -> &str {
        self.provider
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// Search with the function the package actually registered and the
    /// argument shape it declares.
    pub fn search(
        &mut self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchResults, CallError> {
        let function = if self.capabilities.has("customSearch") {
            "customSearch"
        } else if self.capabilities.has("searchTracks") {
            "searchTracks"
        } else {
            return Err(self.unsupported("searchTracks"));
        };

        let args = if function == "customSearch" {
            vec![json!(query), self.search_options(options)?]
        } else {
            let limit = options.limit.unwrap_or(20);
            match self.capabilities.arity(function) {
                Some(1) => vec![json!(query)],
                _ => vec![json!(query), json!(limit)],
            }
        };

        let value = self.call_cached(function, &args)?;
        let hits = self.search_hits(function, &value)?;

        Ok(SearchResults {
            provider: self.provider.to_string(),
            hits,
        })
    }

    pub fn track(&mut self, id: &str) -> Result<Option<Resolved<Track>>, CallError> {
        let value = self.call_required("getTrack", &[json!(id)])?;
        self.resolve(value, "getTrack", adapter::track_from_value)
    }

    pub fn album(&mut self, id: &str) -> Result<Option<Resolved<Album>>, CallError> {
        let value = self.call_required("getAlbum", &[json!(id)])?;
        self.resolve(value, "getAlbum", adapter::album_from_value)
    }

    pub fn artist(&mut self, id: &str) -> Result<Option<Resolved<Artist>>, CallError> {
        let value = self.call_required("getArtist", &[json!(id)])?;
        self.resolve(value, "getArtist", adapter::artist_from_value)
    }

    /// Playlists have the album shape: identity, artwork and a track list.
    pub fn playlist(&mut self, id: &str) -> Result<Option<Resolved<Album>>, CallError> {
        let value = self.call_required("getPlaylist", &[json!(id)])?;
        self.resolve(value, "getPlaylist", adapter::album_from_value)
    }

    /// Home feed sections. The shape is provider-specific, so it is returned
    /// as JSON after a failure marker check rather than forced into a lie.
    pub fn home_feed(&mut self) -> Result<Option<Resolved<Value>>, CallError> {
        let args = if self.capabilities.arity("getHomeFeed") == Some(0) {
            Vec::new()
        } else {
            vec![Value::Object(Map::new())]
        };

        let value = self.call_required_args("getHomeFeed", &args)?;
        Ok(self.some(value))
    }

    pub fn browse_categories(&mut self) -> Result<Option<Resolved<Value>>, CallError> {
        let args = if self.capabilities.arity("getBrowseCategories") == Some(0) {
            Vec::new()
        } else {
            vec![Value::Object(Map::new())]
        };

        let value = self.call_required_args("getBrowseCategories", &args)?;
        Ok(self.some(value))
    }

    /// Resolve a share link into the item it points at.
    pub fn handle_url(&mut self, url: &str) -> Result<Option<Resolved<ResolvedLink>>, CallError> {
        let Some(value) = self.call_required("handleUrl", &[json!(url)])? else {
            return Ok(None);
        };

        let link = self.resolved_link("handleUrl", &value)?;
        Ok(Some(Resolved {
            provider: self.provider.to_string(),
            item: link,
        }))
    }

    pub fn enrich(&mut self, track: &Track) -> Result<Option<Resolved<Track>>, CallError> {
        let metadata = serde_json::to_value(track).map_err(|error| {
            CallError::invalid(self.provider, "enrichTrack", error.to_string())
        })?;
        let value = self.call_required("enrichTrack", &[metadata])?;
        self.resolve(value, "enrichTrack", adapter::track_from_value)
    }

    pub fn match_track(
        &mut self,
        request: &MatchRequest,
    ) -> Result<Option<Resolved<Track>>, CallError> {
        let arity = self.capabilities.arity("matchTrack");
        let args = match arity {
            None => return Err(self.unsupported("matchTrack")),
            Some(0) => Vec::new(),
            Some(1) => vec![self.request_value(request, "matchTrack")?],
            Some(2) => vec![json!(request.isrc), json!(request.track_name)],
            Some(3) => vec![
                json!(request.isrc),
                json!(request.track_name),
                json!(request.artist_name),
            ],
            Some(_) => vec![
                json!(request.isrc),
                json!(request.track_name),
                json!(request.artist_name),
                self.request_value(request, "matchTrack")?,
            ],
        };

        let value = self.call_required_args("matchTrack", &args)?;
        self.resolve(value, "matchTrack", adapter::track_from_value)
    }

    /// Lyrics with the argument shape the package declares.
    ///
    /// `apple-music` ships `fetchLyrics(trackName, artistName, albumName,
    /// durationSec)`; the guide documents a single request object. Both reach
    /// the package correctly, and the result is normalised into one type.
    pub fn lyrics(
        &mut self,
        request: &LyricsRequest,
    ) -> Result<Option<Resolved<LyricsResult>>, CallError> {
        let arity = self.capabilities.arity("fetchLyrics");
        let args = match arity {
            None => return Err(self.unsupported("fetchLyrics")),
            Some(0) => Vec::new(),
            Some(1) => vec![self.request_value(request, "fetchLyrics")?],
            Some(2) => vec![json!(request.track_name), json!(request.artist_name)],
            Some(3) => vec![
                json!(request.track_name),
                json!(request.artist_name),
                json!(request.album_name),
            ],
            Some(4) => vec![
                json!(request.track_name),
                json!(request.artist_name),
                json!(request.album_name),
                json!(request.duration_seconds),
            ],
            Some(_) => vec![
                json!(request.track_name),
                json!(request.artist_name),
                json!(request.album_name),
                json!(request.duration_seconds),
                self.request_value(request, "fetchLyrics")?,
            ],
        };

        let value = self.call_required_args("fetchLyrics", &args)?;
        let Some(value) = value else {
            return Ok(None);
        };

        let lyrics = adapter::lyrics_from_value(value)
            .map_err(CallError::from)?
            .map(|lyrics| Resolved {
                provider: self.provider.to_string(),
                item: lyrics,
            });

        Ok(lyrics)
    }

    fn call_cached(&mut self, function: &str, args: &[Value]) -> Result<Value, CallError> {
        let query = canonical_arguments(args);
        if let Some(cached) = self.cache.get(self.provider, function, &query)? {
            return Ok(cached);
        }

        let value = self.run(function, args)?;
        self.cache.put(self.provider, function, &query, &value)?;
        Ok(value)
    }

    fn run(&self, function: &str, args: &[Value]) -> Result<Value, CallError> {
        adapter::call(self.runtime, function, args).map_err(|error| match error {
            AdapterError::Unsupported { function } => self.unsupported_owned(function),
            other => CallError::Adapter(other),
        })
    }

    /// Cached call that must be implemented; `None` means the package has no
    /// such function.
    fn call_required(
        &mut self,
        function: &str,
        args: &[Value],
    ) -> Result<Option<Value>, CallError> {
        self.call_required_args(function, args)
    }

    fn call_required_args(
        &mut self,
        function: &str,
        args: &[Value],
    ) -> Result<Option<Value>, CallError> {
        if !self.capabilities.has(function) {
            return Err(self.unsupported(function));
        }

        let value = self.call_cached(function, args)?;
        if value.is_null() {
            return Ok(None);
        }

        if let Some(message) = failure_message(&value) {
            // A failure marker is an error, not a cacheable empty result.
            self.cache
                .delete(self.provider, function, &canonical_arguments(args))?;
            return Err(CallError::Provider {
                provider: self.provider.to_string(),
                function: function.to_string(),
                message,
            });
        }

        Ok(Some(value))
    }

    fn resolve<T>(
        &self,
        value: Option<Value>,
        function: &str,
        parse: fn(&Value) -> Result<T, AdapterError>,
    ) -> Result<Option<Resolved<T>>, CallError> {
        let Some(value) = value else {
            return Ok(None);
        };

        let item = parse(&value)
            .map_err(|error| CallError::invalid(self.provider, function, error.to_string()))?;

        Ok(Some(Resolved {
            provider: self.provider.to_string(),
            item,
        }))
    }

    fn some(&self, value: Option<Value>) -> Option<Resolved<Value>> {
        value.map(|value| Resolved {
            provider: self.provider.to_string(),
            item: value,
        })
    }

    fn search_hits(&self, function: &str, value: &Value) -> Result<Vec<SearchHit>, CallError> {
        let entries = match value {
            Value::Null => return Ok(Vec::new()),
            Value::Array(entries) => entries.as_slice(),
            other => {
                return Err(CallError::invalid(
                    self.provider,
                    function,
                    format!("expected an array of results, got {other}"),
                ));
            }
        };

        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                self.search_hit(entry).map_err(|error| match error {
                    CallError::Invalid { reason, .. } => CallError::invalid(
                        self.provider,
                        function,
                        format!("result {index}: {reason}"),
                    ),
                    other => other,
                })
            })
            .collect()
    }

    fn search_hit(&self, entry: &Value) -> Result<SearchHit, CallError> {
        let item_type = entry
            .get("item_type")
            .or_else(|| entry.get("itemType"))
            .and_then(Value::as_str)
            .unwrap_or("track");

        match item_type {
            "track" => adapter::track_from_value(entry)
                .map(|track| SearchHit::Track(Box::new(track)))
                .map_err(CallError::from),
            "album" => adapter::album_from_value(entry)
                .map(|album| SearchHit::Album(Box::new(album)))
                .map_err(CallError::from),
            "artist" => adapter::artist_from_value(entry)
                .map(|artist| SearchHit::Artist(Box::new(artist)))
                .map_err(CallError::from),
            "playlist" => adapter::album_from_value(entry)
                .map(|album| SearchHit::Playlist(Box::new(album)))
                .map_err(CallError::from),
            other => Ok(SearchHit::Unknown {
                item_type: other.to_string(),
                value: entry.clone(),
            }),
        }
    }

    fn resolved_link(&self, function: &str, value: &Value) -> Result<ResolvedLink, CallError> {
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CallError::invalid(self.provider, function, "resolved link has no `type`")
            })?;

        let nested = |key: &str| {
            value.get(key).cloned().ok_or_else(|| {
                CallError::invalid(
                    self.provider,
                    function,
                    format!("resolved `{kind}` link has no `{key}` object"),
                )
            })
        };

        match kind {
            "track" => Ok(ResolvedLink::Track(Box::new(
                adapter::track_from_value(&nested("track")?)
                    .map_err(|error| CallError::invalid(self.provider, function, error.to_string()))?,
            ))),
            "album" => Ok(ResolvedLink::Album(Box::new(
                adapter::album_from_value(&nested("album")?)
                    .map_err(|error| CallError::invalid(self.provider, function, error.to_string()))?,
            ))),
            "artist" => Ok(ResolvedLink::Artist(Box::new(
                adapter::artist_from_value(&nested("artist")?)
                    .map_err(|error| CallError::invalid(self.provider, function, error.to_string()))?,
            ))),
            "playlist" => Ok(ResolvedLink::Playlist(Box::new(
                adapter::album_from_value(&nested("playlist")?)
                    .map_err(|error| CallError::invalid(self.provider, function, error.to_string()))?,
            ))),
            other => Err(CallError::invalid(
                self.provider,
                function,
                format!("unsupported link type `{other}`"),
            )),
        }
    }

    fn search_options(&self, options: &SearchOptions) -> Result<Value, CallError> {
        serde_json::to_value(options)
            .map_err(|error| CallError::invalid(self.provider, "customSearch", error.to_string()))
    }

    fn request_value<T: Serialize>(&self, request: &T, function: &str) -> Result<Value, CallError> {
        serde_json::to_value(request)
            .map_err(|error| CallError::invalid(self.provider, function, error.to_string()))
    }

    fn unsupported(&self, function: &str) -> CallError {
        self.unsupported_owned(function.to_string())
    }

    fn unsupported_owned(&self, function: String) -> CallError {
        CallError::Unsupported {
            provider: self.provider.to_string(),
            function,
        }
    }
}

/// Stable cache key for an argument list.
fn canonical_arguments(args: &[Value]) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
}

/// A `success: false` object is an error, even when the package also returned
/// empty fields next to it.
fn failure_message(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let failed = matches!(object.get("success"), Some(Value::Bool(false)));
    if !failed {
        return None;
    }

    let message = ["error", "error_message", "message"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("provider reported failure without a reason");

    Some(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeLimits;

    const CANNED: &str = r#"
var searchCalls = 0;
registerExtension({
  initialize: function () { return true; },
  customSearch: function (query, options) {
    searchCalls++;
    return [
      { id: 't1', name: 'Song ' + query, artists: 'A', duration_ms: 1000, item_type: 'track', provider_id: 'canned' },
      { id: 'al1', name: 'Album', artists: ['X'], item_type: 'album', provider_id: 'canned' },
      { id: 'ar1', name: 'Artist', item_type: 'artist', provider_id: 'canned' },
      { id: 'pl1', name: 'Playlist', item_type: 'playlist', provider_id: 'canned' },
      { id: 'v1', name: 'Visualiser', item_type: 'visualiser', provider_id: 'canned' }
    ];
  },
  getTrack: function (id) {
    return { id: id, name: 'Track ' + id, artists: 'A', duration_ms: 1, images: 'art' };
  },
  getAlbum: function (id) {
    return { album_info: { id: id, name: 'Album ' + id }, track_list: [{ id: 't1', name: 'One' }] };
  },
  getArtist: function (id) {
    return { artist: { id: id, name: 'Artist ' + id } };
  },
  getPlaylist: function (id) {
    return { id: id, name: 'Playlist ' + id, tracks: [{ id: 'p1', name: 'P' }] };
  },
  getHomeFeed: function () { return { sections: [{ title: 'Made for you' }] }; },
  getBrowseCategories: function () { return { categories: [{ id: 'pop' }] }; },
  handleUrl: function (url) {
    return { success: true, type: 'track', track: { id: url, name: 'From URL' } };
  },
  enrichTrack: function (track) {
    track.name = track.name + ' (enriched)';
    return track;
  },
  matchTrack: function (isrc, trackName, artistName, options) {
    return { id: 'matched', name: trackName, artists: artistName, isrc: isrc };
  },
  fetchLyrics: function (trackName, artistName, albumName, durationSec) {
    return { syncType: 'LINE_SYNCED', lines: [{ startTimeMs: 0, words: 'hi' }] };
  },
  debugSearchCalls: function () { return searchCalls; }
});
"#;

    fn load(name: &str, source: &str) -> ExtensionRuntime {
        ExtensionRuntime::load(name, source, RuntimeLimits::default()).expect("loads")
    }

    fn cache() -> CallCache {
        CallCache::open_in_memory(0).expect("cache opens")
    }

    fn options(limit: u32) -> SearchOptions {
        SearchOptions {
            limit: Some(limit),
            ..SearchOptions::default()
        }
    }

    fn lyrics_request() -> LyricsRequest {
        LyricsRequest {
            track_id: Some("t1".to_string()),
            track_name: "Song".to_string(),
            artist_name: "Artist".to_string(),
            album_name: Some("Album".to_string()),
            duration_seconds: Some(200),
            ..LyricsRequest::default()
        }
    }

    #[test]
    fn every_registered_call_round_trips_through_the_fixture_provider() {
        let runtime = load("canned", CANNED);
        let mut cache = cache();
        let mut calls = Calls::new("canned", &runtime, &mut cache);

        let search = calls.search("hello", &options(5)).expect("search");
        assert_eq!(search.provider, "canned");
        assert_eq!(search.hits.len(), 5);
        assert_eq!(
            search
                .hits
                .iter()
                .map(SearchHit::item_type)
                .collect::<Vec<_>>(),
            vec!["track", "album", "artist", "playlist", "visualiser"]
        );
        assert_eq!(search.tracks()[0].name, "Song hello");
        let SearchHit::Artist(artist) = &search.hits[2] else {
            panic!("expected an artist hit");
        };
        assert_eq!(artist.name, "Artist");
        let SearchHit::Unknown { item_type, .. } = &search.hits[4] else {
            panic!("expected an unknown hit");
        };
        assert_eq!(*item_type, "visualiser");

        let track = calls.track("a").expect("track").expect("result");
        assert_eq!(track.provider, "canned");
        assert_eq!(track.item.name, "Track a");
        assert_eq!(track.item.artwork_url(), Some("art"));

        let album = calls.album("al").expect("album").expect("result");
        assert_eq!(album.item.name, "Album al");
        assert_eq!(album.item.tracks.len(), 1);

        let artist = calls.artist("ar").expect("artist").expect("result");
        assert_eq!(artist.item.name, "Artist ar");

        let playlist = calls.playlist("pl").expect("playlist").expect("result");
        assert_eq!(playlist.item.name, "Playlist pl");
        assert_eq!(playlist.item.tracks[0].name, "P");

        let feed = calls.home_feed().expect("feed").expect("result");
        assert_eq!(feed.provider, "canned");
        assert_eq!(feed.item["sections"][0]["title"], json!("Made for you"));

        let categories = calls
            .browse_categories()
            .expect("categories")
            .expect("result");
        assert_eq!(categories.item["categories"][0]["id"], json!("pop"));

        let link = calls.handle_url("https://example.test/track/1").expect("url");
        let ResolvedLink::Track(track) = link.expect("resolved").item else {
            panic!("expected a track link");
        };
        assert_eq!(track.name, "From URL");

        let enriched = calls
            .enrich(&track_from_value(&json!({ "id": "e1", "name": "Song" })).expect("parses"))
            .expect("enrich")
            .expect("result");
        assert_eq!(enriched.item.name, "Song (enriched)");

        let matched = calls
            .match_track(&MatchRequest {
                isrc: Some("ISRC1".to_string()),
                track_name: Some("Song".to_string()),
                artist_name: Some("Artist".to_string()),
                duration_ms: Some(1000),
                ..MatchRequest::default()
            })
            .expect("match")
            .expect("result");
        assert_eq!(matched.item.id, "matched");
        assert_eq!(matched.item.isrc.as_deref(), Some("ISRC1"));

        let lyrics = calls
            .lyrics(&lyrics_request())
            .expect("lyrics")
            .expect("result");
        assert_eq!(lyrics.provider, "canned");
        assert!(lyrics.item.is_synced());
        assert_eq!(lyrics.item.lines[0].words, "hi");
    }

    fn track_from_value(value: &Value) -> Result<Track, AdapterError> {
        adapter::track_from_value(value)
    }

    #[test]
    fn a_cache_hit_avoids_a_second_runtime_call() {
        let runtime = load("canned", CANNED);
        let mut cache = cache();

        {
            let mut calls = Calls::new("canned", &runtime, &mut cache);
            calls.search("same", &options(5)).expect("first search");
            calls.search("same", &options(5)).expect("second search");
            calls.track("a").expect("first track");
            calls.track("a").expect("second track");
        }

        assert_eq!(
            runtime.call("debugSearchCalls", &[]).expect("debug"),
            json!(1),
            "the second identical search must not reach the package"
        );
    }

    #[test]
    fn cache_entries_are_keyed_by_provider_and_arguments() {
        let runtime = load("canned", CANNED);
        let mut cache = cache();

        {
            let mut first = Calls::new("provider-a", &runtime, &mut cache);
            first.track("x").expect("track");
        }

        let query_x = canonical_arguments(&[json!("x")]);
        assert!(cache.get("provider-a", "getTrack", &query_x).expect("read").is_some());
        assert!(cache.get("provider-b", "getTrack", &query_x).expect("read").is_none());

        let query_y = canonical_arguments(&[json!("y")]);
        assert!(cache.get("provider-a", "getTrack", &query_y).expect("read").is_none());
    }

    #[test]
    fn a_throwing_provider_caches_nothing_and_does_not_disturb_another_provider() {
        let broken = load(
            "broken",
            "registerExtension({ getTrack: function () { throw new Error('boom'); } });",
        );
        let working = load("working", CANNED);
        let mut cache = cache();

        let error = Calls::new("broken", &broken, &mut cache)
            .track("same")
            .unwrap_err();
        assert!(
            matches!(&error, CallError::Adapter(AdapterError::Threw { function, message })
                if function == "getTrack" && message.contains("boom")),
            "unexpected error: {error:?}"
        );

        assert!(
            cache
                .get("broken", "getTrack", &canonical_arguments(&[json!("same")]))
                .expect("read")
                .is_none(),
            "a failure must not poison the cache"
        );

        let result = Calls::new("working", &working, &mut cache)
            .track("same")
            .expect("second provider")
            .expect("result");
        assert_eq!(result.item.id, "same");
    }

    #[test]
    fn a_failure_marker_is_an_error_and_is_not_cached() {
        let runtime = load(
            "failing",
            "registerExtension({ getAlbum: function () { return { success: false, message: 'gone' }; } });",
        );
        let mut cache = cache();

        let error = Calls::new("failing", &runtime, &mut cache)
            .album("al")
            .unwrap_err();
        assert_eq!(
            error,
            CallError::Provider {
                provider: "failing".to_string(),
                function: "getAlbum".to_string(),
                message: "gone".to_string(),
            }
        );
        assert!(cache
            .get("failing", "getAlbum", &canonical_arguments(&[json!("al")]))
            .expect("read")
            .is_none());

        let error = Calls::new("failing", &runtime, &mut cache)
            .handle_url("https://example.test/x")
            .unwrap_err();
        assert!(matches!(error, CallError::Unsupported { .. }));
    }

    #[test]
    fn a_provider_without_the_function_reports_a_typed_refusal() {
        let runtime = load(
            "bare",
            "registerExtension({ initialize: function () { return true; } });",
        );
        let mut cache = cache();
        let mut calls = Calls::new("bare", &runtime, &mut cache);

        assert_eq!(
            calls.track("a").unwrap_err(),
            CallError::Unsupported {
                provider: "bare".to_string(),
                function: "getTrack".to_string(),
            }
        );
        assert!(matches!(
            calls.search("q", &options(5)).unwrap_err(),
            CallError::Unsupported { .. }
        ));
        assert!(matches!(
            calls.lyrics(&lyrics_request()).unwrap_err(),
            CallError::Unsupported { .. }
        ));
    }

    #[test]
    fn search_uses_search_tracks_when_custom_search_is_absent() {
        let runtime = load(
            "tracks-only",
            "registerExtension({ searchTracks: function (query, limit) { return [{ id: '1', name: query + ':' + limit }]; } });",
        );
        let mut cache = cache();
        let mut calls = Calls::new("tracks-only", &runtime, &mut cache);

        let results = calls
            .search("jazz", &SearchOptions { limit: Some(7), ..SearchOptions::default() })
            .expect("search");
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.tracks()[0].name, "jazz:7");
    }

    #[test]
    fn custom_search_receives_the_option_shape_including_extra_keys() {
        let runtime = load(
            "options",
            "registerExtension({ customSearch: function (query, options) { return [{ id: '1', name: query + '|' + options.limit + '|' + options.filter + '|' + options.region }]; } });",
        );
        let mut cache = cache();
        let mut calls = Calls::new("options", &runtime, &mut cache);

        let mut extra = BTreeMap::new();
        extra.insert("region".to_string(), json!("GB"));

        let results = calls
            .search(
                "pop",
                &SearchOptions {
                    limit: Some(3),
                    filter: Some("albums".to_string()),
                    extra,
                    ..SearchOptions::default()
                },
            )
            .expect("search");

        assert_eq!(results.tracks()[0].name, "pop|3|albums|GB");
    }

    #[test]
    fn unusable_search_payloads_are_typed_errors_not_silent_empties() {
        let runtime = load(
            "wrong-shape",
            "registerExtension({ customSearch: function () { return { results: [] }; } });",
        );
        let mut cache = cache();

        let error = Calls::new("wrong-shape", &runtime, &mut cache)
            .search("q", &options(1))
            .unwrap_err();
        match error {
            CallError::Invalid { provider, function, reason } => {
                assert_eq!(provider, "wrong-shape");
                assert_eq!(function, "customSearch");
                assert!(reason.contains("array"), "got {reason:?}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn lyrics_normalise_synced_unsynced_and_instrumental_results() {
        struct Case {
            name: &'static str,
            source: &'static str,
            check: fn(&LyricsResult),
        }

        let cases = [
            Case {
                name: "synced",
                source: "registerExtension({ fetchLyrics: function (trackName, artistName, albumName, durationSec) { return { syncType: 'LINE_SYNCED', lines: [{ startTimeMs: '1500', words: 'a' }, { startTimeMs: 3000, words: 'b' }] }; } });",
                check: |lyrics| {
                    assert!(lyrics.is_synced());
                    assert_eq!(lyrics.lines.len(), 2);
                    assert_eq!(lyrics.lines[0].start_time_ms, 1_500);
                },
            },
            Case {
                name: "unsynced",
                source: "registerExtension({ fetchLyrics: function (trackName, artistName, albumName, durationSec) { return { syncType: 'UNSYNCED', plainLyrics: 'la la', lines: [] }; } });",
                check: |lyrics| {
                    assert!(!lyrics.is_synced());
                    assert!(lyrics.has_content());
                    assert_eq!(lyrics.plain_lyrics.as_deref(), Some("la la"));
                },
            },
            Case {
                name: "instrumental-request-form",
                source: "registerExtension({ fetchLyrics: function (request) { return { instrumental: true, trackName: request.trackName }; } });",
                check: |lyrics| {
                    assert!(lyrics.has_content());
                    assert!(lyrics.lines.is_empty());
                    assert_eq!(lyrics.extra["trackName"], json!("Song"));
                },
            },
        ];

        for case in cases {
            let runtime = load(case.name, case.source);
            let mut cache = cache();
            let result = Calls::new(case.name, &runtime, &mut cache)
                .lyrics(&lyrics_request())
                .expect("lyrics call")
                .expect("result");

            assert_eq!(result.provider, case.name);
            (case.check)(&result.item);
            assert_eq!(
                Calls::new(case.name, &runtime, &mut cache)
                    .lyrics(&lyrics_request())
                    .expect("cached lyrics")
                    .expect("result")
                    .item,
                result.item,
                "the cache returns the same normalised lyrics"
            );
        }
    }

    #[test]
    fn null_results_mean_absent_without_an_error() {
        let runtime = load(
            "empty",
            "registerExtension({ getTrack: function () { return null; }, fetchLyrics: function (trackName, artistName, albumName, durationSec) { return null; } });",
        );
        let mut cache = cache();
        let mut calls = Calls::new("empty", &runtime, &mut cache);

        assert_eq!(calls.track("missing").expect("no error"), None);
        assert_eq!(calls.lyrics(&lyrics_request()).expect("no error"), None);
    }

    #[test]
    fn search_results_keep_the_provider_id_on_every_hit_type() {
        let runtime = load("canned", CANNED);
        let mut cache = cache();
        let results = Calls::new("canned", &runtime, &mut cache)
            .search("q", &options(5))
            .expect("search");

        assert_eq!(results.provider, "canned");
        for hit in &results.hits {
            let provider_id = match hit {
                SearchHit::Track(track) => track.extra.get("provider_id"),
                SearchHit::Album(album) | SearchHit::Playlist(album) => {
                    album.extra.get("provider_id")
                }
                SearchHit::Artist(artist) => artist.extra.get("provider_id"),
                SearchHit::Unknown { value, .. } => value.get("provider_id"),
            };
            assert_eq!(provider_id, Some(&json!("canned")), "hit {}", hit.item_type());
        }
    }

    #[test]
    fn invalidating_one_provider_leaves_another_providers_entries_alone() {
        let runtime = load("canned", CANNED);
        let mut cache = cache();

        {
            let mut first = Calls::new("provider-a", &runtime, &mut cache);
            first.track("x").expect("track");
        }
        {
            let mut second = Calls::new("provider-b", &runtime, &mut cache);
            second.track("x").expect("track");
        }

        assert_eq!(cache.invalidate_provider("provider-a").expect("invalidate"), 1);
        let query = canonical_arguments(&[json!("x")]);
        assert!(cache.get("provider-a", "getTrack", &query).expect("read").is_none());
        assert!(cache.get("provider-b", "getTrack", &query).expect("read").is_some());

        cache.clear().expect("clear");
        assert!(cache.get("provider-b", "getTrack", &query).expect("read").is_none());
    }
}
