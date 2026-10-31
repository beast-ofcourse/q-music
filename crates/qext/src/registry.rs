//! The extension registry: what the platform publishes, and what this host can
//! actually run.
//!
//! The registry document is untrusted input. Every entry keeps the fields it
//! published, and the host adds one thing the document cannot know: whether it
//! is usable here, with a reason when it is not. An entry is never dropped
//! because of a version string this host cannot parse — a user who sees
//! "Bugs Music: version `2.0.4-ko` is not a version this host understands" can
//! report it, and a silently shortened list cannot be diagnosed at all.
//!
//! Fetching is separated from parsing so the store can refresh in the
//! background and keep showing the last good list when the network is down. The
//! cache is only replaced on a fully parsed document; a transport failure leaves
//! the previous registry exactly as it was.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::http::{HostHttp, HttpError};

/// Schema version this client understands. A newer document is still parsed;
/// unknown keys survive in `extra`, so a forward-compatible registry does not
/// break an older host.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("the extension registry could not be fetched: {message}")]
    Network { message: String },

    #[error("the extension registry answered {status} for {url}")]
    Status { status: u16, url: String },

    #[error("the extension registry is not a usable document: {message}")]
    InvalidDocument { message: String },
}

impl From<HttpError> for RegistryError {
    fn from(error: HttpError) -> Self {
        Self::Network {
            message: error.to_string(),
        }
    }
}

/// A `major.minor.patch` version with an optional prerelease tag.
///
/// Build metadata (`+sha`) is accepted and ignored: it does not participate in
/// ordering, and refusing a version because a publisher stamped one on would be
/// a compatibility bug rather than a safety feature.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Option<String>,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            prerelease: None,
        }
    }

    /// Parse strictly enough to be predictable: three numeric parts, then an
    /// optional `-prerelease`, with any `+build` ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let without_build = text.split('+').next()?;

        let (core, prerelease) = match without_build.split_once('-') {
            Some((core, tag)) => {
                if tag.is_empty() || !tag.chars().all(valid_prerelease_character) {
                    return None;
                }
                (core, Some(tag.to_string()))
            }
            None => (without_build, None),
        };

        let mut parts = core.split('.');
        let major = parse_number(parts.next()?)?;
        let minor = parse_number(parts.next()?)?;
        let patch = parse_number(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }

        Some(Self {
            major,
            minor,
            patch,
            prerelease,
        })
    }
}

fn valid_prerelease_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '.' || character == '-'
}

fn parse_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(prerelease) = &self.prerelease {
            write!(formatter, "-{prerelease}")?;
        }
        Ok(())
    }
}

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            // Semver: a prerelease sorts before the release it leads to.
            .then_with(|| match (&self.prerelease, &other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => compare_prerelease(left, right),
            })
    }
}

fn compare_prerelease(left: &str, right: &str) -> Ordering {
    let mut left_parts = left.split('.');
    let mut right_parts = right.split('.');

    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return Ordering::Equal,
            // A shorter prerelease set has lower precedence when every shared
            // identifier matched: `1.0.0-alpha` < `1.0.0-alpha.1`.
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => {
                let ordering = compare_identifier(left, right);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

fn compare_identifier(left: &str, right: &str) -> Ordering {
    match (left.parse::<u64>(), right.parse::<u64>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        // Numeric identifiers always have lower precedence than alphanumeric.
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => left.cmp(right),
    }
}

/// Why an entry cannot be installed on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum Unsupported {
    /// The entry needs a newer application than this one.
    HostTooOld { required: Version, host: Version },
    /// A version field is present but not a version this host can compare.
    InvalidVersion { field: String, value: String },
    /// The published digest is not a SHA-256 hex string, so the download could
    /// never be verified.
    InvalidChecksum { value: String },
}

impl Unsupported {
    /// Stable identifier for the UI and for logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::HostTooOld { .. } => "host_too_old",
            Self::InvalidVersion { .. } => "invalid_version",
            Self::InvalidChecksum { .. } => "invalid_checksum",
        }
    }
}

