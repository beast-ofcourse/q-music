//! `manifest.json` schema, defaults and validation.
//!
//! The schema is derived from manifests that actually ship, not from the
//! development guide. Every field below appears in at least one of
//! `spotify-web`, `amazon` or `tidal-web`; see `fixtures/sources/`.
//!
//! Two rules drive the design:
//!
//! 1. **Unknown fields are kept.** Packages evolve faster than hosts. A new
//!    manifest key must degrade to "ignored", never to "install refused".
//! 2. **Enum-like strings stay strings here.** `type` and setting `type` are
//!    validated by helper methods rather than deserialised into closed enums,
//!    so a future provider kind is preserved and reported instead of dropped.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

/// Used when a manifest omits `capabilities.networkTimeoutSeconds`.
pub const DEFAULT_NETWORK_TIMEOUT_SECONDS: u64 = 15;

/// Provider kind strings understood by Mornye hosts.
pub const PROVIDER_METADATA: &str = "metadata_provider";
pub const PROVIDER_DOWNLOAD: &str = "download_provider";
pub const PROVIDER_LYRICS: &str = "lyrics_provider";

/// Setting kind strings understood by Mornye hosts.
pub const SETTING_STRING: &str = "string";
pub const SETTING_NUMBER: &str = "number";
pub const SETTING_BOOLEAN: &str = "boolean";
pub const SETTING_SELECT: &str = "select";
pub const SETTING_BUTTON: &str = "button";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest is not valid JSON: {0}")]
    Json(String),
    #[error("manifest field `{field}` is required but missing or empty")]
    MissingField { field: &'static str },
    #[error("manifest field `name` contains a path separator or control character: {value:?}")]
    InvalidName { value: String },
    #[error("manifest `permissions.network` entry is empty")]
    EmptyNetworkEntry,
}

/// A Mornye extension manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: String,

    #[serde(default)]
    pub display_name: Option<String>,

    pub version: String,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub homepage: Option<String>,

    #[serde(default)]
    pub icon: Option<String>,

    #[serde(default)]
    pub author: Option<String>,

    /// Provider kinds. Serialised as `type`.
    #[serde(rename = "type", default)]
    pub kinds: Vec<String>,

    #[serde(default)]
    pub min_app_version: Option<String>,

    #[serde(default)]
    pub skip_metadata_enrichment: bool,

    #[serde(default, rename = "skipBuiltInFallback")]
    pub skip_built_in_fallback: bool,

    /// Host capabilities the package needs, e.g. `signedSession@3`.
    #[serde(default)]
    pub required_runtime_features: Vec<String>,

    #[serde(default)]
    pub signed_session: Option<SignedSession>,

    #[serde(default)]
    pub permissions: Permissions,

    #[serde(default)]
    pub quality_options: Vec<QualityOption>,

    #[serde(default)]
    pub capabilities: Capabilities,

    #[serde(default)]
    pub service_health: Vec<ServiceHealth>,

    #[serde(default)]
    pub search_behavior: Option<SearchBehavior>,

    #[serde(default)]
    pub url_handler: Option<UrlHandler>,

    #[serde(default)]
    pub settings: Vec<Setting>,

    /// Present in newer packages. Declares which file transformations the
    /// package can run after a download and before the host embeds metadata.
    #[serde(default)]
    pub post_processing: Option<PostProcessing>,

    /// As above: carried verbatim until a real package defines the shape.
    #[serde(default)]
    pub track_matching: Option<Value>,

    /// Unknown keys survive a parse/serialise round trip.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub storage: bool,
    #[serde(default)]
    pub file: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    #[serde(default)]
    pub home_feed: bool,
    #[serde(default)]
    pub home_feed_region_setting: Option<String>,
    #[serde(default)]
    pub browse_categories: bool,
    #[serde(default)]
    pub share_url_templates: BTreeMap<String, String>,
    #[serde(default)]
    pub network_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub download_fallback_tier: Option<String>,
    #[serde(default)]
    pub requires_native_container_conversion: bool,
    #[serde(default)]
    pub download_transfer: Option<DownloadTransfer>,
    #[serde(default)]
    pub replaces_built_in_providers: Vec<String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTransfer {
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub initial_retry_delay_ms: Option<u64>,
    #[serde(default)]
    pub max_retry_delay_ms: Option<u64>,
    #[serde(default)]
    pub resume_policy: Option<String>,
    #[serde(default)]
    pub persistent_checkpoint: bool,
    #[serde(default)]
    pub refresh_stream_on_status: Vec<u16>,
    #[serde(default)]
    pub max_parallel_segments: Option<u16>,
    #[serde(default)]
    pub max_concurrent_downloads: Option<u16>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// `postProcessing` from the manifest: what a package can do to a file after
/// it has been downloaded and before the host embeds metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostProcessing {
    /// A package that ships hooks but disables them here is not asked to run.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub hooks: Vec<PostProcessHook>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

impl PostProcessing {
    /// True when the host should offer this package's hooks at all.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.hooks.is_empty()
    }

    pub fn hook(&self, id: &str) -> Option<&PostProcessHook> {
        self.hooks.iter().find(|hook| hook.id == id)
    }

    /// Hooks enabled by default that accept `extension` (lowercase, no dot).
    ///
    /// An empty `supportedFormats` means "every format": a hook that declares
    /// no restriction should run rather than silently never run.
    pub fn hooks_for(&self, extension: &str) -> Vec<&PostProcessHook> {
        if !self.enabled {
            return Vec::new();
        }

        self.hooks
            .iter()
            .filter(|hook| hook.default_enabled && hook.supports(extension))
            .collect()
    }
}

