//! Compatibility layer between the host's vocabulary and what packages ship.
//!
//! Three jobs, all of them about the gap between the development guide and the
//! package bytes:
//!
//! - **Capabilities come from the registration, never from the manifest.** A
//!   package that only registers `initialize` has no search, and asking it to
//!   search is a typed "unsupported" instead of an exception. Functions the host
//!   has never heard of are reported rather than dropped, so a future package
//!   surface is visible instead of silently ignored.
//! - **`download` is dispatched on declared arity.** The guide documents
//!   `download(request, progressCallback)`; every shipped package implements
//!   `download(trackID, quality, outputPath, onProgress, options)`. Both are
//!   called correctly from one entry point.
//! - **Results are normalised once.** Failure text arrives as `error`,
//!   `error_message` or `message`; `images` and `artists` arrive as a string or
//!   an array; lyric times arrive as numbers or numeric strings. Everything is
//!   coerced here so the call layer and the queue never special-case a package.
//!
//! Progress is the one place this module reaches into the bridge: a callback
//! cannot travel as JSON, so the adapter sends a marker object and the prelude
//! turns it into a function that forwards its arguments to the host. See
//! [`CALLBACK_MARKER`].

use crate::contract::{Album, Artist, DownloadResult, LyricsResult, Track};
use crate::runtime::{ExtensionRuntime, FunctionInfo, RuntimeError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use thiserror::Error;

/// Key the adapter puts in an argument that the package must receive as a
/// function. The prelude replaces the object with a callback that forwards its
/// arguments to the host's progress sink.
pub const CALLBACK_MARKER: &str = "__qext_callback__";

/// Registered function names the host understands.
///
/// Anything absent from this list is a package surface the host has not been
/// taught, which is worth reporting: dropping it silently would make a new
/// upstream feature look like a package that simply does nothing.
pub const KNOWN_FUNCTIONS: [&str; 21] = [
    "initialize",
    "cleanup",
    "completeGrant",
    "searchTracks",
    "customSearch",
    "getHomeFeed",
    "getBrowseCategories",
    "handleUrl",
    "getTrack",
    "getAlbum",
    "getArtist",
    "getPlaylist",
    "enrichTrack",
    "matchTrack",
    "fetchLyrics",
    "checkAvailability",
    "getDownloadUrl",
    "validateTrackForDownload",
    "download",
    "postProcess",
    "postProcessV2",
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("extension does not implement `{function}`")]
    Unsupported { function: String },

    #[error("extension function `{function}` threw: {message}")]
    Threw { function: String, message: String },

    #[error("extension function `{function}` exceeded its {timeout_ms} ms deadline")]
    Timeout { function: String, timeout_ms: u64 },

    #[error("extension call `{function}` failed: {reason}")]
    Failed { function: String, reason: String },

    #[error("extension returned unexpected data from `{function}`: {reason}")]
    Invalid { function: String, reason: String },
}

impl AdapterError {
    fn invalid(function: &str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            function: function.to_string(),
            reason: reason.into(),
        }
    }
}

impl From<RuntimeError> for AdapterError {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::MissingFunction { function } => Self::Unsupported { function },
            RuntimeError::Threw { function, message } => Self::Threw { function, message },
            RuntimeError::Timeout {
                function,
                timeout_ms,
            } => Self::Timeout {
                function,
                timeout_ms,
            },
            RuntimeError::InvalidResult(reason) => Self::Invalid {
                function: "<result>".to_string(),
                reason,
            },
            RuntimeError::NotRegistered => Self::Failed {
                function: "<load>".to_string(),
                reason: "extension did not call registerExtension()".to_string(),
            },
            RuntimeError::Stopped => Self::Failed {
                function: "<runtime>".to_string(),
                reason: "extension runtime is not running".to_string(),
            },
            RuntimeError::Start(reason) => Self::Failed {
                function: "<start>".to_string(),
                reason,
            },
        }
    }
}

/// What one package registered, and what that means for the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    functions: Vec<FunctionInfo>,
}

impl Capabilities {
    pub fn from_surface(surface: &[FunctionInfo]) -> Self {
        Self {
            functions: surface.to_vec(),
        }
    }

    pub fn from_runtime(runtime: &ExtensionRuntime) -> Self {
        Self::from_surface(runtime.surface())
    }

    /// Every registered function, in registration order.
    pub fn registered(&self) -> &[FunctionInfo] {
        &self.functions
    }

    pub fn has(&self, function: &str) -> bool {
        self.functions.iter().any(|entry| entry.name == function)
    }

    pub fn arity(&self, function: &str) -> Option<u32> {
        self.functions
            .iter()
            .find(|entry| entry.name == function)
            .map(|entry| entry.arity)
    }

    /// Search is either the dedicated function or the filtered one; packages
    /// ship one, the other, or both.
    pub fn has_search(&self) -> bool {
        self.has("searchTracks") || self.has("customSearch")
    }

    pub fn is_download_provider(&self) -> bool {
        self.has("download")
    }

    pub fn is_metadata_provider(&self) -> bool {
        ["getTrack", "getAlbum", "getArtist", "getPlaylist", "handleUrl"]
            .iter()
            .any(|name| self.has(name))
    }

