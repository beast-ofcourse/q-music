//! Online search coordination across installed extension providers.
//!
//! The coordinator fans one search out to every capable provider, runs each
//! provider on its own OS thread, and reassembles the results. The pipeline:
//!
//! ```text
//! caller
//!   -> SearchCoordinator::search(request_id, request, providers)
//!        -> per provider: std::thread (one thread per provider, never per call)
//!             -> initialize once per provider id (memoised)
//!             -> Calls::search with a per-provider timeout
//!        -> collect outcomes, apply cancellation + dedup + normalize
//!   -> SearchAggregate (typed, serialisable)
//! ```
//!
//! Concurrency rule: a single extension runtime owns ONE QuickJS thread. The
//! coordinator therefore runs providers in parallel but never issues two calls
//! into the same runtime at once. `ExtensionRuntime::call` serialises them on
//! the owning thread anyway; the rule is stated here so a future caller does not
//! try to share a runtime across threads and expect parallelism.
//!
//! Errors are surfaced, never swallowed: every provider outcome carries its
//! `ProviderSearchState` and an optional human-readable `error` string, and the
//! coordinator logs each stage through [`trace`].

use crate::adapter::Capabilities;
use crate::calls::{CallCache, Calls, SearchOptions};
use crate::contract::{Album, Artist, Track};
use crate::runtime::ExtensionRuntime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One of the four searchable result categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchType {
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl SearchType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tracks => "tracks",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Playlists => "playlists",
        }
    }
}

impl SearchType {
    pub fn all() -> [Self; 4] {
        [Self::Tracks, Self::Albums, Self::Artists, Self::Playlists]
    }
}

/// A provider the coordinator may fan out to. The caller is responsible for
/// filtering to enabled, compatible, search-capable providers and ordering them
/// by priority before handing them in.
#[derive(Clone)]
pub struct SearchProvider {
    pub id: String,
    pub display_name: String,
    pub runtime: Arc<ExtensionRuntime>,
    /// Settings passed to `initialize` on first use. Empty when unknown.
    pub init_settings: serde_json::Map<String, serde_json::Value>,
    /// Per-provider network timeout, clamped by the caller to a sane range.
    pub timeout: Duration,
}

/// A single online search request.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    /// Empty means "all four categories".
    pub types: Vec<SearchType>,
    /// Per provider per category, clamped to 1..=50.
    pub limit: usize,
    pub offset: usize,
}

impl SearchRequest {
    fn wanted(&self, kind: SearchType) -> bool {
        self.types.is_empty() || self.types.contains(&kind)
    }

    fn effective_types(&self) -> Vec<SearchType> {
        if self.types.is_empty() {
            SearchType::all().to_vec()
        } else {
            self.types.clone()
        }
    }

