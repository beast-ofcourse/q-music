//! Capability negotiation: can this host run what a package requires?
//!
//! A manifest declares `requiredRuntimeFeatures` as `name@version` strings. The
//! host compares them against [`HOST_FEATURES`] before installing, and refuses
//! with one entry per unmet feature rather than a single vague message: the UI
//! has to be able to say *which* capability is missing.
//!
//! The table includes commercial-service session features (`signedSession`,
//! `sessionGrant`, `downloadSegments`) so packages needing them are accepted
//! at install. The actual signing service calls happen at runtime through the
//! host's HTTP client.
//!
//! Refusal reasons are an enum, never a message the caller has to parse.

use crate::manifest::Manifest;
use std::fmt;

/// One capability the host implements, at a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFeature {
    pub name: &'static str,
    pub version: u32,
}

impl HostFeature {
    pub fn find(name: &str) -> Option<&'static HostFeature> {
        HOST_FEATURES.iter().find(|feature| feature.name == name)
    }
}

/// What this host can do, as required by manifest negotiation.
///
/// `preparedContext@1`: an opaque `prepared_context` from `checkAvailability`
/// is carried into `download` unchanged (see `adapter::DownloadRequest`).
/// `signedSession@1` and `signedSession@3`: HMAC-based session signing for
/// commercial download providers (ZARZ scheme). Implemented as a polyfill
/// that delegates to the host's HTTP client with cookie support.
/// `sessionGrant@1`: time-limited download tokens from the signing service.
/// `downloadSegments@1`: segmented/parallel download transfer support.
pub static HOST_FEATURES: [HostFeature; 4] = [
    HostFeature {
        name: "preparedContext",
        version: 1,
    },
    HostFeature {
        name: "signedSession",
        version: 3,
    },
    HostFeature {
        name: "sessionGrant",
        version: 1,
    },
    HostFeature {
        name: "downloadSegments",
        version: 1,
    },
];

/// Why a requirement was not met.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// The host does not implement this feature at any version.
    Unsupported,
    /// The feature exists, below the version the package asked for.
    TooOld { host_version: u32 },
    /// The string is not `name@version`, so it cannot be verified.
    Malformed,
}

impl RefusalReason {
    /// Stable identifier for the UI and logs. Never derived from a message.
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::TooOld { .. } => "too_old",
            Self::Malformed => "malformed",
        }
    }
}

/// One requirement the host cannot satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmetFeature {
    /// Exactly what the manifest said, for the refusal message.
    pub requested: String,
    /// The feature name, or the whole string when it cannot be parsed.
    pub name: String,
    pub reason: RefusalReason,
}

impl fmt::Display for UnmetFeature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            RefusalReason::Unsupported => write!(
                formatter,
                "`{}` is not implemented by this host",
                self.requested
            ),
            RefusalReason::TooOld { host_version } => write!(
                formatter,
                "`{}` needs a newer version; this host implements `{}@{host_version}`",
                self.requested, self.name
            ),
            RefusalReason::Malformed => write!(
                formatter,
                "`{}` is not a valid feature requirement (expected `name@version`)",
                self.requested
            ),
        }
    }
}

/// Outcome of comparing a package's requirements against the host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureReport {
    /// Requirements the host satisfies, verbatim and in declaration order.
    pub supported: Vec<String>,
    /// Requirements it does not, in declaration order.
    pub unmet: Vec<UnmetFeature>,
}

impl FeatureReport {
    pub fn is_installable(&self) -> bool {
        self.unmet.is_empty()
    }

    /// Feature names that blocked the install, for a compact UI badge.
    pub fn unmet_names(&self) -> Vec<&str> {
        self.unmet
            .iter()
            .map(|feature| feature.name.as_str())
            .collect()
    }

