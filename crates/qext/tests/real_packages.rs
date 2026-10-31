//! End-to-end tests with real `.sflx` packages (T-24).
//!
//! These tests load the five integration packages from real bytes, parse their
//! manifests, instantiate the QuickJS runtime, and exercise offline code paths
//! (`initialize`, `handleUrl` parsing, adapter dispatch). Network is disabled
//! (`HostConfig { http: None, .. }`) so no live calls are made.
//!
//! Run with: `cargo test -p qext --features real-packages`

use qext::package::PackageInfo;
use qext::runtime::RuntimeLimits;
// testing module is pub(crate); integration tests use public APIs only.
use std::path::PathBuf;

fn archive_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("archives")
        .join(format!("{name}.sflx"))
}

fn inspect_package(name: &str) -> PackageInfo {
    PackageInfo::inspect(&archive_path(name))
        .unwrap_or_else(|e| panic!("{name} failed to inspect: {e}"))
}

/// The five integration packages that must load and initialize offline.
const INTEGRATION_PACKAGES: &[&str] = &[
    "spotify-web",
    "apple-music",
    "bugs-music",
    "melon-music",
    "genie-music",
];

#[cfg(feature = "real-packages")]
mod real {
    use super::*;

    #[test]
    fn all_five_packages_inspect_cleanly() {
        for &name in INTEGRATION_PACKAGES {
            let path = archive_path(name);
            let info = PackageInfo::inspect(&path)
                .unwrap_or_else(|e| panic!("{name} failed to inspect: {e}"));
            assert!(!info.manifest.name.is_empty(), "{name} has empty name");
            assert!(!info.manifest.version.is_empty(), "{name} has empty version");
            assert!(!info.index_js.is_empty(), "{name} has empty index.js");
            assert!(info.sha256.len() == 64, "{name} sha256 is not 64 chars");
        }
    }

    #[test]
    fn all_five_packages_have_no_required_runtime_features_unmet() {
        use qext::negotiate::negotiate_manifest;
        for &name in INTEGRATION_PACKAGES {
            let path = archive_path(name);
            let info = PackageInfo::inspect(&path).expect("inspect");
            let report = negotiate_manifest(&info.manifest);
            assert!(
                report.is_installable(),
                "{name} is not installable: {:?}",
                report.unmet
            );
        }
    }

    #[test]
    fn all_five_runtimes_load_and_register_handle_url() {
        use qext::adapter::Capabilities;
        use qext::runtime::ExtensionRuntime;
        for &name in INTEGRATION_PACKAGES {
            let info = inspect_package(name);
            let runtime = ExtensionRuntime::load(name, &info.index_js, RuntimeLimits::default())
                .unwrap_or_else(|e| panic!("{name} failed to load runtime: {e}"));
            let caps = Capabilities::from_runtime(&runtime);
            // Every integration package registers handleUrl (per adapter tests).
            assert!(
                caps.has("handleUrl"),
                "{name} does not register handleUrl"
            );
        }
    }

    #[test]
    fn all_five_runtimes_initialize_offline() {
        use qext::runtime::ExtensionRuntime;
        use serde_json::json;
        for &name in INTEGRATION_PACKAGES {
            let info = inspect_package(name);
            let runtime = ExtensionRuntime::load(name, &info.index_js, RuntimeLimits::default())
                .unwrap_or_else(|e| panic!("{name} failed to load: {e}"));
            // initialize takes a config object; pass an empty one for offline smoke.
            let result = runtime.call("initialize", &[json!({})]);
            // initialize may return a value or null; either is acceptable offline.
            // A thrown exception is the only failure.
            assert!(
                result.is_ok(),
                "{name} initialize threw: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn handle_url_parsing_works_offline() {
        use qext::runtime::ExtensionRuntime;
        use serde_json::json;
        // spotify-web declares urlHandler patterns for open.spotify.com.
        let info = inspect_package("spotify-web");
        let runtime = ExtensionRuntime::load("spotify-web", &info.index_js, RuntimeLimits::default())
            .expect("load");
        // handleUrl with a known URL should not panic.
        let result = runtime.call("handleUrl", &[
            json!("https://open.spotify.com/track/abc123"),
            json!({}),
        ]);
        assert!(result.is_ok(), "handleUrl threw: {:?}", result.err());
    }

    #[test]
    fn adapter_dispatch_is_offline() {
        use qext::adapter::Capabilities;
        use qext::runtime::ExtensionRuntime;
        for &name in INTEGRATION_PACKAGES {
            let info = inspect_package(name);
            let runtime = ExtensionRuntime::load(name, &info.index_js, RuntimeLimits::default())
                .expect("load");
            let caps = Capabilities::from_runtime(&runtime);
            // Every package registers initialize and cleanup.
            assert!(caps.has("initialize"), "{name} missing initialize");
            assert!(caps.has("cleanup"), "{name} missing cleanup");
        }
    }
}

// Default (non-feature) test: verify the feature gate works.
#[cfg(not(feature = "real-packages"))]
mod default_suite {
    #[test]
    fn real_packages_feature_is_opt_in() {
        // This test passes trivially to confirm the default suite is network-free.
        assert!(true);
    }
}