/// Whether this host can install an entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum EntryStatus {
    /// No status has been computed. Only reachable on a hand-built entry.
    #[default]
    NotEvaluated,
    Supported,
    Unsupported(Unsupported),
}

impl EntryStatus {
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }

    /// Stable identifier for the UI and for logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotEvaluated => "not_evaluated",
            Self::Supported => "supported",
            Self::Unsupported(reason) => reason.code(),
        }
    }
}

/// One published extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RegistryEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, alias = "downloadUrl")]
    pub download_url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default, alias = "iconUrl")]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default, alias = "minAppVersion")]
    pub min_app_version: Option<String>,

    /// Filled in by [`Registry::parse`]; never read from the document.
    #[serde(skip_deserializing, default)]
    pub status: EntryStatus,

    /// Keys this host does not model, kept so a newer registry round-trips.
    #[serde(flatten, default)]
    pub extra: std::collections::BTreeMap<String, Value>,
}

impl RegistryEntry {
    /// The name to show: the publisher's display name, else the id.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }

    /// The entry's own version, when it parses.
    pub fn parsed_version(&self) -> Option<Version> {
        Version::parse(&self.version)
    }

    /// What installing this entry would do, given the version already installed.
    pub fn offer_for(&self, installed: Option<&Version>) -> InstallOffer {
        match &self.status {
            EntryStatus::Supported => {}
            EntryStatus::Unsupported(reason) => {
                return InstallOffer::Refused {
                    reason: reason.clone(),
                }
            }
            EntryStatus::NotEvaluated => {
                return InstallOffer::Refused {
                    reason: Unsupported::InvalidVersion {
                        field: "status".to_string(),
                        value: "not evaluated".to_string(),
                    },
                }
            }
        }

        // `parse` only marks an entry supported after its own version parsed, so
        // this cannot fail for a supported entry.
        let Some(offered) = self.parsed_version() else {
            return InstallOffer::Refused {
                reason: Unsupported::InvalidVersion {
                    field: "version".to_string(),
                    value: self.version.clone(),
                },
            };
        };

        match installed {
            None => InstallOffer::Fresh,
            Some(installed) if offered > *installed => InstallOffer::Update {
                from: installed.clone(),
                to: offered,
            },
            Some(installed) if offered == *installed => InstallOffer::UpToDate {
                at: offered,
            },
            Some(installed) => InstallOffer::Downgrade {
                from: installed.clone(),
                to: offered,
            },
        }
    }
}

/// What a store may do with a published entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "offer")]
pub enum InstallOffer {
    Fresh,
    Update { from: Version, to: Version },
    UpToDate { at: Version },
    Downgrade { from: Version, to: Version },
    Refused { reason: Unsupported },
}

impl InstallOffer {
    /// Only a fresh install and a strictly newer version may be installed.
    /// An equal version is not an update, and a lower one is a downgrade.
    pub fn is_installable(&self) -> bool {
        matches!(self, Self::Fresh | Self::Update { .. })
    }
}

/// A parsed registry document, with every entry classified for one host version.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Registry {
    pub schema_version: u32,
    pub updated_at: Option<String>,
    pub entries: Vec<RegistryEntry>,
    /// True when the document declares a schema this client does not know.
    pub schema_is_newer: bool,
}

#[derive(Deserialize)]
struct RegistryDocument {
    #[serde(default = "default_schema_version", alias = "schemaVersion")]
    version: u32,
    #[serde(default, alias = "updatedAt")]
    updated_at: Option<String>,
    #[serde(default)]
    extensions: Vec<RegistryEntry>,
}

fn default_schema_version() -> u32 {
    SUPPORTED_SCHEMA_VERSION
}