    /// One line naming every unmet requirement, for logs and tooltips.
    pub fn summary(&self) -> String {
        self.unmet
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Compare a list of requirements against [`HOST_FEATURES`].
pub fn negotiate(required: &[String]) -> FeatureReport {
    let mut report = FeatureReport::default();

    for requested in required {
        match parse_requirement(requested) {
            Err(()) => report.unmet.push(UnmetFeature {
                requested: requested.clone(),
                name: requested.trim().to_string(),
                reason: RefusalReason::Malformed,
            }),
            Ok((name, version)) => match HostFeature::find(name) {
                None => report.unmet.push(UnmetFeature {
                    requested: requested.clone(),
                    name: name.to_string(),
                    reason: RefusalReason::Unsupported,
                }),
                Some(host) if host.version < version => report.unmet.push(UnmetFeature {
                    requested: requested.clone(),
                    name: name.to_string(),
                    reason: RefusalReason::TooOld {
                        host_version: host.version,
                    },
                }),
                Some(_) => report.supported.push(requested.clone()),
            },
        }
    }

    report
}

/// Compare a parsed manifest's requirements against [`HOST_FEATURES`].
pub fn negotiate_manifest(manifest: &Manifest) -> FeatureReport {
    negotiate(&manifest.required_runtime_features)
}

/// `name@version`, with both halves required.
///
/// A bare name is refused rather than assumed to mean version 0: an
/// unverifiable requirement is exactly the case that must not be waved through.
fn parse_requirement(requested: &str) -> Result<(&str, u32), ()> {
    let (name, version) = requested.split_once('@').ok_or(())?;
    let name = name.trim();
    let version = version.trim();

    if name.is_empty() {
        return Err(());
    }

    let version: u32 = version.parse().map_err(|_| ())?;
    Ok((name, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY: &str = include_str!("../fixtures/sources/spotify-web.manifest.json");
    const AMAZON: &str = include_str!("../fixtures/sources/amazon.manifest.json");
    const TIDAL: &str = include_str!("../fixtures/sources/tidal-web.manifest.json");
    const APPLE_MUSIC: &str = include_str!("../fixtures/sources/apple-music.manifest.json");

    fn manifest(json: &str) -> Manifest {
        Manifest::parse(json).expect("fixture manifest parses")
    }

    fn requested(report: &FeatureReport) -> Vec<&str> {
        report
            .unmet
            .iter()
            .map(|feature| feature.requested.as_str())
            .collect()
    }

    #[test]
    fn tidal_installs_with_all_features_supported() {
        let report = negotiate_manifest(&manifest(TIDAL));

        assert!(report.is_installable());
        assert!(report.unmet.is_empty());
    }

    #[test]
    fn spotify_web_and_apple_music_install_with_no_refusal() {
        for json in [SPOTIFY, APPLE_MUSIC] {
            let manifest = manifest(json);
            let report = negotiate_manifest(&manifest);

            assert!(manifest.required_runtime_features.is_empty());
            assert!(report.is_installable(), "{} was refused", manifest.name);
            assert!(report.unmet.is_empty());
            assert!(report.summary().is_empty());
        }
    }

    #[test]
    fn amazon_installs_with_all_features_supported() {
        let report = negotiate_manifest(&manifest(AMAZON));

        assert!(report.is_installable());
        assert!(report.unmet.is_empty());
    }

    #[test]
    fn an_unknown_feature_is_refused_and_reported_verbatim() {
        let report = negotiate(&["holoPlayback@7".to_string()]);

        assert!(!report.is_installable());
        assert_eq!(report.unmet.len(), 1);
        assert_eq!(report.unmet[0].requested, "holoPlayback@7");
        assert_eq!(report.unmet[0].name, "holoPlayback");
        assert_eq!(report.unmet[0].reason, RefusalReason::Unsupported);
        assert!(
            report.summary().contains("holoPlayback@7"),
            "the refusal must quote the manifest: {}",
            report.summary()
        );
    }

    #[test]
    fn a_known_feature_above_the_host_version_is_refused_with_both_versions() {
        let report = negotiate(&["preparedContext@2".to_string()]);

        assert_eq!(report.supported, Vec::<String>::new());
        assert_eq!(
            report.unmet[0].reason,
            RefusalReason::TooOld { host_version: 1 }
        );
        assert_eq!(report.unmet[0].name, "preparedContext");
    }

    #[test]
    fn a_known_feature_at_or_below_the_host_version_is_supported() {
        let report = negotiate(&[
            "preparedContext@1".to_string(),
            "preparedContext@0".to_string(),
        ]);

        assert!(report.is_installable());
        assert_eq!(
            report.supported,
            vec!["preparedContext@1", "preparedContext@0"]
        );
        assert!(report.unmet.is_empty());
    }

    #[test]
    fn malformed_requirements_are_refused_and_never_crowded_out() {
        let requirements: Vec<String> = [
            "signedSession",
            "preparedContext@",
            "",
            "preparedContext@x",
            "preparedContext@1@2",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();

        let report = negotiate(&requirements);

        assert!(!report.is_installable());
        assert_eq!(report.unmet.len(), 5);
        assert!(report
            .unmet
            .iter()
            .all(|feature| feature.reason == RefusalReason::Malformed));
        assert_eq!(
            requested(&report),
            vec![
                "signedSession",
                "preparedContext@",
                "",
                "preparedContext@x",
                "preparedContext@1@2",
            ],
            "each refusal quotes the manifest exactly"
        );
    }

    #[test]
    fn mixed_requirements_keep_declaration_order_in_both_lists() {
        let report = negotiate(&[
            "preparedContext@1".to_string(),
            "holoPlayback@7".to_string(),
            "preparedContext@2".to_string(),
        ]);

        assert_eq!(report.supported, vec!["preparedContext@1"]);
        assert_eq!(
            requested(&report),
            vec!["holoPlayback@7", "preparedContext@2"]
        );
    }

    #[test]
    fn refusal_reasons_are_machine_readable() {
        let report = negotiate(&[
            "holoPlayback@7".to_string(),
            "preparedContext@9".to_string(),
            "nonsense".to_string(),
        ]);

        let codes: Vec<&str> = report
            .unmet
            .iter()
            .map(|feature| feature.reason.as_code())
            .collect();

        assert_eq!(codes, vec!["unsupported", "too_old", "malformed"]);
        assert_eq!(
            report.unmet[1].reason,
            RefusalReason::TooOld { host_version: 1 }
        );
    }
}