    pub fn has_availability(&self) -> bool {
        self.has("checkAvailability")
    }

    pub fn has_lyrics(&self) -> bool {
        self.has("fetchLyrics")
    }

    pub fn has_home_feed(&self) -> bool {
        self.has("getHomeFeed")
    }

    /// Registered names outside [`KNOWN_FUNCTIONS`].
    ///
    /// Reported with their arity so a host can log "this package also offers
    /// `getConcertList(a,b)`" instead of pretending the surface is complete.
    pub fn unrecognised(&self) -> Vec<&FunctionInfo> {
        self.functions
            .iter()
            .filter(|entry| !KNOWN_FUNCTIONS.contains(&entry.name.as_str()))
            .collect()
    }
}

/// One `download` call, in the host's vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadRequest {
    pub track_id: String,
    pub quality: String,
    pub output_path: String,
    pub isrc: Option<String>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub duration_ms: Option<u64>,
    /// Echo of `checkAvailability().prepared_context`, when the host has one.
    pub prepared_context: Option<Value>,
    /// The host's track object, for packages that prefer metadata over a lookup.
    pub metadata: Option<Value>,
}

impl DownloadRequest {
    pub fn new(
        track_id: impl Into<String>,
        quality: impl Into<String>,
        output_path: impl Into<String>,
    ) -> Self {
        Self {
            track_id: track_id.into(),
            quality: quality.into(),
            output_path: output_path.into(),
            isrc: None,
            track_name: None,
            artist_name: None,
            duration_ms: None,
            prepared_context: None,
            metadata: None,
        }
    }

    pub fn with_isrc(mut self, isrc: impl Into<String>) -> Self {
        self.isrc = Some(isrc.into());
        self
    }