impl Registry {
    /// Parse a document and classify every entry against `host`.
    pub fn parse(json: &str, host: &Version) -> Result<Self, RegistryError> {
        // A registry served by an editor or a CDN sometimes carries a UTF-8 BOM.
        // It is an encoding marker, not content, and rejecting the whole list
        // over three bytes would be a needless outage.
        let json = json.trim_start_matches('\u{feff}');

        let document: RegistryDocument =
            serde_json::from_str(json).map_err(|error| RegistryError::InvalidDocument {
                message: error.to_string(),
            })?;

        let mut entries = document.extensions;
        for entry in &mut entries {
            entry.status = evaluate(entry, host);
        }

        Ok(Self {
            schema_version: document.version,
            updated_at: document.updated_at,
            entries,
            schema_is_newer: document.version > SUPPORTED_SCHEMA_VERSION,
        })
    }

    pub fn entry(&self, id: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn supported(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.status.is_supported())
    }

    pub fn unsupported(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.entries
            .iter()
            .filter(|entry| !entry.status.is_supported())
    }
}

/// Decide one entry's status. Version problems are reported before host
/// compatibility, because "this string is not a version" is a different fix for
/// the publisher than "this host is too old".
fn evaluate(entry: &RegistryEntry, host: &Version) -> EntryStatus {
    if Version::parse(&entry.version).is_none() {
        return EntryStatus::Unsupported(Unsupported::InvalidVersion {
            field: "version".to_string(),
            value: entry.version.clone(),
        });
    }

    let required = match entry.min_app_version.as_deref() {
        None => None,
        Some(text) => match Version::parse(text) {
            Some(version) => Some(version),
            None => {
                return EntryStatus::Unsupported(Unsupported::InvalidVersion {
                    field: "min_app_version".to_string(),
                    value: text.to_string(),
                })
            }
        },
    };

    if !is_sha256_hex(&entry.sha256) {
        return EntryStatus::Unsupported(Unsupported::InvalidChecksum {
            value: entry.sha256.clone(),
        });
    }

    if let Some(required) = required {
        if *host < required {
            return EntryStatus::Unsupported(Unsupported::HostTooOld {
                required,
                host: host.clone(),
            });
        }
    }

    EntryStatus::Supported
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Fetches the registry and remembers the last document that parsed.
///
/// A failed refresh is an error the caller reports, not a state change: the
/// cached document is left untouched so the extension list keeps working while
/// the network does not.
pub struct RegistryClient {
    url: String,
    http: HostHttp,
    host_version: Version,
    user_agent: String,
    cached: Mutex<Option<Registry>>,
}

impl fmt::Debug for RegistryClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistryClient")
            .field("url", &self.url)
            .field("host_version", &self.host_version)
            .finish()
    }
}

impl RegistryClient {
    pub fn new(url: impl Into<String>, http: HostHttp, host_version: Version) -> Self {
        let user_agent = format!("Q-Music/{host_version}");
        Self {
            url: url.into(),
            http,
            host_version,
            user_agent,
            cached: Mutex::new(None),
        }
    }

    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn host_version(&self) -> &Version {
        &self.host_version
    }

