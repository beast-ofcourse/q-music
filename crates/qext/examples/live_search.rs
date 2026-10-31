//! Live probe: activate every installed extension and run a real search.
//!
//! Diagnostic harness, not part of the test suite. Prints per-stage evidence
//! for each installed package: manifest facts, registered surface, initialize
//! outcome, search outcome. Any stage failure is printed with its typed error
//! so the exact broken link in the chain is visible.
//!
//! Usage:
//!   cargo run -p qext --example live_search -- <extensions-root> [query]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use qext::adapter::{self, Capabilities};
use qext::calls::{CallCache, Calls, SearchOptions};
use qext::manifest::Manifest;
use qext::runtime::http::{HostHttp, HttpLimits};
use qext::runtime::permissions::NetworkAllowlist;
use qext::runtime::polyfills::{HostConfig, HostServices};
use qext::runtime::RuntimeLimits;
use qext::store::ExtensionStore;

/// Per-request HTTP timeout for the probe.
const HTTP_TIMEOUT_SECONDS: u64 = 20;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| {
        r"C:\Users\Bhavin\AppData\Roaming\Q-Music\extensions".to_string()
    }));
    let query = args.next().unwrap_or_else(|| "Adele".to_string());
    // QEXT_PROBE_NO_INIT=1 skips initialize; QEXT_PROBE_CALL_TIMEOUT_MS sets the
    // JS deadline, letting the probe reproduce the app's exact conditions.
    let run_initialize = std::env::var("QEXT_PROBE_NO_INIT").unwrap_or_default() != "1";
    let call_timeout_ms: u64 = std::env::var("QEXT_PROBE_CALL_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60_000);

    let limits = RuntimeLimits {
        call_timeout: Duration::from_millis(call_timeout_ms),
        ..RuntimeLimits::default()
    };
    let mut store = match ExtensionStore::open_with(&root, limits) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("FATAL: cannot open store at {}: {error}", root.display());
            std::process::exit(1);
        }
    };

    let scan = store.scan();
    println!(
        "store={} installed={} broken={}",
        root.display(),
        scan.installed.len(),
        scan.broken.len()
    );
    for broken in &scan.broken {
        println!("  BROKEN {}: {}", broken.id, broken.reason);
    }

    for installed in &scan.installed {
        println!("\n=== {} v{} enabled={} ===", installed.id, installed.version, installed.enabled);
        if !installed.enabled {
            println!("  skipped: disabled");
            continue;
        }

        let manifest: Manifest = match std::fs::read_to_string(installed.manifest_path())
            .and_then(|text| serde_json::from_str(&text).map_err(std::io::Error::other))
        {
            Ok(manifest) => manifest,
            Err(error) => {
                println!("  MANIFEST unreadable: {error}");
                continue;
            }
        };
        println!(
            "  kinds={:?} minAppVersion={:?} requiredFeatures={:?}",
            manifest.kinds, manifest.min_app_version, manifest.required_runtime_features
        );

        let allowlist = NetworkAllowlist::from_patterns(&manifest.permissions.network);
        let http = HostHttp::new(
            allowlist,
            HttpLimits::with_timeout_seconds(HTTP_TIMEOUT_SECONDS),
        );
        let host = Arc::new(HostServices::new(HostConfig {
            app_version: "4.10.0".to_string(),
            user_agent: "Q-Music/0.1.0".to_string(),
            data_dir: Some(root.join(".data").join(&installed.id)),
            http: Some(http),
            ffmpeg: None,
        }));

        if let Err(error) = store.activate(&installed.id, host) {
            println!("  ACTIVATE failed: {error}");
            continue;
        }
        println!("  activated");

        let Some(surface) = store.with_runtime(&installed.id, |runtime| {
            let caps = Capabilities::from_runtime(runtime);
            caps.registered()
                .iter()
                .map(|f| format!("{}({})", f.name, f.arity))
                .collect::<Vec<_>>()
        }) else {
            println!("  RUNTIME missing after activate");
            continue;
        };
        println!("  surface: {}", surface.join(", "));

        // initialize, with manifest default settings, exactly as a host would.
        if run_initialize {
            store
                .with_runtime(&installed.id, |runtime| {
                    let caps = Capabilities::from_runtime(runtime);
                    if !caps.has("initialize") {
                        println!("  INITIALIZE: not registered");
                        return;
                    }
                    let settings = manifest.default_settings();
                    let args = match caps.arity("initialize") {
                        Some(0) => vec![],
                        _ => vec![serde_json::Value::Object(settings)],
                    };
                    match adapter::call(runtime, "initialize", &args) {
                        Ok(value) => println!("  INITIALIZE ok: {value}"),
                        Err(error) => println!("  INITIALIZE failed: {error}"),
                    }
                })
                .unwrap_or_else(|| println!("  RUNTIME vanished before initialize"));
        }

        // Real network search.
        let outcome = store.with_runtime(&installed.id, |runtime| {
            let caps = Capabilities::from_runtime(runtime);
            if !caps.has("customSearch") && !caps.has("searchTracks") {
                println!("  SEARCH: no search function registered");
                return;
            }
            let mut cache = match CallCache::open_in_memory(0) {
                Ok(cache) => cache,
                Err(error) => {
                    println!("  SEARCH: cache unavailable: {error}");
                    return;
                }
            };
            let mut calls = Calls::new(&installed.id, runtime, &mut cache);
            let options = SearchOptions {
                limit: Some(5),
                filter: Some("all".to_string()),
                ..Default::default()
            };
            match calls.search(&query, &options) {
                Ok(results) => {
                    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
                    for hit in &results.hits {
                        *by_type.entry(hit.item_type().to_string()).or_default() += 1;
                    }
                    println!("  SEARCH ok: {} hits {:?}", results.hits.len(), by_type);
                    for hit in results.hits.iter().take(3) {
                        println!("    sample: {:?}", hit_summary(hit));
                    }
                }
                Err(error) => println!("  SEARCH failed: {error}"),
            }
        });
        if outcome.is_none() {
            println!("  SEARCH: runtime vanished");
        }
    }
}

fn hit_summary(hit: &qext::calls::SearchHit) -> String {
    match hit {
        qext::calls::SearchHit::Track(t) => format!("track '{}' by {:?} ({})", t.name, t.artists.joined(), t.id),
        qext::calls::SearchHit::Album(a) => format!("album '{}' ({})", a.name, a.id),
        qext::calls::SearchHit::Artist(a) => format!("artist '{}' ({})", a.name, a.id),
        qext::calls::SearchHit::Playlist(p) => format!("playlist '{}' ({})", p.name, p.id),
        qext::calls::SearchHit::Unknown { item_type, value } => format!("unknown '{item_type}': {value}"),
    }
}