    pub fn with_names(mut self, track_name: impl Into<String>, artist_name: impl Into<String>) -> Self {
        self.track_name = Some(track_name.into());
        self.artist_name = Some(artist_name.into());
        self
    }

    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_prepared_context(mut self, context: Value) -> Self {
        self.prepared_context = Some(context);
        self
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Which calling convention a `download` implementation uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadSignature {
    /// `download(trackID, quality, outputPath, onProgress, options)` - what every
    /// shipped package implements.
    Positional,
    /// `download(request, progressCallback)` - what the guide documents.
    RequestObject,
}

/// Dispatch rule: the documented two-argument form is exactly
/// `(request, progressCallback)`, so two declared parameters mean the request
/// object. Everything else is positional, because that is the family the
/// shipped packages and the guide's own 4-argument example belong to.
pub fn download_signature(arity: u32) -> DownloadSignature {
    if arity == 2 {
        DownloadSignature::RequestObject
    } else {
        DownloadSignature::Positional
    }
}

/// How a download ended, after normalisation.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadOutcome {
    /// Boxed because a full result carries twenty optional metadata fields and
    /// would otherwise dominate every value of this enum.
    Completed(Box<DownloadResult>),
    Failed(DownloadFailure),
}

impl DownloadOutcome {
    /// Normalise whatever `download` returned.
    ///
    /// A non-object return is a failure, not a panic: a package that returns
    /// `null` on an error path is common and the message has to say so.
    pub fn from_value(value: Value) -> Self {
        if !value.is_object() {
            return Self::Failed(DownloadFailure {
                kind: DownloadFailureKind::DownloadError,
                message: format!(
                    "download provider returned {} instead of a result object",
                    describe(&value)
                ),
                retry_after_seconds: None,
                auth_url: None,
            });
        }

        let result = download_result_from(&value);
        if result.success {
            return Self::Completed(Box::new(result));
        }

        let message = result
            .error_text()
            .map(str::to_string)
            .unwrap_or_else(|| "download provider reported failure without a reason".to_string());

        Self::Failed(DownloadFailure {
            kind: DownloadFailureKind::from_hint(result.error_type.as_deref()),
            message,
            retry_after_seconds: extra_u64(&result.extra, "retry_after_seconds"),
            auth_url: extra_string(&result.extra, "auth_url"),
        })
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Completed(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadFailure {
    /// Machine-readable kind, so the queue and the UI never string-match.
    pub kind: DownloadFailureKind,
    pub message: String,
    pub retry_after_seconds: Option<u64>,
    pub auth_url: Option<String>,
}

/// The documented `error_type` values, plus a verbatim fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadFailureKind {
    /// No `error_type` was supplied.
    Unknown,
    InvalidInput,
    NotFound,
    Unavailable,
    Timeout,
    Cancelled,
    VerificationRequired,
    RateLimited,
    Authentication,
    DownloadError,
    ApiError,
    PostProcess,
    GeoBlocked,
    StreamError,
    FormatError,
    QuotaExceeded,
    PremiumRequired,
    ServerError,
    /// A package-specific value the host does not model. Kept verbatim so it can
    /// be logged and shown without guessing what it means.
    Other(String),
}

impl DownloadFailureKind {
    pub fn from_hint(hint: Option<&str>) -> Self {
        let Some(hint) = hint.map(str::trim).filter(|text| !text.is_empty()) else {
            return Self::Unknown;
        };

        match hint.to_ascii_lowercase().as_str() {
            "unknown" => Self::Unknown,
            "invalid_input" => Self::InvalidInput,
            "not_found" => Self::NotFound,
            "unavailable" | "not_available" => Self::Unavailable,
            "timeout" => Self::Timeout,
            "cancelled" | "canceled" => Self::Cancelled,
            "verification_required" => Self::VerificationRequired,
            "rate_limited" => Self::RateLimited,
            "authentication_error" | "auth_error" => Self::Authentication,
            "download_error" => Self::DownloadError,
            "api_error" => Self::ApiError,
            "post_process_error" => Self::PostProcess,
            "geo_blocked" => Self::GeoBlocked,
            "stream_error" => Self::StreamError,
            "format_error" => Self::FormatError,
            "quota_exceeded" => Self::QuotaExceeded,
            "premium_required" => Self::PremiumRequired,
            "server_error" => Self::ServerError,
            _ => Self::Other(hint.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Unknown => "unknown",
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::VerificationRequired => "verification_required",
            Self::RateLimited => "rate_limited",
            Self::Authentication => "authentication_error",
            Self::DownloadError => "download_error",
            Self::ApiError => "api_error",
            Self::PostProcess => "post_process_error",
            Self::GeoBlocked => "geo_blocked",
            Self::StreamError => "stream_error",
            Self::FormatError => "format_error",
            Self::QuotaExceeded => "quota_exceeded",
            Self::PremiumRequired => "premium_required",
            Self::ServerError => "server_error",
            Self::Other(other) => other,
        }
    }
}

/// Serialised as the stable code from [`DownloadFailureKind::as_str`], so a
/// queue event and a log line use the same identifier.
impl Serialize for DownloadFailureKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DownloadFailureKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Ok(Self::from_hint(Some(&code)))
    }
}

/// Call `download` with the argument shape the package declares.
pub fn download(
    runtime: &ExtensionRuntime,
    request: &DownloadRequest,
) -> Result<DownloadOutcome, AdapterError> {
    let arity = runtime
        .arity("download")
        .ok_or_else(|| AdapterError::Unsupported {
            function: "download".to_string(),
        })?;

    let args = download_arguments(arity, request);
    let value = call(runtime, "download", &args)?;

    Ok(DownloadOutcome::from_value(value))
}

/// The exact argument list for an implementation of `arity` parameters.
pub fn download_arguments(arity: u32, request: &DownloadRequest) -> Vec<Value> {
    if download_signature(arity) == DownloadSignature::RequestObject {
        return vec![request_object(request), progress_callback()];
    }

    let mut args: Vec<Value> = Vec::new();
    // A zero-parameter `download` is a stub; calling it with nothing is the only
    // interpretation that cannot be wrong.
    if arity == 0 {
        return args;
    }

    args.push(json!(request.track_id));
    if arity == 1 {
        return args;
    }

    args.push(json!(request.quality));
    args.push(json!(request.output_path));
    if arity == 3 {
        return args;
    }

    args.push(progress_callback());
    if arity == 4 {
        return args;
    }

    args.push(positional_options(request));
    args
}

fn progress_callback() -> Value {
    json!({ CALLBACK_MARKER: "progress" })
}

/// `options` for the positional form. `preparedContext` mirrors the value
/// `checkAvailability` returned; `track` is the host's metadata, which packages
/// use instead of repeating a catalog lookup.
fn positional_options(request: &DownloadRequest) -> Value {
    let mut options = Map::new();
    if let Some(context) = &request.prepared_context {
        options.insert("preparedContext".to_string(), context.clone());
    }
    if let Some(track) = &request.metadata {
        options.insert("track".to_string(), track.clone());
    }
    Value::Object(options)
}

#[derive(Serialize)]
struct RequestPayload<'a> {
    track_id: &'a str,
    quality: &'a str,
    output_path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    isrc: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepared_context: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<&'a Value>,
}

fn request_object(request: &DownloadRequest) -> Value {
    serde_json::to_value(RequestPayload {
        track_id: &request.track_id,
        quality: &request.quality,
        output_path: &request.output_path,
        isrc: request.isrc.as_deref(),
        track_name: request.track_name.as_deref(),
        artist_name: request.artist_name.as_deref(),
        duration_ms: request.duration_ms,
        prepared_context: request.prepared_context.as_ref(),
        metadata: request.metadata.as_ref(),
    })
    .unwrap_or(Value::Null)
}

/// Invoke any registered function, mapping runtime failures to
/// [`AdapterError`].
pub fn call(
    runtime: &ExtensionRuntime,
    function: &str,
    args: &[Value],
) -> Result<Value, AdapterError> {
    runtime.call(function, args).map_err(AdapterError::from)
}

/// Fields [`DownloadResult`] models; everything else is kept in `extra`.
///
/// `auth_url` and `retry_after_seconds` are deliberately absent: the struct has
/// no field for them, so they must land in `extra` where the queue reads them.
const DOWNLOAD_FIELDS: [&str; 20] = [
    "success",
    "file_path",
    "path",
    "output_path",
    "format",
    "actual_bit_depth",
    "actual_sample_rate",
    "title",
    "artist",
    "album",
    "album_artist",
    "track_number",
    "disc_number",
    "release_date",
    "cover_url",
    "isrc",
    "error",
    "error_message",
    "message",
    "error_type",
];

/// Read a download result without trusting any single key name.
fn download_result_from(value: &Value) -> DownloadResult {
    let object = value.as_object();

    let field = |key: &str| object.and_then(|map| map.get(key));

    let mut extra = BTreeMap::new();
    if let Some(map) = object {
        for (key, value) in map {
            if !DOWNLOAD_FIELDS.contains(&key.as_str()) {
                extra.insert(key.clone(), value.clone());
            }
        }
    }

    DownloadResult {
        success: field("success").map(truthy).unwrap_or(false),
        file_path: first_string(field("file_path"), field("path")).or_else(|| text(field("output_path"))),
        format: text(field("format")),
        actual_bit_depth: first_u32(field("actual_bit_depth"), field("bit_depth")),
        actual_sample_rate: first_u32(field("actual_sample_rate"), field("sample_rate")),
        title: text(field("title")),
        artist: text(field("artist")),
        album: text(field("album")),
        album_artist: text(field("album_artist")),
        track_number: field("track_number").and_then(value_u32),
        disc_number: field("disc_number").and_then(value_u32),
        release_date: text(field("release_date")),
        cover_url: text(field("cover_url")),
        isrc: text(field("isrc")),
        error: text(field("error")),
        error_message: text(field("error_message")),
        message: text(field("message")),
        error_type: text(field("error_type")),
        extra,
    }
}

fn first_string(left: Option<&Value>, right: Option<&Value>) -> Option<String> {
    text(left).or_else(|| text(right))
}

fn first_u32(left: Option<&Value>, right: Option<&Value>) -> Option<u32> {
    left.and_then(value_u32).or_else(|| right.and_then(value_u32))
}

/// A string, or a JSON encoding of whatever else was sent. A package that puts
/// an object under `error` still produces readable failure text.
fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        other => Some(other.to_string()),
    }
}