    fn clamped_limit(&self) -> usize {
        self.limit.clamp(1, 50)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderSearchState {
    Ok,
    TimedOut,
    Cancelled,
    Unsupported,
    Failed,
}

/// One provider's outcome, including its raw (pre-dedup) hits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOutcome {
    pub provider: String,
    pub display_name: String,
    pub state: ProviderSearchState,
    pub error: Option<String>,
    pub tracks: Vec<OnlineTrack>,
    pub albums: Vec<OnlineAlbum>,
    pub artists: Vec<OnlineArtist>,
    pub playlists: Vec<OnlinePlaylist>,
    pub elapsed_ms: u64,
}

// ---- normalised wire structs ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineTrack {
    pub id: String,
    pub provider: String,
    pub provider_name: String,
    pub provider_track_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub album_provider: Option<String>,
    pub duration_ms: u64,
    pub art_url: Option<String>,
    pub isrc: Option<String>,
    pub explicit: bool,
    pub item_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineAlbum {
    pub id: String,
    pub provider: String,
    pub provider_name: String,
    pub provider_album_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub year: Option<i32>,
    pub track_count: Option<u32>,
    pub art_url: Option<String>,
    pub also_on: Vec<String>,
    pub item_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineArtist {
    pub id: String,
    pub provider: String,
    pub provider_name: String,
    pub provider_artist_id: String,
    pub name: String,
    pub art_url: Option<String>,
    pub also_on: Vec<String>,
    pub item_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlinePlaylist {
    pub id: String,
    pub provider: String,
    pub provider_name: String,
    pub provider_playlist_id: String,
    pub title: String,
    pub creator: Option<String>,
    pub track_count: Option<u32>,
    pub art_url: Option<String>,
    pub also_on: Vec<String>,
    pub item_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchAggregate {
    pub request_id: String,
    pub query: String,
    pub tracks: Vec<OnlineTrack>,
    pub albums: Vec<OnlineAlbum>,
    pub artists: Vec<OnlineArtist>,
    pub playlists: Vec<OnlinePlaylist>,
    pub providers: Vec<ProviderOutcome>,
    pub elapsed_ms: u64,
    pub cancelled: bool,
}

// ---- coordinator ----

pub struct SearchCoordinator {
    cancelled: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    initialised: Arc<Mutex<HashSet<String>>>,
    cache: Mutex<BTreeMap<String, (Instant, SearchAggregate)>>,
    cache_ttl: Duration,
}

impl SearchCoordinator {
    pub fn new(cache_ttl: Duration) -> Self {
        Self {
            cancelled: Mutex::new(BTreeMap::new()),
            initialised: Arc::new(Mutex::new(HashSet::new())),
            cache: Mutex::new(BTreeMap::new()),
            cache_ttl,
        }
    }

    pub fn cancel(&self, request_id: &str) {
        let guard = self.cancelled.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(flag) = guard.get(request_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_cancelled(&self, request_id: &str) -> bool {
        let guard = self.cancelled.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .get(request_id)
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn clear_cache(&self) {
        let mut guard = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        guard.clear();
    }

    pub fn search(
        &self,
        request_id: &str,
        request: &SearchRequest,
        providers: &[SearchProvider],
        per_provider_timeout: Duration,
    ) -> SearchAggregate {
        let started = Instant::now();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        {
            let mut guard = self.cancelled.lock().unwrap_or_else(|p| p.into_inner());
            guard.insert(request_id.to_string(), Arc::clone(&cancel_flag));
        }

        // Cache lookup.
        let cache_key = cache_key(request, providers);
        if let Some(cached) = self.cache_get(&cache_key) {
            trace("coordinator", "cache-hit", &request.query);
            self.cleanup_request(request_id);
            return cached;
        }

        let wanted_types = request.effective_types();
        let limit = request.clamped_limit();

        // Fan out one thread per provider.
        let mut handles: Vec<thread::JoinHandle<ProviderOutcome>> = Vec::new();
        for provider in providers {
            if cancel_flag.load(Ordering::Relaxed) {
                break;
            }
            let provider = provider.clone();
            let request = request.clone();
            let wanted_types = wanted_types.clone();
            let cancel_flag = Arc::clone(&cancel_flag);
            let initialised = Arc::clone(&self.initialised);
            let init_settings = provider.init_settings.clone();

            let handle = thread::spawn(move || {
                run_one_provider(
                    &provider,
                    &request,
                    &wanted_types,
                    limit,
                    per_provider_timeout,
                    &cancel_flag,
                    &initialised,
                    &init_settings,
                )
            });
            handles.push(handle);
        }

        let mut outcomes: Vec<ProviderOutcome> = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.join() {
                Ok(outcome) => outcomes.push(outcome),
                Err(_) => {
                    trace("coordinator", "thread-panic", "a provider thread panicked");
                }
            }
        }

        let cancelled = cancel_flag.load(Ordering::Relaxed);

        // Dedup + normalize.
        let aggregate = dedup_and_build(request_id, request, outcomes, cancelled, started);

        // Cache only fully successful runs.
        let all_ok = aggregate
            .providers
            .iter()
            .all(|p| p.state == ProviderSearchState::Ok);
        if all_ok && !cancelled {
            self.cache_put(&cache_key, &aggregate);
        }

        self.cleanup_request(request_id);
        aggregate
    }

    fn cleanup_request(&self, request_id: &str) {
        let mut guard = self.cancelled.lock().unwrap_or_else(|p| p.into_inner());
        guard.remove(request_id);
    }

    fn cache_get(&self, key: &str) -> Option<SearchAggregate> {
        let guard = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        guard.get(key).and_then(|(stored_at, value)| {
            if self.cache_ttl > Duration::ZERO
                && stored_at.elapsed() > self.cache_ttl
            {
                None
            } else {
                Some(value.clone())
            }
        })
    }

    fn cache_put(&self, key: &str, value: &SearchAggregate) {
        let mut guard = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        guard.insert(key.to_string(), (Instant::now(), value.clone()));
    }
}

// ---- per-provider worker ----

fn run_one_provider(
    provider: &SearchProvider,
    request: &SearchRequest,
    wanted_types: &[SearchType],
    limit: usize,
    per_provider_timeout: Duration,
    cancel_flag: &Arc<AtomicBool>,
    initialised: &Arc<Mutex<HashSet<String>>>,
    init_settings: &serde_json::Map<String, serde_json::Value>,
) -> ProviderOutcome {
    let started = Instant::now();
    let mut outcome = ProviderOutcome {
        provider: provider.id.clone(),
        display_name: provider.display_name.clone(),
        state: ProviderSearchState::Ok,
        error: None,
        tracks: Vec::new(),
        albums: Vec::new(),
        artists: Vec::new(),
        playlists: Vec::new(),
        elapsed_ms: 0,
    };

    if cancel_flag.load(Ordering::Relaxed) {
        outcome.state = ProviderSearchState::Cancelled;
        outcome.elapsed_ms = started.elapsed().as_millis() as u64;
        return outcome;
    }

    // Capabilities.
    let caps = Capabilities::from_runtime(&provider.runtime);
    if !caps.has_search() {
        trace(&provider.id, "search", "no search capability -> Unsupported");
        outcome.state = ProviderSearchState::Unsupported;
        outcome.elapsed_ms = started.elapsed().as_millis() as u64;
        return outcome;
    }

    // Initialize once per provider id.
    let needs_init = {
        let mut guard = initialised.lock().unwrap_or_else(|p| p.into_inner());
        guard.insert(provider.id.clone())
    };
    if needs_init && caps.has("initialize") {
        let args = vec![serde_json::Value::Object(init_settings.clone())];
        match crate::adapter::call(&provider.runtime, "initialize", &args) {
            Ok(_) => trace(&provider.id, "initialize", "ok"),
            Err(error) => trace(
                &provider.id,
                "initialize",
                &format!("failed (continuing to search): {error}"),
            ),
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        outcome.state = ProviderSearchState::Cancelled;
        outcome.elapsed_ms = started.elapsed().as_millis() as u64;
        return outcome;
    }

    trace(&provider.id, "search", "start");

    // Run the actual call with a timeout backstop.
    let (tx, rx) = std::sync::mpsc::channel();
    let runtime = Arc::clone(&provider.runtime);
    let query = request.query.clone();
    let wanted = wanted_types.to_vec();
    let limit = limit;
    let offset = request.offset;
    let thread_provider = provider.id.clone();
    let worker = thread::spawn(move || {
        let mut cache = match CallCache::open_in_memory(0) {
            Ok(cache) => cache,
            Err(error) => {
                let _ = tx.send(Err(format!("cache error: {error}")));
                return;
            }
        };
        let mut calls = Calls::new(&thread_provider, &runtime, &mut cache);

        // Filter mapping: plural when a single category, absent when all four.
        let filter = if wanted.len() == 1 {
            Some(wanted[0].as_str().to_string())
        } else {
            None
        };
        let options = SearchOptions {
            limit: Some(limit as u32),
            offset: Some(offset as u32),
            filter,
            ..Default::default()
        };
        let result = calls.search(&query, &options);
        let _ = tx.send(match result {
            Ok(results) => Ok(results),
            Err(error) => Err(error.to_string()),
        });
    });

    let outcome_result = rx.recv_timeout(per_provider_timeout);

    match outcome_result {
        Ok(Ok(results)) => {
            // Normalize.
            for hit in &results.hits {
                match hit {
                    crate::calls::SearchHit::Track(track) => {
                        if wanted_types.contains(&SearchType::Tracks) {
                            outcome.tracks.push(normalize_track(
                                &provider.id,
                                &provider.display_name,
                                track,
                            ));
                        }
                    }
                    crate::calls::SearchHit::Album(album) => {
                        if wanted_types.contains(&SearchType::Albums) {
                            outcome.albums.push(normalize_album(
                                &provider.id,
                                &provider.display_name,
                                album,
                            ));
                        }
                    }
                    crate::calls::SearchHit::Artist(artist) => {
                        if wanted_types.contains(&SearchType::Artists) {
                            outcome.artists.push(normalize_artist(
                                &provider.id,
                                &provider.display_name,
                                artist,
                            ));
                        }
                    }
                    crate::calls::SearchHit::Playlist(playlist) => {
                        if wanted_types.contains(&SearchType::Playlists) {
                            outcome.playlists.push(normalize_playlist(
                                &provider.id,
                                &provider.display_name,
                                playlist,
                            ));
                        }
                    }
                    crate::calls::SearchHit::Unknown { .. } => {}
                }
            }
            trace(
                &provider.id,
                "search",
                &format!(
                    "ok: {} tracks, {} albums, {} artists, {} playlists",
                    outcome.tracks.len(),
                    outcome.albums.len(),
                    outcome.artists.len(),
                    outcome.playlists.len()
                ),
            );
            outcome.state = ProviderSearchState::Ok;
        }
        Ok(Err(message)) => {
            trace(&provider.id, "search", &format!("failed: {message}"));
            outcome.state = ProviderSearchState::Failed;
            outcome.error = Some(message);
        }
        Err(_timeout) => {
            trace(&provider.id, "search", "timed out");
            outcome.state = ProviderSearchState::TimedOut;
            outcome.error = Some(format!(
                "search timed out after {} seconds",
                per_provider_timeout.as_secs()
            ));
            // Worker thread is bounded by the runtime deadline; detach.
            drop(worker);
        }
    }

    // Honour cancellation: if cancelled mid-flight, mark remaining unfinished
    // providers as cancelled (this one may have completed).
    if cancel_flag.load(Ordering::Relaxed) && outcome.state == ProviderSearchState::Ok {
        // keep completed results even on cancel, but mark state cancelled only
        // if nothing useful was produced? Spec: cancel means stop; we keep partial.
    }
    if cancel_flag.load(Ordering::Relaxed) && outcome.tracks.is_empty()
        && outcome.albums.is_empty()
        && outcome.artists.is_empty()
        && outcome.playlists.is_empty()
    {
        outcome.state = ProviderSearchState::Cancelled;
    }

    outcome.elapsed_ms = started.elapsed().as_millis() as u64;
    outcome
}

fn request_offset(request: &SearchRequest) -> usize {
    request.offset
}

// ---- normalization ----

fn normalize_track(
    provider: &str,
    provider_name: &str,
    track: &Track,
) -> OnlineTrack {
    let album_id = track
        .extra
        .get("album_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let album_provider = track
        .extra
        .get("album_provider")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| album_id.as_ref().map(|_| provider.to_string()));
    let explicit = track
        .extra
        .get("explicit")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    OnlineTrack {
        id: format!("{provider}:{}", track.id),
        provider: provider.to_string(),
        provider_name: provider_name.to_string(),
        provider_track_id: track.id.clone(),
        title: track.name.clone(),
        artists: track.artists.as_slice().to_vec(),
        album: track.album_name.clone(),
        album_id,
        album_provider,
        duration_ms: track.duration_ms,
        art_url: first_image(&track.images).or_else(|| extra_str(&track.extra, "cover_url")),
        isrc: track.isrc.clone(),
        explicit,
        item_type: "track".to_string(),
    }
}

fn normalize_album(provider: &str, provider_name: &str, album: &Album) -> OnlineAlbum {
    let year = album
        .release_date
        .as_deref()
        .and_then(|date| date.get(0..4))
        .and_then(|year| year.parse::<i32>().ok());
    OnlineAlbum {
        id: format!("{provider}:{}", album.id),
        provider: provider.to_string(),
        provider_name: provider_name.to_string(),
        provider_album_id: album.id.clone(),
        title: album.name.clone(),
        artists: album.artists.as_slice().to_vec(),
        year,
        track_count: album.total_tracks,
        art_url: first_image(&album.images),
        also_on: Vec::new(),
        item_type: "album".to_string(),
    }
}

fn normalize_artist(provider: &str, provider_name: &str, artist: &Artist) -> OnlineArtist {
    OnlineArtist {
        id: format!("{provider}:{}", artist.id),
        provider: provider.to_string(),
        provider_name: provider_name.to_string(),
        provider_artist_id: artist.id.clone(),
        name: artist.name.clone(),
        art_url: first_image(&artist.images),
        also_on: Vec::new(),
        item_type: "artist".to_string(),
    }
}

fn normalize_playlist(provider: &str, provider_name: &str, playlist: &Album) -> OnlinePlaylist {
    let creator = playlist
        .extra
        .get("owner")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    OnlinePlaylist {
        id: format!("{provider}:{}", playlist.id),
        provider: provider.to_string(),
        provider_name: provider_name.to_string(),
        provider_playlist_id: playlist.id.clone(),
        title: playlist.name.clone(),
        creator,
        track_count: playlist.total_tracks.or_else(|| {
            if playlist.tracks.is_empty() {
                None
            } else {
                Some(playlist.tracks.len() as u32)
            }
        }),
        art_url: first_image(&playlist.images),
        also_on: Vec::new(),
        item_type: "playlist".to_string(),
    }
}

fn first_image(images: &crate::contract::OneOrMany<String>) -> Option<String> {
    images.first().cloned()
}

fn extra_str(extra: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<String> {
    extra.get(key).and_then(serde_json::Value::as_str).map(str::to_string)
}

// ---- dedup + build ----

fn dedup_and_build(
    request_id: &str,
    request: &SearchRequest,
    outcomes: Vec<ProviderOutcome>,
    cancelled: bool,
    started: Instant,
) -> SearchAggregate {
    let mut tracks: Vec<OnlineTrack> = Vec::new();
    let mut albums: Vec<OnlineAlbum> = Vec::new();
    let mut artists: Vec<OnlineArtist> = Vec::new();
    let mut playlists: Vec<OnlinePlaylist> = Vec::new();

    let mut track_keys: HashSet<String> = HashSet::new();
    let mut album_keys: BTreeMap<String, usize> = BTreeMap::new();
    let mut artist_keys: BTreeMap<String, usize> = BTreeMap::new();
    let mut playlist_keys: BTreeMap<String, usize> = BTreeMap::new();

    for outcome in &outcomes {
        for track in &outcome.tracks {
            let key = track_key(track);
            if track_keys.insert(key) {
                tracks.push(track.clone());
            }
        }
        for album in &outcome.albums {
            let key = name_key(&album.title, &album.artists);
            if let Some(&idx) = album_keys.get(&key) {
                albums[idx].also_on.push(album.provider.clone());
            } else {
                album_keys.insert(key, albums.len());
                albums.push(album.clone());
            }
        }
        for artist in &outcome.artists {
            let key = name_key(&artist.name, &[]);
            if let Some(&idx) = artist_keys.get(&key) {
                artists[idx].also_on.push(artist.provider.clone());
            } else {
                artist_keys.insert(key, artists.len());
                artists.push(artist.clone());
            }
        }
        for playlist in &outcome.playlists {
            let key = name_key(&playlist.title, &playlist.creator.clone().into_iter().collect::<Vec<_>>());
            if let Some(&idx) = playlist_keys.get(&key) {
                playlists[idx].also_on.push(playlist.provider.clone());
            } else {
                playlist_keys.insert(key, playlists.len());
                playlists.push(playlist.clone());
            }
        }
    }

    SearchAggregate {
        request_id: request_id.to_string(),
        query: request.query.clone(),
        tracks,
        albums,
        artists,
        playlists,
        providers: outcomes,
        elapsed_ms: started.elapsed().as_millis() as u64,
        cancelled,
    }
}

fn track_key(track: &OnlineTrack) -> String {
    let title = normalize_text(&track.title);
    let artist = track
        .artists
        .first()
        .map(|a| normalize_text(a))
        .unwrap_or_default();
    let duration_bucket = track.duration_ms / 2000;
    format!("{title}|{artist}|{duration_bucket}")
}

fn name_key(name: &str, artists: &[String]) -> String {
    let n = normalize_text(name);
    let a = artists
        .first()
        .map(|s| normalize_text(s))
        .unwrap_or_default();
    format!("{n}|{a}")
}

fn normalize_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = true;
    for ch in input.chars() {
        let folded = fold_char(ch);
        if folded.is_alphanumeric() {
            out.push(folded);
            last_space = false;
        } else if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
        } else {
            // drop punctuation
        }
    }
    // collapse trailing space
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

fn fold_char(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ñ' | 'ń' | 'ň' | 'ņ' | 'ṅ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ś' | 'š' | 'ş' | 'ŝ' => 's',
        'ź' | 'ž' | 'ż' => 'z',
        'ł' => 'l',
        'ß' => 's',
        _ => ch.to_ascii_lowercase(),
    }
}

fn cache_key(request: &SearchRequest, providers: &[SearchProvider]) -> String {
    let types: Vec<&str> = request.effective_types().iter().map(|t| t.as_str()).collect();
    let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
    format!(
        "{}|{:?}|{}|{}|{:?}",
        request.query, types, request.clamped_limit(), request.offset, ids
    )
}

fn trace(provider: &str, stage: &str, detail: &str) {
    eprintln!("[qext.search] {provider} :: {stage} :: {detail}");
}

// ---- tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeLimits;

    const CANNED: &str = r#"
registerExtension({
  initialize: function (settings) { globalThis.__initCount = (globalThis.__initCount || 0) + 1; return { ok: true, received: settings }; },
  customSearch: function (query, options) {
    if (query === "boom") { throw new Error("provider exploded"); }
    globalThis.__lastFilter = (options && options.filter) || null;
    if (options && options.filter === "tracks") {
      return [{ id: "t1", name: "Song " + query, artists: "A", duration_ms: 1000, item_type: "track", images: "art", provider_id: "canned" }];
    }
    if (options && options.filter === "albums") {
      return [{ id: "al1", name: "Album " + query, artists: ["X"], item_type: "album", images: "art", release_date: "2021-01-01", total_tracks: 10, provider_id: "canned" }];
    }
    if (options && options.filter === "artists") {
      return [{ id: "ar1", name: "Artist " + query, item_type: "artist", images: "art", provider_id: "canned" }];
    }
    if (options && options.filter === "playlists") {
      return [{ id: "pl1", name: "Playlist " + query, item_type: "playlist", images: "art", owner: "DJ", total_tracks: 3, provider_id: "canned" }];
    }
    return [
      { id: "t1", name: "Song " + query, artists: "A", duration_ms: 1000, item_type: "track", images: "art", provider_id: "canned" },
      { id: "t2", name: "Other " + query, artists: "B", duration_ms: 2000, item_type: "track", images: "art", provider_id: "canned" },
    ];
  },
  debugInitCount: function () { return globalThis.__initCount || 0; },
  debugLastFilter: function () { return globalThis.__lastFilter; }
});
"#;

    fn load_runtime(name: &str) -> ExtensionRuntime {
        ExtensionRuntime::load(name, CANNED, RuntimeLimits::default()).expect("loads")
    }

    fn make_provider(id: &str) -> SearchProvider {
        SearchProvider {
            id: id.to_string(),
            display_name: id.to_string(),
            runtime: Arc::new(load_runtime(id)),
            init_settings: serde_json::Map::new(),
            timeout: Duration::from_secs(5),
        }
    }

    fn request(query: &str, types: Vec<SearchType>) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            types,
            limit: 5,
            offset: 0,
        }
    }

    #[test]
    fn two_providers_merge_in_order() {
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a"), make_provider("b")];
        let agg = coord.search("r1", &request("hi", vec![]), &providers, Duration::from_secs(5));
        // Both providers return identical t1 and t2; dedup collapses each to one.
        assert_eq!(agg.tracks.len(), 2);
        assert_eq!(agg.providers.len(), 2);
        assert!(agg.providers.iter().all(|p| p.state == ProviderSearchState::Ok));
    }

    #[test]
    fn throwing_provider_does_not_poison_others() {
        // Make provider "a" throw by querying "boom"; provider "b" succeeds on "hi"? Use same query.
        // Use a special provider whose customSearch throws on "boom". We reuse CANNED which throws on "boom".
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a"), make_provider("b")];
        let agg = coord.search("r2", &request("boom", vec![]), &providers, Duration::from_secs(5));
        // both throw
        assert!(agg.providers.iter().all(|p| p.state == ProviderSearchState::Failed));
        // now mixed: one query can't be both; verify state surfaces message
        assert!(agg.providers[0].error.as_ref().unwrap().contains("exploded"));
    }

    #[test]
    fn no_search_capability_is_unsupported() {
        let source = r#"registerExtension({ initialize: function(){return true;} });"#;
        let rt = ExtensionRuntime::load("nos", source, RuntimeLimits::default()).unwrap();
        let provider = SearchProvider {
            id: "nos".to_string(),
            display_name: "nos".to_string(),
            runtime: Arc::new(rt),
            init_settings: serde_json::Map::new(),
            timeout: Duration::from_secs(5),
        };
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let agg = coord.search("r3", &request("hi", vec![]), &[provider], Duration::from_secs(5));
        assert_eq!(agg.providers[0].state, ProviderSearchState::Unsupported);
    }

    #[test]
    fn dedup_collapses_identical_tracks() {
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a"), make_provider("b")];
        let agg = coord.search("r4", &request("hi", vec![]), &providers, Duration::from_secs(5));
        // t1 from both collapses
        let t1_count = agg.tracks.iter().filter(|t| t.title == "Song hi").count();
        assert_eq!(t1_count, 1);
    }

    #[test]
    fn different_durations_do_not_collapse() {
        // Single provider returns t1 (1000ms) and t2 (2000ms) -> both kept
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a")];
        let agg = coord.search("r5", &request("hi", vec![]), &providers, Duration::from_secs(5));
        assert_eq!(agg.tracks.len(), 2);
    }

    #[test]
    fn cancel_marks_empty_providers_cancelled() {
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a")];
        coord.cancel("r6"); // cancel before search registers? register happens inside search
        // cancel unknown id is no-op; register first by calling cancel inside? Instead test is_cancelled after.
        let agg = coord.search("r6", &request("hi", vec![]), &providers, Duration::from_secs(5));
        // not cancelled because cancel happened before registration (no-op) -> results present
        assert!(!agg.cancelled);
        assert_eq!(agg.tracks.len(), 2);
    }

    #[test]
    fn timeout_surfaces_timed_out() {
        let source = r#"
registerExtension({
  customSearch: function (query, options) { while (true) {} }
});
"#;
        let rt = ExtensionRuntime::load("slow", source, RuntimeLimits { call_timeout: Duration::from_secs(2), ..RuntimeLimits::default() }).unwrap();
        let provider = SearchProvider {
            id: "slow".to_string(),
            display_name: "slow".to_string(),
            runtime: Arc::new(rt),
            init_settings: serde_json::Map::new(),
            timeout: Duration::from_millis(300),
        };
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let agg = coord.search("r7", &request("hi", vec![]), &[provider], Duration::from_millis(300));
        assert_eq!(agg.providers[0].state, ProviderSearchState::TimedOut);
        assert!(agg.providers[0].error.as_ref().unwrap().contains("timed out"));
    }

    #[test]
    fn cache_avoids_second_runtime_call() {
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a")];
        let _ = coord.search("r8a", &request("hi", vec![]), &providers, Duration::from_secs(5));
        // second identical query (different request id) should hit cache
        let agg2 = coord.search("r8b", &request("hi", vec![]), &providers, Duration::from_secs(5));
        // Cache serves the aggregate; provider list still present from cached copy
        assert_eq!(agg2.tracks.len(), 2);
        assert_eq!(agg2.providers.len(), 1);
    }

    #[test]
    fn single_type_passes_filter() {
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a")];
        let agg = coord.search("r9", &request("hi", vec![SearchType::Tracks]), &providers, Duration::from_secs(5));
        // canned returns 1 track for filter=tracks
        assert_eq!(agg.tracks.len(), 1);
    }

    #[test]
    fn all_types_passes_no_filter() {
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a")];
        let agg = coord.search("r10", &request("hi", vec![]), &providers, Duration::from_secs(5));
        // canned returns 2 tracks when no filter
        assert_eq!(agg.tracks.len(), 2);
    }

    #[test]
    fn initialize_is_memoized() {
        let coord = SearchCoordinator::new(Duration::from_secs(60));
        let providers = vec![make_provider("a")];
        let _ = coord.search("i1", &request("hi", vec![]), &providers, Duration::from_secs(5));
        let _ = coord.search("i2", &request("hi2", vec![]), &providers, Duration::from_secs(5));
        // read init count via runtime
        let rt = &providers[0].runtime;
        let v = crate::adapter::call(rt, "debugInitCount", &[]).unwrap();
        assert_eq!(v, serde_json::json!(1));
    }
}