/// One `postProcessing.hooks[]` entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostProcessHook {
    /// Identifier passed to `postProcess(filePath, metadata, hookId)`.
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_enabled: bool,
    /// Lowercase extensions without a dot, e.g. `["flac", "mp3"]`.
    #[serde(default)]
    pub supported_formats: Vec<String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

impl PostProcessHook {
    pub fn supports(&self, extension: &str) -> bool {
        self.supported_formats.is_empty()
            || self
                .supported_formats
                .iter()
                .any(|format| format.trim().trim_start_matches('.').eq_ignore_ascii_case(extension))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedSession {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub scheme_label: Option<String>,
    #[serde(default)]
    pub header_prefix: Option<String>,
    #[serde(default)]
    pub time_window_seconds: Option<u64>,
    #[serde(default)]
    pub endpoints: BTreeMap<String, String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityOption {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Per-quality setting overrides, applied on top of the global settings.
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealth {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub service_key: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub cache_ttl_seconds: Option<u64>,
    #[serde(default)]
    pub required: bool,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBehavior {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub thumbnail_ratio: Option<String>,
    #[serde(default)]
    pub thumbnail_width: Option<u32>,
    #[serde(default)]
    pub thumbnail_height: Option<u32>,
    #[serde(default)]
    pub filters: Vec<SearchFilter>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilter {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlHandler {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// One entry of `settings[]`.
///
/// `default` is arbitrary JSON: a string for `string`/`select`, a number for
/// `number`, a bool for `boolean`, absent for `button`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(rename = "type")]
    pub setting_type: String,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub options: Vec<SettingOption>,
    /// Action name invoked for `button` settings.
    #[serde(default)]
    pub action: Option<String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// A select option: either a bare string or a `{ value, label }` pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingOption {
    Value(String),
    Labelled { value: String, label: String },
}

impl SettingOption {
    pub fn value(&self) -> &str {
        match self {
            SettingOption::Value(value) => value,
            SettingOption::Labelled { value, .. } => value,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            SettingOption::Value(value) => value,
            SettingOption::Labelled { label, .. } => label,
        }
    }
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Self, ManifestError> {
        let manifest: Manifest =
            serde_json::from_str(json).map_err(|err| ManifestError::Json(err.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Field presence and filesystem safety. The `name` becomes a directory
    /// name on disk, so it is checked here rather than at every use site.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError::MissingField { field: "name" });
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::MissingField { field: "version" });
        }
        if !is_safe_identifier(&self.name) {
            return Err(ManifestError::InvalidName {
                value: self.name.clone(),
            });
        }
        if self.permissions.network.iter().any(|host| host.trim().is_empty()) {
            return Err(ManifestError::EmptyNetworkEntry);
        }
        Ok(())
    }

    pub fn display_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    pub fn network_timeout_seconds(&self) -> u64 {
        self.capabilities
            .network_timeout_seconds
            .unwrap_or(DEFAULT_NETWORK_TIMEOUT_SECONDS)
    }

    pub fn is_metadata_provider(&self) -> bool {
        self.kinds.iter().any(|kind| kind == PROVIDER_METADATA)
    }

    pub fn is_download_provider(&self) -> bool {
        self.kinds.iter().any(|kind| kind == PROVIDER_DOWNLOAD)
    }

    pub fn is_lyrics_provider(&self) -> bool {
        self.kinds.iter().any(|kind| kind == PROVIDER_LYRICS)
    }

    /// Provider kinds this host does not recognise. Reported, never fatal.
    pub fn unknown_kinds(&self) -> Vec<&str> {
        self.kinds
            .iter()
            .map(String::as_str)
            .filter(|kind| {
                !matches!(
                    *kind,
                    PROVIDER_METADATA | PROVIDER_DOWNLOAD | PROVIDER_LYRICS
                )
            })
            .collect()
    }

    /// Settings defaults as a flat JSON object, ready to hand to `initialize`.
    pub fn default_settings(&self) -> serde_json::Map<String, Value> {
        let mut values = serde_json::Map::new();
        for setting in &self.settings {
            if let Some(default) = &setting.default {
                values.insert(setting.key.clone(), default.clone());
            }
        }
        values
    }
}

impl Setting {
    pub fn is_known_type(&self) -> bool {
        matches!(
            self.setting_type.as_str(),
            SETTING_STRING | SETTING_NUMBER | SETTING_BOOLEAN | SETTING_SELECT | SETTING_BUTTON
        )
    }
}

/// Directory-safe extension identifier: no separators, no traversal, no
/// control characters, and not a reserved Windows device name.
fn is_safe_identifier(name: &str) -> bool {
    if name.chars().any(|ch| {
        ch.is_control() || ch == '/' || ch == '\\' || ch == ':' || ch == '*' || ch == '?'
    }) {
        return false;
    }
    if name.contains("..") {
        return false;
    }
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    !RESERVED.contains(&stem.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY: &str = include_str!("../fixtures/sources/spotify-web.manifest.json");
    const AMAZON: &str = include_str!("../fixtures/sources/amazon.manifest.json");
    const TIDAL: &str = include_str!("../fixtures/sources/tidal-web.manifest.json");

    #[test]
    fn parses_spotify_web_fixture() {
        let manifest = Manifest::parse(SPOTIFY).expect("spotify-web manifest parses");

        assert_eq!(manifest.name, "spotify-web");
        assert_eq!(manifest.display_name(), "Spotify Web");
        assert_eq!(manifest.version, "1.10.4");
        assert_eq!(manifest.min_app_version.as_deref(), Some("4.2.3"));
        assert!(manifest.is_metadata_provider());
        assert!(!manifest.is_download_provider());
        assert!(!manifest.is_lyrics_provider());
        assert!(manifest.capabilities.home_feed);
        assert!(manifest.capabilities.browse_categories);
        assert_eq!(manifest.network_timeout_seconds(), 15);
        assert_eq!(manifest.permissions.network.len(), 7);
        assert!(manifest.permissions.storage);
        assert!(!manifest.permissions.file);
        assert!(manifest.quality_options.is_empty());
        assert!(manifest.settings.is_empty());
    }

    #[test]
    fn parses_amazon_fixture() {
        let manifest = Manifest::parse(AMAZON).expect("amazon manifest parses");

        assert_eq!(manifest.name, "amazon");
        assert_eq!(manifest.version, "2.3.14");
        assert!(manifest.is_metadata_provider());
        assert!(manifest.is_download_provider());
        assert_eq!(manifest.quality_options.len(), 4);
        assert_eq!(manifest.quality_options[0].id, "best");
        assert_eq!(manifest.quality_options[0].kind.as_deref(), Some("lossless"));
        assert_eq!(
            manifest.capabilities.download_fallback_tier.as_deref(),
            Some("hi_res")
        );
        assert_eq!(manifest.settings.len(), 1);
    }

    #[test]
    fn parses_skip_built_in_fallback_when_present() {
        let json = r#"{
            "name": "x",
            "displayName": "X",
            "version": "1.0.0",
            "description": "d",
            "author": "a",
            "permissions": { "network": [] },
            "type": ["metadata_provider"],
            "skipBuiltInFallback": true
        }"#;
        let manifest = Manifest::parse(json).expect("parses");
        assert!(manifest.skip_built_in_fallback);
        // Default is false when absent.
        let json2 = json.replace(",\n            \"skipBuiltInFallback\": true", "");
        let manifest2 = Manifest::parse(&json2).expect("parses");
        assert!(!manifest2.skip_built_in_fallback);
    }

    #[test]
    fn parses_tidal_fixture_including_download_transfer() {
        let manifest = Manifest::parse(TIDAL).expect("tidal-web manifest parses");

        assert_eq!(manifest.name, "tidal-web");
        assert_eq!(
            manifest.required_runtime_features,
            vec!["signedSession@3", "sessionGrant@1", "downloadSegments@1"]
        );

        let session = manifest.signed_session.as_ref().expect("signedSession present");
        assert_eq!(session.namespace.as_deref(), Some("zarz-v2"));
        assert_eq!(session.time_window_seconds, Some(300));
        assert_eq!(
            session.endpoints.get("exchange").map(String::as_str),
            Some("/session/exchange")
        );

        let transfer = manifest
            .capabilities
            .download_transfer
            .as_ref()
            .expect("downloadTransfer present");
        assert_eq!(transfer.max_attempts, Some(4));
        assert_eq!(transfer.max_parallel_segments, Some(4));
        assert_eq!(transfer.refresh_stream_on_status, vec![401, 403]);
        assert!(transfer.persistent_checkpoint);

        assert_eq!(manifest.service_health.len(), 1);
        assert!(manifest.service_health[0].required);
        assert_eq!(
            manifest.service_health[0].url.as_deref(),
            Some("https://api.zarz.moe/v1/health")
        );

        assert_eq!(manifest.quality_options.len(), 5);
        assert_eq!(
            manifest.capabilities.replaces_built_in_providers,
            vec!["tidal"]
        );
    }

    #[test]
    fn rejects_missing_name() {
        let json = r#"{ "version": "1.0.0" }"#;
        let error = Manifest::parse(json).expect_err("missing name is rejected");
        assert!(
            error.to_string().contains("name"),
            "error must name the field, got: {error}"
        );
    }

    #[test]
    fn rejects_empty_name() {
        let json = r#"{ "name": "   ", "version": "1.0.0" }"#;
        assert_eq!(
            Manifest::parse(json),
            Err(ManifestError::MissingField { field: "name" })
        );
    }

    #[test]
    fn rejects_missing_version() {
        let json = r#"{ "name": "free-music-api" }"#;
        let error = Manifest::parse(json).expect_err("missing version is rejected");
        assert!(
            error.to_string().contains("version"),
            "error must name the field, got: {error}"
        );
    }

    #[test]
    fn rejects_path_traversal_in_name() {
        for name in ["../evil", "a/b", "a\\b", "C:evil", "con", ".."] {
            let json = format!(r#"{{ "name": "{name}", "version": "1.0.0" }}"#);
            let result = Manifest::parse(&json);
            assert!(result.is_err(), "expected rejection for name {name:?}");
        }
    }

    #[test]
    fn unknown_top_level_field_round_trips() {
        let json = r#"{
            "name": "future-ext",
            "version": "2.0.0",
            "type": ["metadata_provider"],
            "brandNewFeature": { "nested": [1, 2, 3] }
        }"#;

        let manifest = Manifest::parse(json).expect("unknown field is tolerated");
        assert_eq!(manifest.extra.len(), 1);

        let reserialised = serde_json::to_string(&manifest).expect("serialises");
        assert!(reserialised.contains("brandNewFeature"));
        assert!(reserialised.contains(r#""nested":[1,2,3]"#));

        let reparsed = Manifest::parse(&reserialised).expect("round trip parses");
        assert_eq!(reparsed, manifest);
    }

    #[test]
    fn unknown_provider_kind_is_preserved_and_reported() {
        let json = r#"{
            "name": "wave-provider",
            "version": "1.0.0",
            "type": ["metadata_provider", "hologram_provider"]
        }"#;

        let manifest = Manifest::parse(json).expect("unknown kind is tolerated");
        assert!(manifest.is_metadata_provider());
        assert_eq!(manifest.unknown_kinds(), vec!["hologram_provider"]);
        assert!(serde_json::to_string(&manifest)
            .expect("serialises")
            .contains("hologram_provider"));
    }

    #[test]
    fn parses_every_setting_type() {
        let json = r#"{
            "name": "settings-ext",
            "version": "1.0.0",
            "type": ["metadata_provider"],
            "settings": [
                { "key": "token", "label": "Token", "type": "string", "default": "abc" },
                { "key": "limit", "label": "Limit", "type": "number", "default": 25 },
                { "key": "enabled", "label": "Enabled", "type": "boolean", "default": true },
                { "key": "region", "label": "Region", "type": "select", "default": "US",
                  "options": ["US", "GB"] },
                { "key": "login", "label": "Log in", "type": "button", "action": "startLogin" }
            ]
        }"#;

        let manifest = Manifest::parse(json).expect("settings manifest parses");
        let kinds: Vec<&str> = manifest
            .settings
            .iter()
            .map(|setting| setting.setting_type.as_str())
            .collect();
        assert_eq!(kinds, vec!["string", "number", "boolean", "select", "button"]);
        assert!(manifest.settings.iter().all(Setting::is_known_type));

        let region = &manifest.settings[3];
        assert_eq!(region.options.len(), 2);
        assert_eq!(region.options[1].value(), "GB");
        assert_eq!(region.options[1].label(), "GB");

        let login = &manifest.settings[4];
        assert_eq!(login.action.as_deref(), Some("startLogin"));

        let defaults = manifest.default_settings();
        assert_eq!(defaults.get("limit"), Some(&serde_json::json!(25)));
        assert_eq!(defaults.get("enabled"), Some(&serde_json::json!(true)));
        assert_eq!(defaults.get("token"), Some(&serde_json::json!("abc")));
    }

    #[test]
    fn select_options_accept_labelled_pairs() {
        let json = r#"{
            "name": "labelled-ext",
            "version": "1.0.0",
            "settings": [
                { "key": "quality", "type": "select", "default": "a",
                  "options": [{ "value": "a", "label": "Alpha" }] }
            ]
        }"#;

        let manifest = Manifest::parse(json).expect("labelled options parse");
        let option = &manifest.settings[0].options[0];
        assert_eq!(option.value(), "a");
        assert_eq!(option.label(), "Alpha");
    }

    #[test]
    fn unknown_setting_type_is_not_claimed_as_known() {
        let json = r#"{
            "name": "odd-ext",
            "version": "1.0.0",
            "settings": [{ "key": "x", "type": "colour-picker" }]
        }"#;

        let manifest = Manifest::parse(json).expect("unknown setting type is tolerated");
        assert!(!manifest.settings[0].is_known_type());
        assert_eq!(manifest.settings[0].setting_type, "colour-picker");
    }

    #[test]
    fn defaults_apply_when_capabilities_absent() {
        let json = r#"{ "name": "bare", "version": "1.0.0" }"#;
        let manifest = Manifest::parse(json).expect("minimal manifest parses");

        assert_eq!(manifest.network_timeout_seconds(), 15);
        assert!(manifest.permissions.network.is_empty());
        assert!(!manifest.permissions.storage);
        assert!(!manifest.permissions.file);
        assert!(!manifest.capabilities.home_feed);
        assert!(manifest.capabilities.share_url_templates.is_empty());
        assert!(manifest.kinds.is_empty());
    }
}