fn value_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|wide| u32::try_from(wide).ok()),
        Value::String(text) => text.trim().parse::<u64>().ok().and_then(|wide| u32::try_from(wide).ok()),
        _ => None,
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => text.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn extra_u64(extra: &BTreeMap<String, Value>, key: &str) -> Option<u64> {
    match extra.get(key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn extra_string(extra: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    extra.get(key).and_then(|value| text(Some(value)))
}

fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Array(_) => "an array".to_string(),
        Value::Bool(flag) => format!("a boolean ({flag})"),
        Value::Number(number) => format!("a number ({number})"),
        Value::String(text) => format!("a string ({})", truncate(text, 60)),
        Value::Object(_) => "an object".to_string(),
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}...")
}

/// Parse a track, keeping unknown fields in `extra`.
pub fn track_from_value(value: &Value) -> Result<Track, AdapterError> {
    parse::<Track>("track", value)
}

/// Parse an album, tolerating the internal `{album_info, track_list}` wrapper
/// that packages use between their fetch and format steps.
pub fn album_from_value(value: &Value) -> Result<Album, AdapterError> {
    let object = value.as_object();
    let info = object.and_then(|map| map.get("album_info").filter(|value| value.is_object()));

    match info {
        Some(info) => {
            let mut album = parse::<Album>("album", info)?;
            if let Some(tracks) = object.and_then(|map| map.get("track_list")) {
                album.tracks = tracks_from_value(tracks)?;
            }
            Ok(album)
        }
        None => parse::<Album>("album", value),
    }
}

pub fn artist_from_value(value: &Value) -> Result<Artist, AdapterError> {
    let object = value.as_object();
    let inner = object.and_then(|map| map.get("artist").filter(|value| value.is_object()));

    match inner {
        Some(inner) => parse::<Artist>("artist", inner),
        None => parse::<Artist>("artist", value),
    }
}

/// Parse a track list.
///
/// Accepts the flat array every `customSearch` returns, and the `tracks` or
/// `track_list` wrapper an album or playlist uses. A malformed entry fails the
/// list with its index named: silently dropping it would hide a broken provider.
pub fn tracks_from_value(value: &Value) -> Result<Vec<Track>, AdapterError> {
    let entries = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(entries) => entries.as_slice(),
        Value::Object(map) => map
            .get("tracks")
            .or_else(|| map.get("track_list"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .ok_or_else(|| {
                AdapterError::invalid("tracks", "object has neither a `tracks` nor `track_list` array")
            })?,
        other => {
            return Err(AdapterError::invalid(
                "tracks",
                format!("expected a track list, got {}", describe(other)),
            ));
        }
    };

    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            parse::<Track>("track", entry).map_err(|error| AdapterError::invalid("tracks", format!("entry {index}: {error}")))
        })
        .collect()
}

/// Parse a lyrics result. `null` means "no lyrics", which is not an error.
pub fn lyrics_from_value(value: Value) -> Result<Option<LyricsResult>, AdapterError> {
    match value {
        Value::Null => Ok(None),
        Value::Object(_) => parse::<LyricsResult>("lyrics", &value).map(Some),
        other => Err(AdapterError::invalid(
            "lyrics",
            format!("expected a lyrics object, got {}", describe(&other)),
        )),
    }
}

