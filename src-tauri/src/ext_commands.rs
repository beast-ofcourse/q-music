//! Extension platform commands for the desktop shell.
//!
//! Layout:
//! - **Commands** translate between Tauri's boundary and `qext`, which owns the
//!   behaviour. Nothing here re-implements package semantics.
//! - **Queue** (`queue_*` + [`spawn_download_worker`]): one background worker
//!   drives queued jobs through a real `download` call on the provider's
//!   runtime. Progress and cancellation ride the exact `HostServices` the
//!   package code runs against, so `file.download` inside the package is what
//!   actually moves the bytes — resume, headers, cancel and all.
//! - **Persistence** (`state.json`): settings and priority survive restarts.
//!   Health does not, on purpose — a stale "unhealthy" verdict surviving a
//!   crash would silently disable a provider that is fine after a restart.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use qext::adapter::{self, DownloadFailureKind, DownloadOutcome};
use qext::registry::{RegistryClient, Version};
use qext::runtime::http::{HostHttp, HttpLimits};
use qext::runtime::permissions::NetworkAllowlist;
use qext::runtime::polyfills::{HostConfig, HostServices};
use qext::store::ExtensionStore;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// Version packages see through `app.appVersion()`. The platform targets
/// Mornye-compatible packages, so this is the Mornye host version this shell is
/// API-compatible with — not the desktop app's own version.
const HOST_APP_VERSION: &str = "4.10.0";

/// Registry document fetched for `extensions_install`.
const REGISTRY_URL: &str = "https://raw.githubusercontent.com/mornyeapp/Mornye-Extension/main/registry.json";

/// Every registry URL the host contacts: the document and each entry's
/// `download_url` are served from the same host, so one pattern covers both.
const REGISTRY_HOST: &str = "raw.githubusercontent.com";

const EVENT_QUEUE: &str = "queue://jobs";
const EVENT_INSTALL: &str = "extensions://progress";

/// A single lossless download takes well past the runtime's default 10 s
/// deadline: the byte transfer happens inside the package's own `download`
/// call, so the host deadline has to be comfortably past it. The store is
/// opened with this so long-running transfers survive their own runtime's
/// call watchdog.
pub const LONG_CALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Stages a queued job legitimately stops in; `queue_retry` only leaves these.
const TERMINAL_STAGES: [&str; 3] = ["done", "failed", "cancelled"];

/// Job ids must be unique even if two enqueues land in the same millisecond.
static JOB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_job_id() -> String {
    use std::sync::atomic::Ordering;
    format!("job-{}-{}", now_millis(), JOB_SEQ.fetch_add(1, Ordering::Relaxed))
}

fn host_user_agent() -> String {
    format!("Q-Music/{}", env!("CARGO_PKG_VERSION"))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// View struct returned to the frontend (camelCase, matching `extensions.ts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledExtensionView {
    pub id: String,
    pub version: String,
    pub display_name: Option<String>,
    pub sha256: String,
    pub installed_at: i64,
    pub enabled: bool,
    pub directory: String,
    pub data_directory: String,
}

impl From<qext::store::InstalledExtension> for InstalledExtensionView {
    fn from(ext: qext::store::InstalledExtension) -> Self {
        Self {
            id: ext.id,
            version: ext.version.to_string(),
            display_name: ext.display_name,
            sha256: ext.sha256,
            installed_at: ext.installed_at,
            enabled: ext.enabled,
            directory: ext.directory.to_string_lossy().to_string(),
            data_directory: ext.data_directory.to_string_lossy().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Persisted state (state.json)
// ---------------------------------------------------------------------------

/// Read `state.json` from `root` (the extensions directory). What the app
/// restores at startup.
pub fn persisted_priority(ext_state: &ExtensionState, capability: &str) -> Vec<String> {
    ext_state
        .persisted
        .lock()
        .ok()
        .and_then(|p| p.priority.get(capability).cloned())
        .unwrap_or_default()
}

pub fn save_persisted_priority(
    ext_state: &ExtensionState,
    capability: &str,
    ids: &[String],
) -> Result<(), String> {
    let mut persisted = ext_state
        .persisted
        .lock()
        .map_err(|_| "extension state poisoned".to_string())?;
    persisted.priority.insert(capability.to_string(), ids.to_vec());
    persisted.save(&ext_state.root)
}

pub fn load_persisted_state(root: &Path) -> PersistedState {
    PersistedState::load(root)
}

/// Settings for one installed extension, as the user left them.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedSettings {
    values: serde_json::Value,
    quality: String,
}

/// The whole `state.json` document: extension settings plus the provider
/// ordering the user chose per capability. Missing keys mean "use defaults".
/// The struct is public because `ExtensionState` exposes it; construction and
/// every mutation stay inside this module.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistedState {
    #[serde(default)]
    settings: BTreeMap<String, PersistedSettings>,
    #[serde(default)]
    priority: BTreeMap<String, Vec<String>>,
}

impl PersistedState {
    fn file(root: &Path) -> PathBuf {
        root.join("state.json")
    }