    /// The last registry that parsed, if any.
    pub fn cached(&self) -> Option<Registry> {
        self.cached
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Fetch, parse and cache. On any failure the cache is unchanged.
    pub fn fetch(&self) -> Result<Registry, RegistryError> {
        let body = self.download()?;
        let registry = Registry::parse(&body, &self.host_version)?;

        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *cached = Some(registry.clone());

        Ok(registry)
    }

    fn download(&self) -> Result<String, RegistryError> {
        let headers = [
            ("accept".to_string(), "application/json".to_string()),
            ("user-agent".to_string(), self.user_agent.clone()),
        ];

        let response = self.http.get(&self.url, &headers)?;

        if !response.ok {
            return Err(RegistryError::Status {
                status: response.status_code,
                url: response.url,
            });
        }

        Ok(response.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::http::HttpLimits;
    use crate::runtime::permissions::NetworkAllowlist;
    use crate::runtime::testing::spawn_server;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::sync::Arc;

    fn host() -> Version {
        Version::new(4, 10, 0)
    }

    fn fixture_document() -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/registry.json");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    }

    fn document_with(overrides: Value) -> String {
        let mut entry = serde_json::json!({
            "id": "fixture",
            "name": "fixture",
            "version": "1.0.0",
            "download_url": "https://example.test/fixture.sflx",
            "sha256": "a".repeat(64),
        });

        if let (Some(fields), Value::Object(overrides)) = (entry.as_object_mut(), overrides) {
            for (key, value) in overrides {
                fields.insert(key, value);
            }
        }

        serde_json::json!({
            "version": 1,
            "updated_at": "2026-10-29T00:00:00Z",
            "extensions": [entry],
        })
        .to_string()
    }

    #[test]
    fn the_fixture_registry_parses_every_entry_intact() {
        let registry = Registry::parse(&fixture_document(), &host()).expect("parses");

        assert_eq!(registry.schema_version, 1);
        assert_eq!(registry.entries.len(), 12);
        assert!(!registry.schema_is_newer);

        let ids: Vec<&str> = registry.entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "spotify-web",
                "amazon",
                "apple-music",
                "soundcloud",
                "ytmusic-spotiflac",
                "deezer",
                "qobuz-web",
                "tidal-web",
                "bugs-music",
                "melon-music",
                "genie-music",
                "jiosaavn",
            ]
        );

        let spotify = registry.entry("spotify-web").expect("spotify entry");
        assert_eq!(spotify.version, "1.10.4");
        assert_eq!(
            spotify.sha256,
            "5ccae8a8c236c97500328a22d69ac039c779179a0c9386e8e3b89e152389dbad"
        );
        assert_eq!(spotify.min_app_version.as_deref(), Some("4.3.0"));
        assert_eq!(spotify.label(), "Spotify Web");
        assert_eq!(spotify.category.as_deref(), Some("integration"));

        for entry in &registry.entries {
            assert_eq!(entry.sha256.len(), 64, "{} digest length", entry.id);
            assert!(
                entry.parsed_version().is_some(),
                "{} version {:?}",
                entry.id,
                entry.version
            );
        }
    }

    #[test]
    fn every_fixture_entry_is_supported_by_a_current_host() {
        let registry = Registry::parse(&fixture_document(), &host()).expect("parses");
        let refused: Vec<&str> = registry
            .unsupported()
            .map(|entry| entry.id.as_str())
            .collect();
        assert!(refused.is_empty(), "unexpected refusals: {refused:?}");
        assert_eq!(registry.supported().count(), 12);
    }

    #[test]
    fn a_minimum_version_above_the_host_refuses_the_entry() {
        let document = document_with(serde_json::json!({ "min_app_version": "9.0.0" }));
        let registry = Registry::parse(&document, &host()).expect("parses");
        let entry = registry.entry("fixture").expect("entry survives");

        match &entry.status {
            EntryStatus::Unsupported(Unsupported::HostTooOld {
                required,
                host: reported,
            }) => {
                assert_eq!(required, &Version::new(9, 0, 0));
                assert_eq!(reported, &host());
            }
            other => panic!("expected host_too_old, got {other:?}"),
        }
        assert_eq!(
            entry.status,
            EntryStatus::Unsupported(Unsupported::HostTooOld {
                required: Version::new(9, 0, 0),
                host: host(),
            })
        );
        assert!(!entry.offer_for(None).is_installable());
    }

    #[test]
    fn a_minimum_version_equal_to_the_host_is_supported() {
        let document = document_with(serde_json::json!({ "min_app_version": "4.10.0" }));
        let registry = Registry::parse(&document, &host()).expect("parses");
        assert!(registry.entry("fixture").expect("entry").status.is_supported());
    }

    #[test]
    fn an_unparseable_version_is_reported_against_its_entry() {
        for (field, document) in [
            (
                "version",
                document_with(serde_json::json!({ "version": "not-a-version" })),
            ),
            (
                "min_app_version",
                document_with(serde_json::json!({ "min_app_version": "4.x" })),
            ),
        ] {
            let registry = Registry::parse(&document, &host()).expect("parses");
            let entry = registry.entry("fixture").expect("entry is not dropped");

            match &entry.status {
                EntryStatus::Unsupported(Unsupported::InvalidVersion {
                    field: reported,
                    value,
                }) => {
                    assert_eq!(reported, field);
                    assert!(!value.is_empty());
                }
                other => panic!("{field}: expected invalid_version, got {other:?}"),
            }
            assert_eq!(registry.unsupported().count(), 1);
        }
    }

    #[test]
    fn a_digest_that_is_not_sha256_is_reported_rather_than_trusted() {
        let document = document_with(serde_json::json!({ "sha256": "abc123" }));
        let registry = Registry::parse(&document, &host()).expect("parses");
        let entry = registry.entry("fixture").expect("entry");

        assert_eq!(
            entry.status,
            EntryStatus::Unsupported(Unsupported::InvalidChecksum {
                value: "abc123".to_string()
            })
        );
        assert_eq!(entry.status.code(), "invalid_checksum");
    }

    #[test]
    fn an_equal_version_is_not_an_update_and_a_newer_one_is() {
        let registry = Registry::parse(&fixture_document(), &host()).expect("parses");
        let spotify = registry.entry("spotify-web").expect("entry");
        let installed = Version::new(1, 10, 4);

        let equal = spotify.offer_for(Some(&installed));
        assert_eq!(
            equal,
            InstallOffer::UpToDate {
                at: Version::new(1, 10, 4)
            }
        );
        assert!(!equal.is_installable(), "an equal version is not an update");

        let older = Version::new(1, 9, 0);
        let newer = spotify.offer_for(Some(&older));
        assert_eq!(
            newer,
            InstallOffer::Update {
                from: Version::new(1, 9, 0),
                to: Version::new(1, 10, 4),
            }
        );
        assert!(newer.is_installable());

        let ahead = Version::new(2, 0, 0);
        let downgrade = spotify.offer_for(Some(&ahead));
        assert_eq!(
            downgrade,
            InstallOffer::Downgrade {
                from: Version::new(2, 0, 0),
                to: Version::new(1, 10, 4),
            }
        );
        assert!(!downgrade.is_installable());

        assert_eq!(spotify.offer_for(None), InstallOffer::Fresh);
        assert!(spotify.offer_for(None).is_installable());
    }

    #[test]
    fn a_refused_entry_is_never_installable() {
        let document = document_with(serde_json::json!({ "min_app_version": "9.0.0" }));
        let registry = Registry::parse(&document, &host()).expect("parses");
        let entry = registry.entry("fixture").expect("entry");

        let offer = entry.offer_for(None);
        assert!(matches!(offer, InstallOffer::Refused { .. }), "{offer:?}");
        assert!(!offer.is_installable());
    }

    #[test]
    fn versions_parse_and_order_the_way_semver_says() {
        assert_eq!(Version::parse("4.10.0"), Some(Version::new(4, 10, 0)));
        assert_eq!(Version::parse(" 1.2.3 "), Some(Version::new(1, 2, 3)));

        // Build metadata is ignored, not rejected.
        assert_eq!(Version::parse("1.2.3+build.7"), Some(Version::new(1, 2, 3)));
        assert_eq!(
            Version::parse("1.2.3-rc.1"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3,
                prerelease: Some("rc.1".to_string()),
            })
        );

        for invalid in ["", "1", "1.2", "1.2.3.4", "1.2.x", "v1.2.3", "-1.2.3", "1.2.3-"] {
            assert_eq!(Version::parse(invalid), None, "{invalid:?} must not parse");
        }

        assert!(Version::parse("1.10.0").unwrap() > Version::parse("1.9.0").unwrap());
        assert!(Version::parse("2.0.0").unwrap() > Version::parse("1.99.99").unwrap());
        // A prerelease leads up to its release.
        assert!(Version::parse("1.0.0-alpha").unwrap() < Version::parse("1.0.0").unwrap());
        assert!(Version::parse("1.0.0-alpha").unwrap() < Version::parse("1.0.0-alpha.1").unwrap());
        assert!(Version::parse("1.0.0-alpha.2").unwrap() < Version::parse("1.0.0-alpha.10").unwrap());
        assert!(Version::parse("1.0.0-alpha.1").unwrap() < Version::parse("1.0.0-beta").unwrap());
        assert_eq!(Version::new(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn a_status_serialises_with_a_stable_code_the_ui_can_switch_on() {
        let document = document_with(serde_json::json!({ "min_app_version": "9.0.0" }));
        let registry = Registry::parse(&document, &host()).expect("parses");
        let entry = registry.entry("fixture").expect("entry");

        let json = serde_json::to_value(entry).expect("serialises");
        assert_eq!(json["status"]["state"], serde_json::json!("unsupported"));
        assert_eq!(json["status"]["reason"], serde_json::json!("host_too_old"));
        assert_eq!(json["status"]["required"], serde_json::json!("9.0.0"));
        assert_eq!(json["status"]["host"], serde_json::json!("4.10.0"));
    }

    #[test]
    fn a_newer_schema_is_reported_but_still_parsed() {        let document = serde_json::json!({
            "version": 99,
            "extensions": [{
                "id": "future",
                "version": "1.0.0",
                "download_url": "https://example.test/future.sflx",
                "sha256": "b".repeat(64),
                "unknown_field": { "nested": true }
            }]
        })
        .to_string();

        let registry = Registry::parse(&document, &host()).expect("parses");
        assert!(registry.schema_is_newer);
        let entry = registry.entry("future").expect("entry survives");
        assert!(entry.status.is_supported());
        assert_eq!(
            entry.extra.get("unknown_field"),
            Some(&serde_json::json!({ "nested": true }))
        );
    }

    #[test]
    fn a_document_that_is_not_json_is_a_typed_error() {
        let error = Registry::parse("<html>not found</html>", &host()).expect_err("refused");
        assert!(matches!(error, RegistryError::InvalidDocument { .. }), "{error:?}");
    }

    #[test]
    fn a_byte_order_mark_does_not_break_the_document() {
        let document = format!("\u{feff}{}", document_with(Value::Null));
        let registry = Registry::parse(&document, &host()).expect("parses past the BOM");
        assert_eq!(registry.entries.len(), 1);
    }

    fn local_client(url: String) -> RegistryClient {
        let allowlist = NetworkAllowlist::from_patterns(&["127.0.0.1".to_string()]);
        let http = HostHttp::new(allowlist, HttpLimits::with_timeout_seconds(5));
        RegistryClient::new(url, http, host())
    }

    #[test]
    fn a_fetch_stores_the_registry_and_a_failure_keeps_it_usable() {
        let broken = Arc::new(AtomicBool::new(false));
        let handler_flag = Arc::clone(&broken);
        let body = fixture_document();

        let base = spawn_server(move |_request| {
            if handler_flag.load(AtomicOrdering::SeqCst) {
                return (503, vec![], b"upstream down".to_vec());
            }
            (
                200,
                vec![("Content-Type".into(), "application/json".into())],
                body.clone().into_bytes(),
            )
        });

        let client = local_client(format!("{base}/registry.json"));
        assert!(client.cached().is_none());

        let registry = client.fetch().expect("first fetch succeeds");
        assert_eq!(registry.entries.len(), 12);
        assert_eq!(client.cached().expect("cached").entries.len(), 12);

        broken.store(true, AtomicOrdering::SeqCst);
        let error = client.fetch().expect_err("second fetch fails");
        assert_eq!(
            error,
            RegistryError::Status {
                status: 503,
                url: format!("{base}/registry.json"),
            }
        );

        let cached = client
            .cached()
            .expect("the previous registry stays usable after a failure");
        assert_eq!(cached.entries.len(), 12);
        assert!(cached.entry("tidal-web").is_some());
    }

    #[test]
    fn an_unreachable_host_is_a_typed_network_error() {
        // Port 9 on loopback is the discard service: nothing listens there in a
        // test environment, so the connection is refused immediately.
        let client = local_client("http://127.0.0.1:9/registry.json".to_string());
        let error = client.fetch().expect_err("refused");

        assert!(
            matches!(error, RegistryError::Network { .. }),
            "expected a typed network error, got {error:?}"
        );
        assert!(client.cached().is_none());
    }
}