fn parse<T: serde::de::DeserializeOwned>(
    function: &str,
    value: &Value,
) -> Result<T, AdapterError> {
    serde_json::from_value(value.clone())
        .map_err(|error| AdapterError::invalid(function, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::polyfills::HostServices;
    use crate::runtime::testing::fixture_source;
    use crate::runtime::RuntimeLimits;
    use std::sync::Arc;

    fn load(name: &str, source: &str) -> ExtensionRuntime {
        ExtensionRuntime::load(name, source, RuntimeLimits::default()).expect("loads")
    }

    /// Load with a host the test keeps a handle to, so forwarded progress events
    /// can be inspected.
    fn load_recording(name: &str, source: &str) -> (ExtensionRuntime, Arc<HostServices>) {
        let host = Arc::new(HostServices::default());
        let runtime = ExtensionRuntime::load_with_host(
            name,
            source,
            RuntimeLimits::default(),
            Arc::clone(&host),
        )
        .expect("loads");

        (runtime, host)
    }

    fn names(capabilities: &Capabilities) -> Vec<&str> {
        capabilities
            .registered()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    #[test]
    fn capabilities_match_every_key_spotify_web_registers() {
        let runtime = load("spotify-web", &fixture_source("spotify-web"));
        let capabilities = Capabilities::from_runtime(&runtime);

        assert_eq!(
            names(&capabilities),
            vec![
                "initialize",
                "cleanup",
                "customSearch",
                "handleUrl",
                "getTrack",
                "getAlbum",
                "getArtist",
                "getPlaylist",
                "searchTracks",
                "enrichTrack",
                "getHomeFeed",
                "getBrowseCategories",
            ]
        );

        assert!(capabilities.has_search());
        assert!(capabilities.has("searchTracks") && capabilities.has("customSearch"));
        assert!(capabilities.is_metadata_provider());
        assert!(capabilities.has_home_feed());
        assert!(!capabilities.is_download_provider());
        assert!(!capabilities.has_availability());
        assert!(!capabilities.has_lyrics());
        assert!(capabilities.unrecognised().is_empty());

        assert_eq!(capabilities.arity("initialize"), Some(1));
        assert_eq!(capabilities.arity("customSearch"), Some(2));
        assert_eq!(capabilities.arity("download"), None);
    }

    #[test]
    fn capabilities_match_every_key_amazon_registers() {
        let runtime = load("amazon", &fixture_source("amazon"));
        let capabilities = Capabilities::from_runtime(&runtime);

        assert_eq!(
            names(&capabilities),
            vec![
                "initialize",
                "completeGrant",
                "handleUrl",
                "getTrack",
                "getAlbum",
                "getPlaylist",
                "getArtist",
                "enrichTrack",
                "getHomeFeed",
                "customSearch",
                "checkAvailability",
                "download",
                "getDownloadUrl",
                "matchTrack",
                "validateTrackForDownload",
                "cleanup",
            ]
        );

        assert!(capabilities.has_search());
        assert!(!capabilities.has("searchTracks"), "amazon ships only customSearch");
        assert!(capabilities.is_metadata_provider());
        assert!(capabilities.is_download_provider());
        assert!(capabilities.has_availability());
        assert!(capabilities.has("matchTrack"));
        assert!(capabilities.unrecognised().is_empty());

        // The shipped 5-argument form, which is the whole reason for dispatch.
        assert_eq!(capabilities.arity("download"), Some(5));
        assert_eq!(capabilities.arity("checkAvailability"), Some(4));
    }

    #[test]
    fn capabilities_match_every_key_tidal_web_registers() {
        let runtime = load("tidal-web", &fixture_source("tidal-web"));
        let capabilities = Capabilities::from_runtime(&runtime);

        assert_eq!(
            names(&capabilities),
            vec![
                "initialize",
                "cleanup",
                "completeGrant",
                "customSearch",
                "checkAvailability",
                "download",
                "handleUrl",
                "getTrack",
                "getAlbum",
                "getArtist",
                "getPlaylist",
                "enrichTrack",
                "searchTracks",
            ]
        );

        assert!(capabilities.has_search());
        assert!(capabilities.is_download_provider());
        assert!(capabilities.has_availability());
        assert!(capabilities.unrecognised().is_empty());
        assert_eq!(capabilities.arity("download"), Some(5));
    }

    #[test]
    fn capabilities_match_every_key_apple_music_registers() {
        let runtime = load("apple-music", &fixture_source("apple-music"));
        let capabilities = Capabilities::from_runtime(&runtime);

        assert_eq!(
            names(&capabilities),
            vec![
                "initialize",
                "cleanup",
                "customSearch",
                "handleUrl",
                "getTrack",
                "getAlbum",
                "getArtist",
                "getConcert",
                "getPlaylist",
                "getHomeFeed",
                "searchTracks",
                "enrichTrack",
                "fetchLyrics",
            ]
        );

        assert!(capabilities.has_search());
        assert!(capabilities.has_lyrics());
        assert!(!capabilities.is_download_provider());
        assert!(!capabilities.has_availability());

        // `getConcert` is a real registered function the host has no route for:
        // it is reported, not dropped, so a missing integration is visible.
        let unknown = capabilities.unrecognised();
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].name, "getConcert");
    }

    #[test]
    fn an_initialize_only_package_reports_no_search_instead_of_failing() {
        let runtime = load(
            "bare",
            "registerExtension({ initialize: function (config) { return true; } });",
        );
        let capabilities = Capabilities::from_runtime(&runtime);

        assert!(capabilities.has("initialize"));
        assert!(!capabilities.has_search(), "no search function was registered");
        assert!(!capabilities.is_metadata_provider());
        assert!(!capabilities.is_download_provider());
        assert!(capabilities.unrecognised().is_empty());

        // Asking anyway is a typed refusal, not an exception from the runtime.
        assert_eq!(
            call(&runtime, "searchTracks", &[]).unwrap_err(),
            AdapterError::Unsupported {
                function: "searchTracks".to_string()
            }
        );
        assert_eq!(
            download(&runtime, &DownloadRequest::new("id", "best", "out.flac")).unwrap_err(),
            AdapterError::Unsupported {
                function: "download".to_string()
            }
        );
    }

    #[test]
    fn functions_the_host_does_not_know_are_reported_with_their_arity() {
        let runtime = load(
            "newer",
            "registerExtension({ initialize: function () { return true; }, getConcertList: function (artist, token) { return []; } });",
        );
        let capabilities = Capabilities::from_runtime(&runtime);

        let unknown = capabilities.unrecognised();
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].name, "getConcertList");
        assert_eq!(unknown[0].arity, 2);
    }

    #[test]
    fn download_arguments_follow_the_declared_arity() {
        let request = DownloadRequest::new("id-1", "best", "out/track.flac")
            .with_isrc("ISRC1")
            .with_names("Song", "Artist")
            .with_duration_ms(213_573)
            .with_prepared_context(json!({ "prepared": true }));

        assert_eq!(download_signature(2), DownloadSignature::RequestObject);
        assert_eq!(download_signature(5), DownloadSignature::Positional);

        assert_eq!(download_arguments(0, &request), Vec::<Value>::new());
        assert_eq!(download_arguments(1, &request), vec![json!("id-1")]);
        assert_eq!(
            download_arguments(3, &request),
            vec![json!("id-1"), json!("best"), json!("out/track.flac")]
        );

        let four = download_arguments(4, &request);
        assert_eq!(four.len(), 4);
        assert_eq!(four[3][CALLBACK_MARKER], json!("progress"));

        let five = download_arguments(5, &request);
        assert_eq!(five.len(), 5);
        assert_eq!(five[4]["preparedContext"], json!({ "prepared": true }));

        // Two parameters is the documented request form.
        let two = download_arguments(2, &request);
        assert_eq!(two.len(), 2);
        assert_eq!(two[0]["track_id"], json!("id-1"));
        assert_eq!(two[0]["quality"], json!("best"));
        assert_eq!(two[0]["output_path"], json!("out/track.flac"));
        assert_eq!(two[0]["isrc"], json!("ISRC1"));
        assert_eq!(two[0]["track_name"], json!("Song"));
        assert_eq!(two[0]["artist_name"], json!("Artist"));
        assert_eq!(two[0]["duration_ms"], json!(213_573));
        assert_eq!(two[0]["prepared_context"], json!({ "prepared": true }));
        assert_eq!(two[1][CALLBACK_MARKER], json!("progress"));
    }

    #[test]
    fn a_five_argument_download_receives_the_positional_shape_and_a_working_callback() {
        let source = r#"
registerExtension({
  download: function (trackID, quality, outputPath, onProgress, options) {
    var seen = {
      trackID: trackID,
      quality: quality,
      outputPath: outputPath,
      progressIsFunction: typeof onProgress === 'function',
      prepared: options && options.preparedContext ? options.preparedContext.token : null,
      hostTrack: options && options.track ? options.track.name : null
    };
    if (typeof onProgress === 'function') onProgress(42, 'halfway');
    return {
      success: true,
      file_path: outputPath,
      format: 'flac',
      bit_depth: 24,
      sample_rate: 96000,
      seen: seen
    };
  }
});
"#;

        let (runtime, host) = load_recording("recording-five", source);
        let request = DownloadRequest::new("B01ASIN", "best", "out/track.flac")
            .with_prepared_context(json!({ "token": "abc" }))
            .with_metadata(json!({ "name": "Host Track" }));

        let outcome = download(&runtime, &request).expect("call");
        let DownloadOutcome::Completed(result) = outcome else {
            panic!("expected success, got {outcome:?}");
        };

        assert_eq!(result.file_path.as_deref(), Some("out/track.flac"));
        assert_eq!(result.format.as_deref(), Some("flac"));
        assert_eq!(result.actual_bit_depth, Some(24));
        assert_eq!(result.actual_sample_rate, Some(96_000));

        let seen = &result.extra["seen"];
        assert_eq!(seen["trackID"], json!("B01ASIN"));
        assert_eq!(seen["quality"], json!("best"));
        assert_eq!(seen["outputPath"], json!("out/track.flac"));
        assert_eq!(seen["progressIsFunction"], json!(true));
        assert_eq!(seen["prepared"], json!("abc"));
        assert_eq!(seen["hostTrack"], json!("Host Track"));

        assert_eq!(
            host.progress_events(),
            vec![json!({ "kind": "progress", "args": [42, "halfway"] })],
            "the callback must reach the host with its arguments intact"
        );
    }

    #[test]
    fn a_two_argument_download_receives_a_request_object_and_still_succeeds() {
        let source = r#"
registerExtension({
  download: function (request, progressCallback) {
    progressCallback(50);
    return {
      success: true,
      file_path: request.output_path,
      seen: {
        id: request.track_id,
        quality: request.quality,
        isrc: request.isrc,
        trackName: request.track_name,
        artistName: request.artist_name,
        durationMs: request.duration_ms,
        prepared: request.prepared_context ? request.prepared_context.token : null
      }
    };
  }
});
"#;

        let (runtime, host) = load_recording("recording-two", source);
        let request = DownloadRequest::new("track-1", "LOSSLESS", "out/song.flac")
            .with_isrc("ISRC9")
            .with_names("Song", "Artist")
            .with_duration_ms(1_000)
            .with_prepared_context(json!({ "token": "z" }));

        let outcome = download(&runtime, &request).expect("call");
        let DownloadOutcome::Completed(result) = outcome else {
            panic!("expected success, got {outcome:?}");
        };

        assert_eq!(result.file_path.as_deref(), Some("out/song.flac"));

        let seen = &result.extra["seen"];
        assert_eq!(seen["id"], json!("track-1"));
        assert_eq!(seen["quality"], json!("LOSSLESS"));
        assert_eq!(seen["isrc"], json!("ISRC9"));
        assert_eq!(seen["trackName"], json!("Song"));
        assert_eq!(seen["artistName"], json!("Artist"));
        assert_eq!(seen["durationMs"], json!(1000));
        assert_eq!(seen["prepared"], json!("z"));

        assert_eq!(
            host.progress_events(),
            vec![json!({ "kind": "progress", "args": [50] })]
        );
    }

    #[test]
    fn every_failure_key_produces_the_same_normalised_error() {
        let mut outcomes = Vec::new();

        for key in ["error", "error_message", "message"] {
            let source = format!(
                "registerExtension({{ download: function (id, quality, path, onProgress, options) {{ return {{ success: false, {key}: 'boom' }}; }} }});"
            );
            let runtime = load("failure-key", &source);
            let outcome = download(&runtime, &DownloadRequest::new("id", "best", "out.flac"))
                .expect("call");
            outcomes.push(outcome);
        }

        let expected = DownloadOutcome::Failed(DownloadFailure {
            kind: DownloadFailureKind::Unknown,
            message: "boom".to_string(),
            retry_after_seconds: None,
            auth_url: None,
        });

        for outcome in &outcomes {
            assert_eq!(outcome, &expected);
        }
        assert_eq!(outcomes[0], outcomes[1]);
        assert_eq!(outcomes[1], outcomes[2]);
        assert!(!expected.is_success());
    }

    #[test]
    fn failures_that_carry_more_detail_survive_normalisation() {
        let source = r#"
registerExtension({
  download: function (id, quality, path, onProgress, options) {
    return {
      success: false,
      error_message: 'slow down',
      error_type: 'rate_limited',
      retry_after_seconds: 30,
      auth_url: ''
    };
  }
});
"#;

        let runtime = load("rate-limited", source);
        let outcome = download(&runtime, &DownloadRequest::new("id", "best", "out.flac"))
            .expect("call");

        assert_eq!(
            outcome,
            DownloadOutcome::Failed(DownloadFailure {
                kind: DownloadFailureKind::RateLimited,
                message: "slow down".to_string(),
                retry_after_seconds: Some(30),
                auth_url: None,
            })
        );

        // A package-specific type is kept verbatim rather than mapped to a lie.
        let source = r#"
registerExtension({
  download: function (id, quality, path, onProgress, options) {
    return { success: false, error: { code: 'E_PROVIDER' }, error_type: 'provider_specific' };
  }
});
"#;
        let runtime = load("provider-specific", source);
        let outcome = download(&runtime, &DownloadRequest::new("id", "best", "out.flac"))
            .expect("call");

        let DownloadOutcome::Failed(failure) = outcome else {
            panic!("expected failure");
        };
        assert_eq!(failure.kind, DownloadFailureKind::Other("provider_specific".to_string()));
        assert_eq!(failure.kind.as_str(), "provider_specific");
        assert!(
            failure.message.contains("E_PROVIDER"),
            "an object error must still produce readable text, got {:?}",
            failure.message
        );
    }

    #[test]
    fn a_download_that_returns_nothing_is_a_failure_with_a_reason() {
        let source = "registerExtension({ download: function (id, quality, path, onProgress, options) { return null; } });";
        let runtime = load("returns-null", source);

        let outcome = download(&runtime, &DownloadRequest::new("id", "best", "out.flac"))
            .expect("call");

        let DownloadOutcome::Failed(failure) = outcome else {
            panic!("null must not be reported as success");
        };
        assert_eq!(failure.kind, DownloadFailureKind::DownloadError);
        assert!(failure.message.contains("null"), "got {:?}", failure.message);

        let source = "registerExtension({ download: function (id, quality, path, onProgress, options) { return { success: false }; } });";
        let runtime = load("silent-failure", source);
        let outcome = download(&runtime, &DownloadRequest::new("id", "best", "out.flac"))
            .expect("call");

        assert_eq!(
            outcome,
            DownloadOutcome::Failed(DownloadFailure {
                kind: DownloadFailureKind::Unknown,
                message: "download provider reported failure without a reason".to_string(),
                retry_after_seconds: None,
                auth_url: None,
            })
        );
    }

    #[test]
    fn track_parsing_coerces_the_shapes_packages_actually_send() {
        let value = json!({
            "id": "4uLU6hMCjMI75M1A2tKUQC",
            "name": "Never Gonna Give You Up",
            "artists": "Rick Astley",
            "album_name": "Whenever You Need Somebody",
            "duration_ms": "213573",
            "images": "https://i.scdn.co/image/ab67616d0000b2739",
            "item_type": "track",
            "provider_id": "spotify-web"
        });

        let track = track_from_value(&value).expect("parses");
        assert_eq!(track.artist_names(), "Rick Astley");
        assert_eq!(
            track.images.0,
            ["https://i.scdn.co/image/ab67616d0000b2739".to_string()]
        );
        assert_eq!(track.duration_ms, 213_573);
        assert_eq!(
            track.artwork_url(),
            Some("https://i.scdn.co/image/ab67616d0000b2739")
        );
        assert_eq!(track.extra["provider_id"], json!("spotify-web"));

        let list = tracks_from_value(&json!([
            { "id": "a", "name": "One", "artists": ["X", "Y"] },
            { "id": "b", "name": "Two", "images": ["u1", "u2"] }
        ]))
        .expect("parses");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].artist_names(), "X, Y");
        assert_eq!(list[1].images.0, vec!["u1".to_string(), "u2".to_string()]);

        let error = tracks_from_value(&json!([{ "id": "a" }])).unwrap_err();
        let AdapterError::Invalid { function, reason } = error else {
            panic!("expected a typed parse error");
        };
        assert_eq!(function, "tracks");
        assert!(reason.contains("entry 0"), "got {reason:?}");
        assert!(reason.contains("name"), "got {reason:?}");
    }

    #[test]
    fn album_and_artist_wrappers_are_unwrapped() {
        let wrapped = json!({
            "type": "album",
            "album_info": { "id": "album-1", "name": "Album" },
            "track_list": [
                { "id": "t1", "name": "One" },
                { "id": "t2", "name": "Two" }
            ]
        });

        let album = album_from_value(&wrapped).expect("parses");
        assert_eq!(album.id, "album-1");
        assert_eq!(album.name, "Album");
        assert_eq!(album.tracks.len(), 2);
        assert_eq!(album.tracks[1].name, "Two");

        let artist = artist_from_value(&json!({
            "type": "artist",
            "artist": { "id": "artist-1", "name": "Artist" }
        }))
        .expect("parses");
        assert_eq!(artist.id, "artist-1");
        assert_eq!(artist.name, "Artist");
    }

    #[test]
    fn lyrics_parsing_normalises_times_and_absent_content() {
        let synced = lyrics_from_value(json!({
            "syncType": "LINE_SYNCED",
            "lines": [
                { "startTimeMs": "1000", "words": "hello" },
                { "startTimeMs": 2500, "endTimeMs": "4000", "words": "world" }
            ],
            "provider": "fixture"
        }))
        .expect("parses")
        .expect("has lyrics");

        assert!(synced.is_synced());
        assert!(synced.has_content());
        assert_eq!(synced.lines[0].start_time_ms, 1_000);
        assert_eq!(synced.lines[1].end_time_ms, 4_000);

        let unsynced = lyrics_from_value(json!({
            "syncType": "UNSYNCED",
            "plainLyrics": "la la"
        }))
        .expect("parses")
        .expect("has lyrics");
        assert!(!unsynced.is_synced());
        assert!(unsynced.has_content());

        let instrumental = lyrics_from_value(json!({ "instrumental": true }))
            .expect("parses")
            .expect("has lyrics");
        assert!(instrumental.has_content());
        assert!(instrumental.lines.is_empty());

        assert_eq!(lyrics_from_value(Value::Null).expect("null is not an error"), None);
    }

    #[test]
    fn from_hint_maps_documented_error_codes_to_typed_variants() {
        assert_eq!(DownloadFailureKind::from_hint(Some("geo_blocked")), DownloadFailureKind::GeoBlocked);
        assert_eq!(DownloadFailureKind::from_hint(Some("stream_error")), DownloadFailureKind::StreamError);
        assert_eq!(DownloadFailureKind::from_hint(Some("format_error")), DownloadFailureKind::FormatError);
        assert_eq!(DownloadFailureKind::from_hint(Some("quota_exceeded")), DownloadFailureKind::QuotaExceeded);
        assert_eq!(DownloadFailureKind::from_hint(Some("premium_required")), DownloadFailureKind::PremiumRequired);
        assert_eq!(DownloadFailureKind::from_hint(Some("server_error")), DownloadFailureKind::ServerError);
        for code in ["geo_blocked", "stream_error", "format_error", "quota_exceeded", "premium_required", "server_error"] {
            assert_eq!(DownloadFailureKind::from_hint(Some(code)).as_str(), code);
        }
    }
}