    /// Read the document, tolerating an absent or unreadable file: a corrupt
    /// `state.json` must never keep the app from starting.
    fn load(root: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(Self::file(root)) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Write atomically: a crash mid-write leaves the previous document, not a
    /// truncated one.
    fn save(&self, root: &Path) -> Result<(), String> {
        let path = Self::file(root);
        let staged = root.join("state.json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(&staged, text)
            .and_then(|_| std::fs::rename(&staged, &path))
            .map_err(|error| format!("cannot persist extension state: {error}"))
    }

    /// Forgetting one extension: uninstall removes its choices and its slot in
    /// every ordering, so nothing survives for an id the store no longer knows.
    fn prune(&mut self, id: &str) {
        self.settings.remove(id);
        for order in self.priority.values_mut() {
            order.retain(|candidate| candidate != id);
        }
    }
}

// ---------------------------------------------------------------------------
// Provider health
// ---------------------------------------------------------------------------

/// Consecutive failures after which a provider is skipped by every command.
const FAILURES_BEFORE_UNHEALTHY: u32 = 2;

/// Failure streak for one provider, shaped the way `ProviderView` reports it:
/// one failure warns (still healthy, message shown), two in a row mark it
/// unhealthy. A success or `providers_retry` clears the streak. The struct is
/// public because `ExtensionState` exposes it; every mutation stays inside
/// this module.
#[derive(Debug, Clone, Default)]
pub struct HealthRecord {
    consecutive_failures: u32,
    message: Option<String>,
}

impl HealthRecord {
    fn is_unhealthy(&self) -> bool {
        self.consecutive_failures >= FAILURES_BEFORE_UNHEALTHY
    }

    fn record_failure(&mut self, message: String) {
        self.message = Some(message);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
    }

    fn clear(&mut self) {
        self.consecutive_failures = 0;
        self.message = None;
    }
}

/// The shape `ProviderView.health` expects.
fn health_view(record: Option<&HealthRecord>) -> serde_json::Value {
    let Some(record) = record else {
        return serde_json::json!({
            "state": "healthy",
            "message": null,
            "consecutiveFailures": 0,
        });
    };

    let state = if record.is_unhealthy() { "unhealthy" } else { "healthy" };
    serde_json::json!({
        "state": state,
        "message": record.message,
        "consecutiveFailures": record.consecutive_failures,
    })
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Extension state managed by Tauri.
pub struct ExtensionState {
    pub store: Mutex<ExtensionStore>,
    /// The extensions root. `state.json` and the registry cache live beside it.
    pub root: PathBuf,
    /// The app data root, where finished downloads land (`<root>/downloads`).
    pub data_root: PathBuf,
    /// Handle this state belongs to; every stage publishes queue events itself.
    pub app: AppHandle,
    /// Download queue, in the exact shape `QueueJob` expects.
    pub jobs: Mutex<Vec<serde_json::Value>>,
    /// Failure streaks keyed by provider id. Grown lazily: absence is healthy.
    pub health: Mutex<HashMap<String, HealthRecord>>,
    /// Online search coordinator (request ids, cancellation, cache).
    pub search_coordinator: qext::search::SearchCoordinator,
    /// Settings and priority restored from `state.json` at startup.
    pub persisted: Mutex<PersistedState>,
}

impl ExtensionState {
    /// Per-extension data directory: file sandbox and storage for one package.
    fn data_dir(&self, id: &str) -> PathBuf {
        self.root.join(".data").join(id)
    }

    fn payload_health(&self, id: &str) -> serde_json::Value {
        let record = self
            .health
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(id)
            .cloned();
        health_view(record.as_ref())
    }

    /// Patch one job in place, then publish the queue.
    fn update_job(&self, job_id: &str, patch: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) {
        {
            let mut jobs = self
                .jobs
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let index = match jobs
                .iter()
                .position(|job| job.get("id").and_then(|value| value.as_str()) == Some(job_id))
            {
                Some(index) => index,
                None => return,
            };
            let object = match jobs[index].as_object_mut() {
                Some(object) => object,
                None => return,
            };
            object.insert("updatedAt".to_string(), serde_json::json!(now_millis()));
            patch(object);
        }
        self.publish_queue();
    }

    /// Broadcast the current queue to every window.
    fn publish_queue(&self) {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let _ = self.app.emit(EVENT_QUEUE, jobs);
    }
}

/// Broadcast one install-progress step.
fn publish_install(app: &AppHandle, id: &str, phase: &str, percent: u64, message: Option<&str>) {
    let _ = app.emit(
        EVENT_INSTALL,
        serde_json::json!({
            "id": id,
            "phase": phase,
            "percent": percent,
            "message": message,
        }),
    );
}

// ---------------------------------------------------------------------------
// Runtime activation (shared by every command that calls a package)
// ---------------------------------------------------------------------------

/// Load one extension into a runtime if it is not running, and return the
/// `HostServices` bound inside it.
///
/// Progress and cancellation live on the host instance the runtime was loaded
/// with, so callers must act through the returned instance, never forge their
/// own — otherwise a package would report progress to an object nobody
/// observes and obey a cancel nobody sent.
fn ensure_runtime(state: &ExtensionState, id: &str) -> Result<Arc<HostServices>, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    if !store.is_active(id) {
        let installed = store
            .installed(id)
            .ok_or_else(|| format!("{id} is not installed"))?;

        let manifest_text = std::fs::read_to_string(installed.manifest_path()).ok();
        let manifest: Option<qext::manifest::Manifest> =
            manifest_text.and_then(|text| serde_json::from_str(&text).ok());

        let allowlist = manifest
            .as_ref()
            .map(|entry| NetworkAllowlist::from_patterns(&entry.permissions.network))
            .unwrap_or_default();
        let config = HostConfig {
            app_version: HOST_APP_VERSION.to_string(),
            user_agent: host_user_agent(),
            data_dir: Some(state.data_dir(id)),
            http: Some(HostHttp::new(allowlist, HttpLimits::default())),
            ffmpeg: None,
        };
        store
            .activate(id, Arc::new(HostServices::new(config)))
            .map_err(|error| error.to_string())?;
    }

    let runtime = store
        .runtime_of(id)
        .ok_or_else(|| format!("{id} could not be activated"))?;
    Ok(runtime.host().clone())
}

/// Which capability string a manifest's `kinds` list grants.
fn manifest_provides(manifest: &qext::manifest::Manifest, capability: &str) -> bool {
    let wanted = match capability {
        "metadata" => "metadata_provider",
        "download" => "download_provider",
        "lyrics" => "lyrics_provider",
        _ => return false,
    };
    manifest.kinds.iter().any(|kind| kind == wanted)
}

/// Read a package's manifest from what the scan already knows, tolerating
/// failure.
fn manifest_of(installed: &qext::store::InstalledExtension) -> Option<qext::manifest::Manifest> {
    let text = std::fs::read_to_string(installed.manifest_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Read a provider's manifest from the store, or None if not installed.
fn provider_manifest(state: &ExtensionState, id: &str) -> Option<qext::manifest::Manifest> {
    let store = state.store.lock().ok()?;
    let installed = store.installed(id)?;
    manifest_of(&installed)
}

// ---------------------------------------------------------------------------
// Extension lifecycle commands
// ---------------------------------------------------------------------------

/// List installed extensions.
#[tauri::command]
pub fn extensions_snapshot(
    ext_state: State<'_, ExtensionState>,
) -> Result<serde_json::Value, String> {
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let scan = store.scan();
    let installed: Vec<InstalledExtensionView> =
        scan.installed.into_iter().map(Into::into).collect();
    let broken: Vec<serde_json::Value> = scan
        .broken
        .into_iter()
        .map(|broken| serde_json::json!({"id": broken.id, "path": broken.path, "reason": broken.reason}))
        .collect();

    Ok(serde_json::json!({
        "entries": [],
        "installed": installed,
        "broken": broken,
        "refreshError": null,
        "fetchedAt": 0,
    }))
}

/// Install an extension from the registry: fetch the document, download the
/// package, verify its checksum and hand it to the store.
///
/// What installs is whatever the store accepts: the `sha256` from the registry
/// document must match the downloaded package and the package's own manifest id
/// must match the entry, both mismatching being a typed refusal — the error a
/// failure shows is the store's refusal, nothing invented here.
#[tauri::command]
pub async fn extensions_install(
    app: AppHandle,
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<InstalledExtensionView, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let failure = |app: &AppHandle, message: &str| {
        publish_install(app, &id, "failed", 0, Some(message));
        message.to_string()
    };

    publish_install(&app, &id, "queued", 0, None);

    // Two identical clients because `HostHttp` is not shared: the registry
    // document and the entry's `download_url` are fetched independently. Both
    // allowlists pin the same registry host, so a redirect off that host is
    // denied outright.
    let registry_http = HostHttp::new(
        NetworkAllowlist::from_patterns(&[REGISTRY_HOST.to_string()]),
        HttpLimits::default(),
    );
    let package_http = HostHttp::new(
        NetworkAllowlist::from_patterns(&[REGISTRY_HOST.to_string()]),
        HttpLimits::default(),
    );
    let host_version = Version::parse(HOST_APP_VERSION)
        .ok_or_else(|| failure(&app, "the host version is not parseable"))?;
    let client = RegistryClient::new(REGISTRY_URL, registry_http, host_version);

    publish_install(&app, &id, "downloading", 0, None);

    let registry = match client.fetch() {
        Ok(registry) => registry,
        Err(error) => {
            let message = format!("registry fetch failed: {error}");
            return Err(failure(&app, &message));
        }
    };

    let entry = match registry.entry(&id) {
        Some(entry) => entry,
        None => {
            let message = format!("the registry has no entry called {id}");
            return Err(failure(&app, &message));
        }
    };

    if !entry.status.is_supported() {
        let reason = entry.status.code().to_string();
        let message = format!("{id} is not installable on this host ({reason})");
        return Err(failure(&app, &message));
    }

    let bytes = match package_http.get_bytes(&entry.download_url, &[]) {
        Ok(bytes) => bytes,
        Err(error) => {
            let message = format!("package download failed: {error}");
            return Err(failure(&app, &message));
        }
    };

    publish_install(&app, &id, "verifying", 100, Some("checking SHA-256"));
    let staged = ext_state.root.join(".cache").join(format!("{id}.sflx"));
    if let Err(error) = write_bytes(&staged, &bytes) {
        return Err(failure(&app, &error));
    }

    publish_install(&app, &id, "installing", 100, None);
    let installed = store.install_from_registry(entry, &staged).map_err(|error| {
        failure(&app, &error.to_string())
    });
    let _ = std::fs::remove_file(&staged);

    let installed = installed?;
    publish_install(&app, &id, "done", 100, None);
    Ok(installed.into())
}

/// Write a downloaded package to a cache location, creating parents.
fn write_bytes(target: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create cache directory: {error}"))?;
    }
    std::fs::write(target, bytes).map_err(|error| format!("cannot write the downloaded package: {error}"))
}

/// Install an extension from a file chosen via dialog.
#[tauri::command]
pub async fn extensions_install_file(
    app: AppHandle,
    ext_state: State<'_, ExtensionState>,
) -> Result<Option<InstalledExtensionView>, String> {
    let path = app
        .dialog()
        .file()
        .add_filter("Extension package", &["sflx", "spotiflac-ext"])
        .blocking_pick_file();

    let Some(path) = path else {
        return Ok(None);
    };

    let path = path
        .as_path()
        .ok_or_else(|| "invalid file path".to_string())?
        .to_path_buf();

    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let installed = store.install(&path).map_err(|error| error.to_string())?;

    Ok(Some(installed.into()))
}

/// Uninstall an extension.
#[tauri::command]
pub async fn extensions_uninstall(
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<(), String> {
    let mut persisted = ext_state
        .persisted
        .lock()
        .map_err(|_| "extension state poisoned".to_string())?;
    let _ = save_pruned(&mut persisted, &ext_state.root, &id);
    drop(persisted);

    // A reinstall starts from a clean slate, failure streak included.
    if let Ok(mut health) = ext_state.health.lock() {
        health.remove(&id);
    }

    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;
    store.deactivate(&id);
    store.uninstall(&id).map_err(|error| error.to_string())?;
    Ok(())
}

/// Remove one id from the persisted document and write it back.
fn save_pruned(
    persisted: &mut PersistedState,
    root: &Path,
    id: &str,
) -> Result<(), String> {
    persisted.prune(id);
    persisted.save(root)
}

/// Set extension enabled/disabled.
#[tauri::command]
pub async fn extensions_set_enabled(
    ext_state: State<'_, ExtensionState>,
    id: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let mut store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    store.set_enabled(&id, enabled).map_err(|error| error.to_string())?;
    // A disabled extension must not keep a live runtime: it would keep serving
    // commands that never re-check.
    if !enabled {
        store.deactivate(&id);
    }

    let installed = store
        .installed(&id)
        .ok_or_else(|| format!("{id} not installed"))?;

    Ok(serde_json::json!({
        "id": installed.id,
        "version": installed.version.to_string(),
        "displayName": installed.display_name,
        "sha256": installed.sha256,
        "installedAt": installed.installed_at,
        "enabled": installed.enabled,
        "directory": installed.directory.to_string_lossy().to_string(),
        "dataDirectory": installed.data_directory.to_string_lossy().to_string(),
    }))
}

/// Settings schema for one extension, with the user's persisted choices.
fn settings_view(
    store: &ExtensionStore,
    persisted: &PersistedState,
    id: &str,
) -> Result<serde_json::Value, String> {
    let installed = store.installed(id).ok_or_else(|| format!("{id} not installed"))?;
    let text = std::fs::read_to_string(installed.manifest_path()).map_err(|error| error.to_string())?;
    let manifest: qext::manifest::Manifest = serde_json::from_str(&text).map_err(|error| error.to_string())?;

    let settings: Vec<serde_json::Value> = manifest
        .settings
        .iter()
        .map(|setting| {
            serde_json::json!({
                "key": setting.key,
                "label": setting.label,
                "type": setting.setting_type,
                "default": setting.default.clone(),
                "options": setting.options,
                "description": setting.description,
            })
        })
        .collect();

    let quality_options: Vec<serde_json::Value> = manifest
        .quality_options
        .iter()
        .map(|quality| {
            serde_json::json!({
                "id": quality.id,
                "label": quality.label,
                "description": quality.description,
                "kind": quality.kind,
            })
        })
        .collect();

    let saved = persisted.settings.get(id);
    Ok(serde_json::json!({
        "id": id,
        "settings": settings,
        "qualities": if quality_options.is_empty() {
            vec![serde_json::json!({"id": "best", "label": "Best available"})]
        } else {
            quality_options
        },
        "values": saved.map(|entry| entry.values.clone()).unwrap_or(serde_json::json!({})),
        "quality": saved.map(|entry| entry.quality.clone()).unwrap_or_else(|| "best".to_string()),
    }))
}

/// Get extension settings.
#[tauri::command]
pub fn extensions_settings(
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;
    let persisted = ext_state
        .persisted
        .lock()
        .map_err(|_| "extension state poisoned".to_string())?;

    settings_view(&store, &persisted, &id)
}

/// Save extension settings. Persists to `state.json`, so they survive
/// restarts.
#[tauri::command]
pub async fn extensions_save_settings(
    ext_state: State<'_, ExtensionState>,
    id: String,
    values: serde_json::Value,
    quality: String,
) -> Result<serde_json::Value, String> {
    {
        let mut persisted = ext_state
            .persisted
            .lock()
            .map_err(|_| "extension state poisoned".to_string())?;
        let entry = persisted.settings.entry(id.clone()).or_insert(PersistedSettings {
            values: serde_json::json!({}),
            quality: "best".to_string(),
        });
        entry.values = values;
        entry.quality = quality;
        persisted.save(&ext_state.root)?;
    }

    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;
    let persisted = ext_state
        .persisted
        .lock()
        .map_err(|_| "extension state poisoned".to_string())?;

    settings_view(&store, &persisted, &id)
}

/// Invoke an extension action.
///
/// The manifest format defines no named actions, so there is nothing real to
/// dispatch; this verifies the extension exists and reports the request was
/// noted. `docs/audit-report.md` records it as the one command without a
/// package-side counterpart.
#[tauri::command]
pub async fn extensions_invoke_action(
    ext_state: State<'_, ExtensionState>,
    id: String,
    action: String,
) -> Result<serde_json::Value, String> {
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;
    let installed = store.installed(&id).ok_or_else(|| format!("{id} not installed"))?;

    Ok(serde_json::json!({
        "ok": true,
        "message": format!("Action '{}' noted for {}", action, installed.display_name.unwrap_or_else(|| id.clone())),
    }))
}

// ---------------------------------------------------------------------------
// Provider commands
// ---------------------------------------------------------------------------

/// The failure text a search-group entry carries when it could not serve.
fn search_failure(id: &str, display_name: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "provider": id,
        "providerName": display_name,
        "results": [],
        "failure": message,
    })
}

/// Record one failed provider call, with the text the extension reported.
fn record_provider_failure(ext_state: &ExtensionState, id: &str, message: &str) {
    let mut health = ext_state
        .health
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(record) = health.get_mut(id) {
        record.record_failure(message.to_string());
    } else {
        let mut record = HealthRecord::default();
        record.record_failure(message.to_string());
        health.insert(id.to_string(), record);
    }
}

/// Clear a provider's failure streak.
fn clear_provider_failure(ext_state: &ExtensionState, id: &str) {
    if let Ok(mut health) = ext_state.health.lock() {
        if let Some(record) = health.get_mut(id) {
            record.clear();
        }
    }
}

/// Whether a provider should be skipped because it kept failing.
fn provider_is_unhealthy(ext_state: &ExtensionState, id: &str) -> bool {
    ext_state
        .health
        .lock()
        .map(|health| health.get(id).is_some_and(HealthRecord::is_unhealthy))
        .unwrap_or(false)
}

/// Whether a provider should be tried, or skipped with its reason.
fn try_provider(ext_state: &ExtensionState, installed: &qext::store::InstalledExtension) -> Option<&'static str> {
    if !installed.enabled {
        // Not an error worth a message: the UI hides disabled providers.
        return None;
    }
    if provider_is_unhealthy(ext_state, &installed.id) {
        return Some("skipped: the provider is unhealthy");
    }
    None
}

/// Search all installed metadata providers.
#[tauri::command]
pub async fn search_providers(
    ext_state: State<'_, ExtensionState>,
    query: String,
    limit: u32,
) -> Result<serde_json::Value, String> {
    let _ = std::fs::create_dir_all(ext_state.root.join(".data"));

    let scan = {
        let store = ext_state
            .store
            .lock()
            .map_err(|_| "extension store poisoned".to_string())?;
        store.scan()
    };

    let mut outcomes: Vec<serde_json::Value> = Vec::new();

    for installed in &scan.installed {
        if !installed.enabled {
            continue;
        }
        if let Some(reason) = try_provider(&ext_state, installed) {
            outcomes.push(search_failure(
                &installed.id,
                installed.display_name.as_deref().unwrap_or(&installed.id),
                reason,
            ));
            continue;
        }
        let Some(manifest) = manifest_of(installed) else {
            continue;
        };
        if !manifest_provides(&manifest, "metadata") {
            continue;
        }

        let _ = ensure_runtime(&ext_state, &installed.id);

        let store = ext_state
            .store
            .lock()
            .map_err(|_| "extension store poisoned".to_string())?;

        let result = store.with_runtime(&installed.id, |runtime| -> Result<Option<qext::calls::SearchResults>, String> {
            let capabilities = qext::adapter::Capabilities::from_runtime(runtime);
            if !capabilities.has("searchTracks") && !capabilities.has("customSearch") {
                return Ok(None);
            }
            // Search results are live data; a persistent cache would serve
            // yesterday's catalogue. Expire everything immediately.
            let mut cache = qext::calls::CallCache::open_in_memory(0)
                .map_err(|error| error.to_string())?;
            let mut calls = qext::calls::Calls::new(&installed.id, runtime, &mut cache);
            let options = qext::calls::SearchOptions {
                limit: Some(limit),
                ..Default::default()
            };
            let results = calls.search(&query, &options).map_err(|error| error.to_string())?;
            Ok(Some(results))
        });

        match result {
            Some(Ok(Some(results))) => {
                clear_provider_failure(&ext_state, &installed.id);
                let hits: Vec<serde_json::Value> = results
                    .hits
                    .iter()
                    .filter_map(|hit| match hit {
                        qext::calls::SearchHit::Track(track) => Some(serde_json::json!({
                            "provider": installed.id,
                            "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                            "providerTrackId": track.id.clone(),
                            "title": track.name.clone(),
                            "artist": track.artists.joined(),
                            "album": track.album_name.clone().unwrap_or_default(),
                            "durationMs": track.duration_ms,
                            "artUrl": track.images.first().cloned(),
                            "quality": "best",
                        })),
                        _ => None,
                    })
                    .collect();
                outcomes.push(serde_json::json!({
                    "provider": installed.id,
                    "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                    "results": hits,
                    "failure": null,
                }));
            }
            Some(Ok(None)) => {
                let message = "the package registers no search function";
                outcomes.push(search_failure(
                    &installed.id,
                    installed.display_name.as_deref().unwrap_or(&installed.id),
                    message,
                ));
            }
            Some(Err(message)) => {
                record_provider_failure(&ext_state, &installed.id, &message);
                outcomes.push(search_failure(
                    &installed.id,
                    installed.display_name.as_deref().unwrap_or(&installed.id),
                    &message,
                ));
            }
            None => {
                let message = "Extension not active.";
                record_provider_failure(&ext_state, &installed.id, message);
                outcomes.push(search_failure(
                    &installed.id,
                    installed.display_name.as_deref().unwrap_or(&installed.id),
                    message,
                ));
            }
        }
    }

    Ok(serde_json::json!(outcomes))
}

/// Provider rows for one capability, honouring the persisted priority order.
fn providers_for_capability(
    store: &ExtensionStore,
    ext_state: &ExtensionState,
    capability: &str,
) -> Vec<serde_json::Value> {
    let scan = store.scan();
    let priority = {
        let persisted = ext_state
            .persisted
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        persisted
            .priority
            .get(capability)
            .cloned()
            .unwrap_or_default()
    };

    let mut providers: Vec<serde_json::Value> = scan
        .installed
        .iter()
        .filter(|installed| {
            installed.enabled
                && manifest_of(installed)
                    .is_some_and(|manifest| manifest_provides(&manifest, capability))
        })
        .map(|installed| {
            serde_json::json!({
                "id": installed.id,
                "displayName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                "capability": capability,
                "version": installed.version.to_string(),
                "enabled": installed.enabled,
                "quality": "best",
                "qualities": [],
                "health": ext_state.payload_health(&installed.id),
                "missingFeatures": [],
            })
        })
        .collect();

    // Named providers come first, in the saved order; the rest keep scan order.
    providers.sort_by_key(|row| {
        let id = row.get("id").and_then(|value| value.as_str()).unwrap_or("");
        match priority.iter().position(|entry| entry == id) {
            Some(position) => position as u64,
            None => u64::MAX,
        }
    });
    providers
}

/// List installed providers grouped by capability.
#[tauri::command]
pub fn providers_list(ext_state: State<'_, ExtensionState>) -> Result<serde_json::Value, String> {
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;

    let groups = ["metadata", "download", "lyrics"]
        .into_iter()
        .map(|capability| {
            serde_json::json!({
                "capability": capability,
                "providers": providers_for_capability(&store, &ext_state, capability),
            })
        })
        .collect::<Vec<_>>();

    Ok(serde_json::json!(groups))
}

/// Set provider priority. Persists per capability and is honoured on the next
/// `providers_list`.
#[tauri::command]
pub async fn providers_set_priority(
    ext_state: State<'_, ExtensionState>,
    capability: String,
    ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    {
        let mut persisted = ext_state
            .persisted
            .lock()
            .map_err(|_| "extension state poisoned".to_string())?;
        persisted.priority.insert(capability, ids.clone());
        persisted.save(&ext_state.root)?;
    }
    providers_list(ext_state)
}

/// Retry a provider: forget its failure streak so the next call takes it.
#[tauri::command]
pub async fn providers_retry(
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<serde_json::Value, String> {
    clear_provider_failure(&ext_state, &id);
    Ok(serde_json::json!({
        "state": "healthy",
        "message": null,
        "consecutiveFailures": 0,
    }))
}

/// Fetch lyrics from installed lyrics providers.
#[tauri::command]
pub async fn provider_lyrics(
    ext_state: State<'_, ExtensionState>,
    track_id: i64,
) -> Result<serde_json::Value, String> {
    let scan = {
        let store = ext_state
            .store
            .lock()
            .map_err(|_| "extension store poisoned".to_string())?;
        store.scan()
    };

    for installed in &scan.installed {
        if try_provider(&ext_state, installed).is_some() {
            continue;
        }
        let Some(manifest) = manifest_of(installed) else {
            continue;
        };
        if !manifest_provides(&manifest, "lyrics") {
            continue;
        }

        let _ = ensure_runtime(&ext_state, &installed.id);

        let store = ext_state
            .store
            .lock()
            .map_err(|_| "extension store poisoned".to_string())?;

        let result = store.with_runtime(&installed.id, |runtime| -> Result<Option<qext::calls::Resolved<qext::contract::LyricsResult>>, String> {
            let capabilities = qext::adapter::Capabilities::from_runtime(runtime);
            if !capabilities.has("fetchLyrics") && !capabilities.has("getLyrics") {
                return Ok(None);
            }
            let mut cache = qext::calls::CallCache::open_in_memory(0)
                .map_err(|error| error.to_string())?;
            let mut calls = qext::calls::Calls::new(&installed.id, runtime, &mut cache);
            let request = qext::calls::LyricsRequest {
                track_id: Some(format!("{}", track_id)),
                track_name: String::new(),
                artist_name: String::new(),
                album_name: None,
                duration_seconds: None,
                extra: BTreeMap::new(),
            };
            let lyrics = calls
                .lyrics(&request)
                .map_err(|error| error.to_string())?;
            Ok(lyrics)
        });

        match result {
            Some(Ok(Some(resolved))) => {
                clear_provider_failure(&ext_state, &installed.id);
                let lyrics = &resolved.item;
                let lines: Vec<serde_json::Value> = lyrics
                    .lines
                    .iter()
                    .map(|line| {
                        serde_json::json!({
                            "timeMs": line.start_time_ms,
                            "text": line.words,
                        })
                    })
                    .collect();
                let synced = !lyrics.lines.is_empty()
                    && lyrics.lines.iter().all(|line| line.start_time_ms > 0);

                return Ok(serde_json::json!({
                    "provider": installed.id,
                    "providerName": installed.display_name.clone().unwrap_or_else(|| installed.id.clone()),
                    "trackId": track_id,
                    "synced": synced,
                    "lines": lines,
                }));
            }
            // Told there are no lyrics: the provider answered, not failed.
            Some(Ok(None)) => clear_provider_failure(&ext_state, &installed.id),
            Some(Err(message)) => {
                record_provider_failure(&ext_state, &installed.id, &message);
            }
            None => {
                let message = "the provider could not be called";
                record_provider_failure(&ext_state, &installed.id, message);
            }
        }
    }

    Ok(serde_json::json!(null))
}

// ---------------------------------------------------------------------------
// Provider metadata commands (handleUrl, home feed, browse categories)
// ---------------------------------------------------------------------------

/// Resolve a URL through the first installed provider that implements handleUrl.
#[tauri::command]
pub async fn extensions_handle_url(
    ext_state: State<'_, ExtensionState>,
    url: String,
) -> Result<serde_json::Value, String> {
    let scan = {
        let store = ext_state
            .store
            .lock()
            .map_err(|_| "extension store poisoned".to_string())?;
        store.scan()
    };

    for installed in &scan.installed {
        if !installed.enabled {
            continue;
        }
        let Some(manifest) = manifest_of(installed) else {
            continue;
        };
        if !manifest_provides(&manifest, "metadata") {
            continue;
        }
        if provider_is_unhealthy(&ext_state, &installed.id) {
            continue;
        }

        let _ = ensure_runtime(&ext_state, &installed.id);
        let store = ext_state
            .store
            .lock()
            .map_err(|_| "extension store poisoned".to_string())?;
        let result = store.with_runtime(&installed.id, |runtime| {
            let capabilities = qext::adapter::Capabilities::from_runtime(runtime);
            if !capabilities.has("handleUrl") {
                return Ok(None);
            }
            let mut cache = qext::calls::CallCache::open_in_memory(0)
                .map_err(|error| error.to_string())?;
            let mut calls = qext::calls::Calls::new(&installed.id, runtime, &mut cache);
            calls.handle_url(&url).map_err(|error| error.to_string())
        });
        drop(store);

        match result {
            Some(Ok(Some(resolved))) => {
                clear_provider_failure(&ext_state, &installed.id);
                return Ok(serde_json::to_value(resolved).unwrap_or(serde_json::json!(null)));
            }
            Some(Ok(None)) => {}
            Some(Err(message)) => {
                record_provider_failure(&ext_state, &installed.id, &message);
            }
            None => {}
        }
    }

    Ok(serde_json::json!(null))
}

/// Fetch a provider's home feed (only meaningful when the manifest declares
/// `capabilities.homeFeed`).
#[tauri::command]
pub async fn provider_home_feed(
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let _ = ensure_runtime(&ext_state, &id);
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;
    let result = store.with_runtime(&id, |runtime| {
        let capabilities = qext::adapter::Capabilities::from_runtime(runtime);
        if !capabilities.has("getHomeFeed") {
            return Ok(None);
        }
        let mut cache = qext::calls::CallCache::open_in_memory(0)
            .map_err(|error| error.to_string())?;
        let mut calls = qext::calls::Calls::new(&id, runtime, &mut cache);
        calls.home_feed().map_err(|error| error.to_string())
    });
    drop(store);

    match result {
        Some(Ok(Some(resolved))) => {
            clear_provider_failure(&ext_state, &id);
            Ok(serde_json::to_value(resolved).unwrap_or(serde_json::json!(null)))
        }
        Some(Ok(None)) => Ok(serde_json::json!(null)),
        Some(Err(message)) => {
            record_provider_failure(&ext_state, &id, &message);
            Err(message)
        }
        None => Err("provider is not active".to_string()),
    }
}

/// Fetch a provider's browse categories.
#[tauri::command]
pub async fn provider_browse_categories(
    ext_state: State<'_, ExtensionState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let _ = ensure_runtime(&ext_state, &id);
    let store = ext_state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;
    let result = store.with_runtime(&id, |runtime| {
        let capabilities = qext::adapter::Capabilities::from_runtime(runtime);
        if !capabilities.has("getBrowseCategories") {
            return Ok(None);
        }
        let mut cache = qext::calls::CallCache::open_in_memory(0)
            .map_err(|error| error.to_string())?;
        let mut calls = qext::calls::Calls::new(&id, runtime, &mut cache);
        calls.browse_categories().map_err(|error| error.to_string())
    });
    drop(store);

    match result {
        Some(Ok(Some(resolved))) => {
            clear_provider_failure(&ext_state, &id);
            Ok(serde_json::to_value(resolved).unwrap_or(serde_json::json!(null)))
        }
        Some(Ok(None)) => Ok(serde_json::json!(null)),
        Some(Err(message)) => {
            record_provider_failure(&ext_state, &id, &message);
            Err(message)
        }
        None => Err("provider is not active".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Download queue and worker
// ---------------------------------------------------------------------------

/// Enqueue a download from a provider track.
#[tauri::command]
pub fn queue_enqueue(
    ext_state: State<'_, ExtensionState>,
    track: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let job = serde_json::json!({
        "id": next_job_id(),
        "trackId": track.get("providerTrackId").and_then(|value| value.as_str()).unwrap_or(""),
        "provider": track.get("provider").and_then(|value| value.as_str()).unwrap_or(""),
        "quality": track.get("quality").and_then(|value| value.as_str()).unwrap_or("best"),
        "title": track.get("title").and_then(|value| value.as_str()).unwrap_or(""),
        "artist": track.get("artist").and_then(|value| value.as_str()).unwrap_or(""),
        // Fields the worker needs that `QueueJob` does not render.
        "album": track.get("album").and_then(|value| value.as_str()).unwrap_or(""),
        "durationMs": track.get("durationMs"),
        "artUrl": track.get("artUrl"),
        "stage": "queued",
        "percent": 0,
        "bytesWritten": 0,
        "bytesTotal": null,
        "speedBps": null,
        "outputPath": "",
        "failure": null,
        "updatedAt": now_millis(),
    });

    {
        let mut jobs = ext_state
            .jobs
            .lock()
            .map_err(|_| "download jobs poisoned".to_string())?;
        jobs.push(job.clone());
    }
    ext_state.publish_queue();

    Ok(job)
}

/// List download queue jobs.
#[tauri::command]
pub fn queue_jobs(ext_state: State<'_, ExtensionState>) -> Result<serde_json::Value, String> {
    let jobs = ext_state
        .jobs
        .lock()
        .map_err(|_| "download jobs poisoned".to_string())?;
    Ok(serde_json::json!(*jobs))
}

/// Cancel a download job.
///
/// A queued job becomes `cancelled` straight away. A running one asks its
/// package to abandon the transfer through the shared host cancellation flag;
/// the worker confirms when the package exits with `cancelled`.
#[tauri::command]
pub fn queue_cancel(
    ext_state: State<'_, ExtensionState>,
    job_id: String,
) -> Result<(), String> {
    let (provider, stage) = {
        let jobs = ext_state
            .jobs
            .lock()
            .map_err(|_| "download jobs poisoned".to_string())?;
        match jobs
            .iter()
            .find(|job| job.get("id").and_then(|value| value.as_str()) == Some(&job_id))
        {
            Some(job) => (
                job.get("provider").and_then(|value| value.as_str()).unwrap_or("").to_string(),
                job.get("stage").and_then(|value| value.as_str()).unwrap_or("").to_string(),
            ),
            None => return Ok(()),
        }
    };

    if TERMINAL_STAGES.contains(&stage.as_str()) {
        return Ok(());
    }

    // The flag lives on the host the package's runtime runs with.
    if let Ok(store) = ext_state.store.lock() {
        if let Some(runtime) = store.runtime_of(&provider) {
            runtime.host().cancel();
        }
    }

    ext_state.update_job(&job_id, |object| {
        object.insert("stage".into(), serde_json::json!("cancelled"));
        object.insert("failure".into(), serde_json::json!(null));
        object.insert("speedBps".into(), serde_json::json!(null));
    });
    Ok(())
}

/// Retry a failed or cancelled download job.
#[tauri::command]
pub fn queue_retry(
    ext_state: State<'_, ExtensionState>,
    job_id: String,
) -> Result<(), String> {
    let (provider, stage) = {
        let jobs = ext_state
            .jobs
            .lock()
            .map_err(|_| "download jobs poisoned".to_string())?;
        match jobs
            .iter()
            .find(|job| job.get("id").and_then(|value| value.as_str()) == Some(&job_id))
        {
            Some(job) => (
                job.get("provider").and_then(|value| value.as_str()).unwrap_or("").to_string(),
                job.get("stage").and_then(|value| value.as_str()).unwrap_or("").to_string(),
            ),
            None => return Ok(()),
        }
    };

    if !TERMINAL_STAGES.contains(&stage.as_str()) {
        return Ok(());
    }

    // A retry must not inherit the previous attempt's cancellation flag.
    if let Ok(store) = ext_state.store.lock() {
        if let Some(runtime) = store.runtime_of(&provider) {
            runtime.host().reset_cancellation();
        }
    }

    ext_state.update_job(&job_id, |object| {
        object.insert("stage".into(), serde_json::json!("queued"));
        object.insert("percent".into(), serde_json::json!(0));
        object.insert("failure".into(), serde_json::json!(null));
        object.insert("bytesWritten".into(), serde_json::json!(0));
        object.insert("speedBps".into(), serde_json::json!(null));
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// Windows-forbidden characters plus the path separators a track title could
/// smuggle in. The sandbox already protects the package side; this is only for
/// the human-readable file name the host composes.
fn sanitize_filename(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ if (character as u32) < 0x20 => '_',
            _ => character,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The failure object `QueueJob.failure` expects.
fn failure_json(kind: &DownloadFailureKind, message: &str) -> serde_json::Value {
    serde_json::json!({
        "stage": "downloading",
        "kind": kind.as_str(),
        "message": message,
    })
}

/// Drive queued jobs until the app exits: claim, resolve, download, embed,
/// publish at every step. One job at a time — a queue is about order, not
/// parallelism.
pub fn spawn_download_worker(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let Some(state) = app.try_state::<ExtensionState>() else {
            continue;
        };
        let Some(job_id) = claim_next_job(&state) else {
            continue;
        };
        run_job(&app, &state, &job_id);
    });
}

/// Flip the oldest queued job to `resolving` and return its id, or none.
fn claim_next_job(state: &ExtensionState) -> Option<String> {
    let job_id = {
        let mut jobs = state
            .jobs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let job = jobs
            .iter_mut()
            .find(|job| job.get("stage").and_then(|value| value.as_str()) == Some("queued"))?;
        let job_id = job
            .get("id")
            .and_then(|value| value.as_str())?
            .to_string();
        job["stage"] = serde_json::json!("resolving");
        job["percent"] = serde_json::json!(0);
        job["failure"] = serde_json::json!(null);
        job["updatedAt"] = serde_json::json!(now_millis());
        job_id
    };
    state.publish_queue();
    Some(job_id)
}

/// Take a claimed job from resolving through the final stage.
fn run_job(app: &AppHandle, state: &ExtensionState, job_id: &str) {
    let job = {
        let jobs = state.jobs.lock().unwrap_or_else(|poison| poison.into_inner());
        match jobs
            .iter()
            .find(|job| job.get("id").and_then(|value| value.as_str()) == Some(job_id))
        {
            Some(job) => job.clone(),
            None => return,
        }
    };

    let provider = job.get("provider").and_then(|value| value.as_str()).unwrap_or("");
    let track_id = job.get("trackId").and_then(|value| value.as_str()).unwrap_or("");
    let quality = job.get("quality").and_then(|value| value.as_str()).unwrap_or("best");

    // Down to the real transfer: the package receives the request and runs.
    let (runtime, host) = match activate_for_download(state, provider) {
        Ok(pairs) => pairs,
        Err(message) => {
            fail_job(state, job_id, &DownloadFailureKind::InvalidInput, &message);
            return;
        }
    };

    // Optional enrichment: a package that implements `enrichTrack` gets a chance
    // to refine metadata (ISRC, tidal_id, qobuz_id, deezer_id, external_links)
    // before the download starts. Failures here must not abort the download —
    // enrichment is an optimisation, not a requirement.
    let mut enriched_metadata: Option<serde_json::Value> = None;
    let mut enriched_track_id = track_id.to_string();
    let mut enriched_isrc: Option<String> = None;
    {
        let capabilities = qext::adapter::Capabilities::from_runtime(&runtime);
        if capabilities.has("enrichTrack") {
            let artist = text_field(&job, "artist");
            let title = text_field(&job, "title");
            let album = text_field(&job, "album");
            let duration_ms = job
                .get("durationMs")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let art_url = job.get("artUrl").and_then(|value| value.as_str()).map(String::from);
            let mut images = qext::contract::OneOrMany::default();
            if let Some(url) = art_url {
                images.0.push(url);
            }
            let track = qext::contract::Track {
                id: track_id.to_string(),
                name: title.clone(),
                artists: qext::contract::OneOrMany(vec![artist.clone()]),
                album_name: if album.is_empty() { None } else { Some(album.clone()) },
                album_artist: None,
                isrc: None,
                duration_ms,
                track_number: None,
                disc_number: None,
                release_date: None,
                images,
                extra: BTreeMap::new(),
            };

            let mut cache = qext::calls::CallCache::open_in_memory(0).ok();
            let resolved = cache.as_mut().and_then(|cache| {
                let mut calls = qext::calls::Calls::new(provider, &runtime, cache);
                calls.enrich(&track).ok().flatten()
            });
            if let Some(resolved) = resolved {
                let enriched_track = resolved.item;
                enriched_metadata = serde_json::to_value(&enriched_track).ok();
                if !enriched_track.id.is_empty() {
                    enriched_track_id = enriched_track.id.clone();
                }
                enriched_isrc = enriched_track.isrc.clone();
            }
        }
    }

    let track_id = enriched_track_id;

    start_progress_watcher(app, job_id.to_string(), host.clone());
    set_stage(state, job_id, "downloading");

    // Download into a per-job workspace under the provider's data directory:
    // `file.download` resolves relative paths there, so the package can move or
    // rename its result before reporting it.
    let extension_data_dir = state.data_dir(provider);
    let work_dir = extension_data_dir.join("downloads").join(job_id);
    let staged_name = format!(
        "{} - {}.flac",
        sanitize_filename(&text_field(&job, "artist")),
        sanitize_filename(&text_field(&job, "title")),
    );
    let staged_path = work_dir.join(&staged_name);

    let request_metadata = enriched_metadata.unwrap_or_else(|| job.clone());

    let request = qext::adapter::DownloadRequest::new(
        track_id,
        quality,
        staged_path.to_string_lossy(),
    )
    .with_names(text_field(&job, "title"), text_field(&job, "artist"))
    .with_metadata(request_metadata);

    let request = if let Some(isrc) = enriched_isrc.or_else(|| job.get("isrc").and_then(|value| value.as_str()).map(String::from)) {
        request.with_isrc(isrc)
    } else {
        request
    };

    let request = if let Some(isrc) = job.get("isrc").and_then(|value| value.as_str()) {
        request.with_isrc(isrc)
    } else {
        request
    };

    let outcome = adapter::download(&runtime, &request);

    let result = match outcome {
        Ok(DownloadOutcome::Completed(result)) => result,
        Ok(DownloadOutcome::Failed(failure)) => {
            finish_running(state, provider);
            if failure.kind == DownloadFailureKind::Cancelled {
                cancel_job(state, job_id);
            } else {
                fail_job(state, job_id, &failure.kind, &failure.message);
            }
            return;
        }
        Err(error) => {
            finish_running(state, provider);
            let kind = match error {
                adapter::AdapterError::Unsupported { .. } => DownloadFailureKind::InvalidInput,
                _ => DownloadFailureKind::Unknown,
            };
            fail_job(state, job_id, &kind, &error.to_string());
            return;
        }
    };

    // Embed and file: move the finished audio to its final home, tag it there.
    set_stage(state, job_id, "embedding");

    let Some(staged) = staged_file(&extension_data_dir, result.file_path.as_deref(), &staged_path) else {
        finish_running(state, provider);
        fail_job(
            state,
            job_id,
            &DownloadFailureKind::DownloadError,
            "the provider did not report where the downloaded file is",
        );
        return;
    };

    let final_dir = state.data_root.join("downloads");
    let final_path = final_dir.join(
        staged
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "download.flac".to_string()),
    );

    if let Err(message) = relocate(&staged, &final_path) {
        finish_running(state, provider);
        fail_job(state, job_id, &DownloadFailureKind::DownloadError, &message);
        return;
    }

    // Optional post-processing: a package that declares `postProcessing` hooks
    // gets a chance to transform the finished file (convert format, normalise
    // audio, ...). Failures here do not discard the downloaded file.
    if let Some(manifest) = provider_manifest(state, provider) {
        if manifest.post_processing.as_ref().is_some_and(|pp| pp.is_active()) {
            let capabilities = qext::adapter::Capabilities::from_runtime(&runtime);
            if capabilities.has("postProcess") || capabilities.has("postProcessV2") {
                let ext = final_path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if let Some(pp) = manifest.post_processing.as_ref() {
                    for hook in &pp.hooks {
                        if !hook.default_enabled {
                            continue;
                        }
                        if !hook.supported_formats.is_empty()
                            && !hook.supported_formats.iter().any(|format| format.eq_ignore_ascii_case(&ext))
                        {
                            continue;
                        }
                        let input = serde_json::json!({
                            "path": final_path.to_string_lossy(),
                            "name": final_path.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default(),
                            "mime_type": "",
                            "size": std::fs::metadata(&final_path).map(|entry| entry.len()).unwrap_or(0),
                            "is_saf": false,
                        });
                        let metadata = job.clone();
                        let mut cache = qext::calls::CallCache::open_in_memory(0).ok();
                        if let Some(cache) = cache.as_mut() {
                            let mut calls = qext::calls::Calls::new(provider, &runtime, cache);
                            if let Ok(Some(resolved)) = calls.post_process(&input, &metadata, &hook.id) {
                                if let Some(new_path) = resolved.item.get("new_file_path").and_then(|value| value.as_str()) {
                                    let new_path = std::path::PathBuf::from(new_path);
                                    if new_path != final_path && new_path.exists() {
                                        let _ = std::fs::rename(&new_path, &final_path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let spec = qcore::embed::EmbedSpec {
        artist: Some(text_field(&job, "artist")),
        album: Some(text_field(&job, "album")),
        ..qcore::embed::EmbedSpec::default()
    };
    if let Err(error) = qcore::embed::embed(&final_path, &spec) {
        // The audio itself is fine and already in place; a tag failure must not
        // discard a downloaded file. The report goes through the shell's error
        // sink.
        eprintln!("extensions: tagging {}: {}", final_path.display(), error);
    }

    let bytes = std::fs::metadata(&final_path).map(|entry| entry.len()).unwrap_or(0);
    finish_running(state, provider);
    state.update_job(job_id, |object| {
        object.insert("stage".into(), serde_json::json!("done"));
        object.insert("percent".into(), serde_json::json!(100));
        object.insert("bytesWritten".into(), serde_json::json!(bytes));
        object.insert("bytesTotal".into(), serde_json::json!(bytes));
        object.insert("speedBps".into(), serde_json::json!(null));
        object.insert("outputPath".into(), serde_json::json!(final_path.to_string_lossy()));
    });
}

/// Reset the cancellation flag once a job reaches a terminal stage, so the
/// next attempt on this provider starts clean.
fn finish_running(state: &ExtensionState, provider: &str) {
    if let Ok(store) = state.store.lock() {
        if let Some(runtime) = store.runtime_of(provider) {
            runtime.host().reset_cancellation();
        }
    }
}

/// Activate a provider for a download call, surfacing the real problem text.
fn activate_for_download(
    state: &ExtensionState,
    provider: &str,
) -> Result<(Arc<qext::runtime::ExtensionRuntime>, Arc<HostServices>), String> {
    let host = ensure_runtime(state, provider)?;

    let store = state
        .store
        .lock()
        .map_err(|_| "extension store poisoned".to_string())?;
    let runtime = store
        .runtime_of(provider)
        .ok_or_else(|| format!("{provider} could not be activated"))?;

    let capabilities = qext::adapter::Capabilities::from_runtime(&runtime);
    if !capabilities.is_download_provider() {
        return Err(format!("{provider} does not implement download"));
    }
    Ok((runtime, host))
}

/// Each progress event's `args[0]` is the percent the package reported.
fn parse_percent(event: &serde_json::Value) -> Option<u64> {
    let arguments = event.get("args")?.as_array()?;
    arguments.first()?.as_u64()
}

/// Publish one job's percent while its download runs, at most every ~400 ms
/// between entries — the package reports progress in a tight loop and the
/// event bus should not carry every one of those.
fn start_progress_watcher(app: &AppHandle, job_id: String, host: Arc<HostServices>) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        let Some(state) = app.try_state::<ExtensionState>() else {
            return;
        };

        let is_terminal = {
            let jobs = state
                .jobs
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            match jobs
                .iter()
                .find(|job| job.get("id").and_then(|value| value.as_str()) == Some(&job_id))
            {
                Some(job) => {
                    let stage = job
                        .get("stage")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    TERMINAL_STAGES.contains(&stage)
                }
                None => true,
            }
        };

        if is_terminal {
            return;
        }

        let events = host.progress_events();
        host.clear_progress();
        if let Some(percent) = events.iter().rev().filter_map(parse_percent).next() {
            state.update_job(&job_id, |object| {
                object.insert("percent".into(), serde_json::json!(percent));
            });
        }
    });
}

fn set_stage(state: &ExtensionState, job_id: &str, stage: &str) {
    state.update_job(job_id, |object| {
        object.insert("stage".into(), serde_json::json!(stage));
    });
}

fn fail_job(state: &ExtensionState, job_id: &str, kind: &DownloadFailureKind, message: &str) {
    let failure = failure_json(kind, message);
    state.update_job(job_id, |object| {
        object.insert("stage".into(), serde_json::json!("failed"));
        object.insert("failure".into(), failure);
        object.insert("speedBps".into(), serde_json::json!(null));
    });
}

fn cancel_job(state: &ExtensionState, job_id: &str) {
    state.update_job(job_id, |object| {
        object.insert("stage".into(), serde_json::json!("cancelled"));
        object.insert("failure".into(), serde_json::json!(null));
        object.insert("speedBps".into(), serde_json::json!(null));
    });
}

fn text_field(job: &serde_json::Value, field: &str) -> String {
    job.get(field)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

/// Where the provider actually wrote the file. A relative path from a package
/// is relative to the extension data directory; an absolute one is accepted
/// only because packages write through the sandbox that already constrains
/// them. Without any report, the requested staged path is what to expect.
fn staged_file(
    extension_data_dir: &Path,
    reported: Option<&str>,
    fallback: &Path,
) -> Option<PathBuf> {
    let Some(reported) = reported else {
        return Some(fallback.to_path_buf());
    };
    let path = Path::new(reported);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(extension_data_dir.join(path))
    }
}

/// `rename` cannot cross volumes; fall back to copy-then-delete.
fn relocate(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    match std::fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(source, target)
                .and_then(|_| std::fs::remove_file(source))
                .map(|_| ())
                .map_err(|error| {
                    format!("cannot move {}: {error}", source.display())
                })
        }
    }
}
