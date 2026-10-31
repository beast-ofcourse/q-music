//! The host surface a package sees: `http`, `fetch`, `storage`, `file`, `utils`,
//! `log`, `gobackend`, `matching`, plus the text and URL builtins QuickJS lacks.
//!
//! Why this is one module: every object here is a thin JavaScript wrapper over a
//! native primitive, and the two halves have to agree exactly. Splitting the JS
//! from the Rust would put the contract in two files that drift.
//!
//! Why natives return JSON envelopes instead of throwing: a JavaScript exception
//! raised from Rust carries no structured detail, and a partial write or a
//! denied permission has to be reportable in words. Every native returns
//! `{"ok":true,"value":…}` or `{"ok":false,"error":"…"}`, and the JavaScript
//! wrapper throws from the second form. That keeps the failure text authored
//! here, where the reason is known.
//!
//! What is deliberately absent: `setTimeout`/`setInterval` (there is no event
//! loop to schedule them on), `credentials` and `auth`. Each throws a message
//! naming itself rather than pretending to work. `EXTENSIONS.md` records the
//! same list.

use crate::runtime::crypto;
use crate::runtime::ffmpeg::{ConvertOptions, Ffmpeg, FfmpegError};
use crate::runtime::http::{HostHttp, HttpCallSpec};
use crate::runtime::permissions::{FileSandbox, PermissionError};
use chrono::{Datelike, Timelike};
use rquickjs::{Ctx, Function, Object};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Name of the persisted key/value file inside the extension data directory.
pub const STORAGE_FILE: &str = "storage.json";
/// Written first, then renamed over [`STORAGE_FILE`] so a crash cannot truncate it.
pub const STORAGE_TEMP_FILE: &str = "storage.json.tmp";
/// Where an unreadable storage file is preserved rather than silently replaced.
pub const STORAGE_CORRUPT_FILE: &str = "storage.json.corrupt";
/// Ceiling for the whole storage map. The guide says "unlimited", which is not
/// a limit a host can enforce; 16 MiB is far above anything a provider needs and
/// keeps one package from filling the user's disk.
pub const MAX_STORAGE_BYTES: usize = 16 * 1024 * 1024;
/// Log lines kept per extension before the oldest is dropped.
pub const MAX_LOG_ENTRIES: usize = 512;
/// Progress events kept per extension. The queue drains them; the cap only stops
/// a package that reports progress in a tight loop from growing memory for a
/// host that has not registered a consumer yet.
pub const MAX_PROGRESS_EVENTS: usize = 64;
/// Longest `utils.sleep` will block. Without this a package could park its
/// thread for hours and no interrupt would fire, because host calls are not
/// interruptible.
pub const MAX_SLEEP_MS: u64 = 30_000;
/// What `utils.appUserAgent()` reports when the host config does not override it.
pub const DEFAULT_USER_AGENT: &str = "Q-Music";

/// User agents `utils.randomUserAgent()` picks from. Host-chosen: the guide
/// documents the function but not the pool, and providers only care that the
/// value looks like a desktop browser.
pub const USER_AGENTS: [&str; 6] = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
];

const SLEEP_SLICE: Duration = Duration::from_millis(25);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StorageError {
    #[error("this write would exceed the {limit} byte storage limit")]
    TooLarge { limit: usize },

    #[error("storage could not be written: {0}")]
    Io(String),

    #[error("storage value is not valid JSON: {0}")]
    InvalidValue(String),
}

/// The `storage` API's backing store: a JSON object in the extension data
/// directory, mirrored in memory so reads never touch the disk.
#[derive(Debug)]
pub struct Storage {
    /// `None` means "in memory only", which is what a bare host gets.
    directory: Option<PathBuf>,
    entries: Mutex<BTreeMap<String, Value>>,
}

impl Storage {
    pub fn in_memory() -> Self {
        Self {
            directory: None,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Load existing state, keeping an unreadable file aside rather than
    /// deleting it.
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        let entries = read_entries(&directory);
        Self {
            directory: Some(directory),
            entries: Mutex::new(entries),
        }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.lock().get(key).cloned()
    }

    pub fn set(&self, key: &str, value: Value) -> Result<(), StorageError> {
        let mut entries = self.lock();
        let previous = entries.insert(key.to_string(), value);

        if let Err(error) = self.persist(&entries) {
            // Keep memory and disk consistent: a value the caller was told
            // failed must not be readable afterwards.
            match previous {
                Some(previous) => {
                    entries.insert(key.to_string(), previous);
                }
                None => {
                    entries.remove(key);
                }
            }
            return Err(error);
        }

        Ok(())
    }

    pub fn remove(&self, key: &str) -> Result<(), StorageError> {
        let mut entries = self.lock();
        let Some(previous) = entries.remove(key) else {
            return Ok(());
        };

        if let Err(error) = self.persist(&entries) {
            entries.insert(key.to_string(), previous);
            return Err(error);
        }

        Ok(())
    }

    /// Number of stored keys; used by tests and diagnostics.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn persist(&self, entries: &BTreeMap<String, Value>) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec(entries)
            .map_err(|error| StorageError::InvalidValue(error.to_string()))?;

        // Enforced even for an in-memory store so the limit is a property of the
        // API rather than of one backend.
        if bytes.len() > MAX_STORAGE_BYTES {
            return Err(StorageError::TooLarge {
                limit: MAX_STORAGE_BYTES,
            });
        }

        let Some(directory) = &self.directory else {
            return Ok(());
        };

        fs::create_dir_all(directory).map_err(to_storage_error)?;

        let target = directory.join(STORAGE_FILE);
        let temporary = directory.join(STORAGE_TEMP_FILE);
        fs::write(&temporary, &bytes).map_err(to_storage_error)?;
        // `rename` replaces an existing file on every platform we target, so a
        // reader never observes a half-written storage file.
        fs::rename(&temporary, &target).map_err(to_storage_error)?;

        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Value>> {
        // Poisoning only means a previous holder panicked. The map is still
        // structurally sound because every mutation is one insert or remove.
        self.entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

fn to_storage_error(error: std::io::Error) -> StorageError {
    StorageError::Io(error.to_string())
}

fn read_entries(directory: &Path) -> BTreeMap<String, Value> {
    let Ok(text) = fs::read_to_string(directory.join(STORAGE_FILE)) else {
        return BTreeMap::new();
    };

    match serde_json::from_str::<BTreeMap<String, Value>>(&text) {
        Ok(entries) => entries,
        Err(_) => {
            // Discarding the bytes would destroy extension state the user may
            // care about (sessions, settings). Keep the file, start clean.
            let corrupt = directory.join(STORAGE_CORRUPT_FILE);
            if !corrupt.exists() {
                let _ = fs::rename(directory.join(STORAGE_FILE), corrupt);
            }
            BTreeMap::new()
        }
    }
}

/// One `log.*` call, retained for the host UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogRecord {
    pub level: String,
    pub message: String,
}

/// The clock the runtime's interrupt handler watches, exposed to `utils`.
#[derive(Debug, Clone)]
pub struct CallClock {
    started: Instant,
    deadline_ms: Arc<AtomicU64>,
}

impl CallClock {
    pub(crate) fn new(started: Instant, deadline_ms: Arc<AtomicU64>) -> Self {
        Self {
            started,
            deadline_ms,
        }
    }

    /// Milliseconds left before the current call is interrupted, or 0 when no
    /// call is running.
    pub fn remaining_ms(&self) -> u64 {
        let deadline = self.deadline_ms.load(Ordering::Relaxed);
        if deadline == 0 {
            return 0;
        }
        let elapsed = self.started.elapsed().as_millis() as u64;
        deadline.saturating_sub(elapsed)
    }
}

/// The machine's local clock, in the shape `gobackend.getLocalTime()` documents.
///
/// Computed in Rust because the guide warns that QuickJS may report a zero
/// `Date.getTimezoneOffset()`; the package's own fallback table is keyed on
/// `offsetMinutes` being `-330` for UTC+5:30, which is the `getTimezoneOffset`
/// sign convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalNow {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub weekday: u32,
    pub offset_minutes: i32,
    pub timezone: String,
    pub timestamp: i64,
}

pub fn local_now() -> LocalNow {
    let now = chrono::Local::now();

    LocalNow {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        hour: now.hour(),
        minute: now.minute(),
        second: now.second(),
        // 0 = Sunday, matching the documented contract.
        weekday: now.weekday().num_days_from_sunday(),
        offset_minutes: -(now.offset().local_minus_utc() / 60),
        // Empty when the platform cannot name the zone. The packages treat an
        // empty name as "use the offset", which is the honest fallback: a made
        // up name would be believed.
        timezone: iana_time_zone::get_timezone().unwrap_or_default(),
        timestamp: now.timestamp(),
    }
}

/// What a caller can give the host at load time.
#[derive(Debug)]
pub struct HostConfig {
    pub app_version: String,
    pub user_agent: String,
    /// Extension data directory: storage and the file-API sandbox both live here.
    pub data_dir: Option<PathBuf>,
    /// `None` makes every network call fail with a typed message.
    pub http: Option<HostHttp>,
    /// `None` makes every `ffmpeg.*` call return a typed `unsupported` error.
    pub ffmpeg: Option<Ffmpeg>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            app_version: String::new(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            data_dir: None,
            http: None,
            ffmpeg: None,
        }
    }
}

/// Everything the JavaScript surface can reach, owned by one extension.
pub struct HostServices {
    storage: Storage,
    sandbox: Option<FileSandbox>,
    http: Option<HostHttp>,
    ffmpeg: Option<Ffmpeg>,
    /// Directory a host-initiated stage opened for its duration. Post-processing
    /// hands the package absolute paths the package never chose, so `file.*`
    /// has to reach them; the grant is scoped to one stage and cleared on drop.
    stage: Mutex<Option<FileSandbox>>,
    logs: Mutex<Vec<LogRecord>>,
    progress: Mutex<Vec<Value>>,
    app_version: String,
    user_agent: String,
    cancelled: AtomicBool,
    clock: OnceLock<CallClock>,
}

impl HostServices {
    pub fn new(config: HostConfig) -> Self {
        let sandbox = config
            .data_dir
            .as_ref()
            .and_then(|directory| FileSandbox::new(directory).ok());

        let storage = match &config.data_dir {
            Some(directory) => Storage::at(directory.clone()),
            None => Storage::in_memory(),
        };

        Self {
            storage,
            sandbox,
            http: config.http,
            ffmpeg: config.ffmpeg,
            stage: Mutex::new(None),
            logs: Mutex::new(Vec::new()),
            progress: Mutex::new(Vec::new()),
            app_version: config.app_version,
            user_agent: config.user_agent,
            cancelled: AtomicBool::new(false),
            clock: OnceLock::new(),
        }
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    pub fn logs(&self) -> Vec<LogRecord> {
        self.logs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Record one event forwarded by a package's callback.
    ///
    /// Events are kept so a host that has not yet attached a consumer can still
    /// read what happened; the queue drains and clears them per job.
    pub fn record_progress(&self, event: Value) {
        let mut events = self
            .progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        if events.len() >= MAX_PROGRESS_EVENTS {
            events.remove(0);
        }

        events.push(event);
    }

    pub fn progress_events(&self) -> Vec<Value> {
        self.progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn clear_progress(&self) {
        self.progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
    }

    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub fn http(&self) -> Option<&HostHttp> {
        self.http.as_ref()
    }

    pub fn ffmpeg(&self) -> Option<&Ffmpeg> {
        self.ffmpeg.as_ref()
    }

    pub fn sandbox(&self) -> Option<&FileSandbox> {
        self.sandbox.as_ref()
    }

    /// Open one directory to `file.*` for the duration of a host-initiated
    /// stage.
    ///
    /// The guard clears the grant when it drops, so a package cannot keep
    /// reaching a download directory after the stage that handed it the file.
    pub fn open_stage(&self, directory: &Path) -> Result<StageAccess<'_>, String> {
        let sandbox = FileSandbox::new(directory)
            .map_err(|error| format!("cannot open {}: {error}", directory.display()))?;

        let mut stage = self.stage.lock().unwrap_or_else(|poison| poison.into_inner());
        *stage = Some(sandbox);
        Ok(StageAccess { host: self })
    }

    fn close_stage(&self) {
        let mut stage = self.stage.lock().unwrap_or_else(|poison| poison.into_inner());
        *stage = None;
    }

    /// Ask the extension to abandon its current download. `utils.sleep` returns
    /// `false` from this point and `utils.isDownloadCancelled()` reports true.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn reset_cancellation(&self) {
        self.cancelled.store(false, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// The shared cancellation flag itself, so a host-initiated transfer stops
    /// the instant `cancel` is called from any thread, not just when package
    /// code next asks `utils`.
    pub fn cancellation_flag(&self) -> &AtomicBool {
        &self.cancelled
    }

    /// Milliseconds left in the current call, or 0 outside a call.
    pub fn remaining_ms(&self) -> u64 {
        self.clock.get().map(CallClock::remaining_ms).unwrap_or(0)
    }

    /// Called by the runtime before the context is created: the runtime owns the
    /// deadline, the host only reads it.
    pub(crate) fn bind_clock(&self, clock: CallClock) {
        let _ = self.clock.set(clock);
    }

    fn record_log(&self, level: &str, message: String) {
        let mut logs = self
            .logs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        if logs.len() >= MAX_LOG_ENTRIES {
            logs.remove(0);
        }

        logs.push(LogRecord {
            level: level.to_string(),
            message,
        });
    }

    /// Sleep, waking early when the extension is cancelled. Returns false iff
    /// cancellation was observed.
    fn sleep_ms(&self, milliseconds: u64) -> bool {
        let total = Duration::from_millis(milliseconds.min(MAX_SLEEP_MS));
        let started = Instant::now();

        while started.elapsed() < total {
            if self.is_cancelled() {
                return false;
            }
            let remaining = total.saturating_sub(started.elapsed());
            thread::sleep(remaining.min(SLEEP_SLICE));
        }

        !self.is_cancelled()
    }

    fn random_user_agent(&self) -> &'static str {
        let index = crypto::random_bytes(4)
            .ok()
            .map(|bytes| {
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
                    % USER_AGENTS.len()
            })
            .unwrap_or(0);

        USER_AGENTS[index]
    }

    /// Resolve a `file.*` path inside the extension data directory, or inside
    /// the stage directory when a host-initiated stage is open.
    fn resolve_file(&self, requested: &str) -> Result<PathBuf, String> {
        if Path::new(requested).is_absolute() {
            let stage = self.stage.lock().unwrap_or_else(|poison| poison.into_inner());
            return match stage.as_ref() {
                Some(sandbox) => sandbox
                    .resolve_host_path(Path::new(requested))
                    .map_err(|error| error.to_string()),
                None => Err(PermissionError::AbsolutePathNotAllowed {
                    path: requested.to_string(),
                }
                .to_string()),
            };
        }

        let Some(sandbox) = &self.sandbox else {
            return Err("file access is not available for this extension".to_string());
        };

        sandbox
            .resolve(requested)
            .map_err(|error| error.to_string())
    }
}

/// The engine behind `file.download` and `file.downloadSegments`.
///
/// The transfer mechanics are `qext::download`'s: one engine whose byte
/// ceiling, resume handling, cancellation checks and progress throttling are
/// already tested. This module only maps package-shaped arguments into
/// `TransferPlan`s and the outcome back into the object the shipped packages
/// read. Untrusted code chooses the target path (resolved through the file
/// sandbox here, never in the engine) and the segment list; it cannot choose
/// the URL check or where partial bytes live.
pub mod transfer {
    use crate::download::{self, Downloader, TransferError, TransferLimits, TransferPlan};
    use crate::runtime::crypto;
    use serde_json::json;
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::path::{Path, PathBuf};

    use super::HostServices;

    /// A segment manifest lists one URL per media fragment. A hostile list
    /// should not be able to pin the runtime thread down for its full patience,
    /// but real manifests are short: an initialisation fragment plus media.
    pub const MAX_SEGMENT_COUNT: usize = 4096;

    /// A failure in package words. `kind` matches the `error_type` strings the
    /// shipped packages already branch on; the message `download cancelled` is
    /// what `tidal-web` scans for.
    pub struct TransferFailure {
        pub message: String,
        pub kind: &'static str,
    }

    /// Bytes that ended up inside the final file.
    pub type TransferOutcome = Result<u64, TransferFailure>;

    /// Deterministic per-address temp suffix, so a re-run of the same address
    /// continues from its own partial bytes and no one else's. Hex only, and
    /// short enough for the job-id rules `download.rs` enforces.
    pub(crate) fn job_id_for(url: &str, tracked: &str) -> String {
        crypto::sha256_hex(format!("{tracked}\n{url}").as_bytes())[..32].to_string()
    }

    /// Package words for one transfer error.
    fn deny(error: &TransferError) -> TransferFailure {
        let mut message = error.to_string();
        let kind = match error {
            TransferError::InvalidUrl { .. } | TransferError::InvalidJobId { .. } => "invalid_input",
            TransferError::Denied { .. } => "forbidden",
            TransferError::Http { status: 404, .. } => "not_found",
            TransferError::Http { status: 429, .. } => "rate_limited",
            TransferError::Http { .. } => "http_error",
            TransferError::Timeout { .. } => "timeout",
            TransferError::TooLarge { .. } => "too_large",
            TransferError::RangeMismatch { .. } => "range_mismatch",
            TransferError::ShortBody { .. } => "incomplete",
            TransferError::Cancelled => {
                message = "download cancelled".to_string();
                "cancelled"
            }
            TransferError::Io { .. } => "io_error",
            TransferError::Transport(_) => "download_error",
        };
        TransferFailure { message, kind }
    }

    /// A transfer inherits its read timeout from the host's `http` permission
    /// so a package never stalls longer than its own network calls would.
    fn limits_for(host: &HostServices) -> TransferLimits {
        match host.http() {
            Some(http) => TransferLimits {
                read_timeout: http.limits().timeout,
                ..TransferLimits::default()
            },
            None => TransferLimits::default(),
        }
    }

    fn remove_quietly(path: &Path) {
        let _ = fs::remove_file(path);
    }

    /// Stream one URL into `output`, which the caller has already resolved
    /// through the file sandbox. Progress reports `(written, total)` with
    /// `total` zero when the server sent no `Content-Length`.
    pub fn fetch_one(
        host: &HostServices,
        url: &str,
        output: &Path,
        headers: &[(String, String)],
        resume: bool,
        report_progress: &mut dyn FnMut(u64, u64),
    ) -> TransferOutcome {
        let Some(http) = host.http() else {
            return Err(TransferFailure {
                message: "file.download is unavailable: this host grants no network access"
                    .to_string(),
                kind: "unavailable",
            });
        };

        let job_id = job_id_for(url, &output.to_string_lossy());
        let partial = download::partial_path(output, &job_id).map_err(|error| deny(&error))?;
        let resume_from = if resume {
            fs::metadata(&partial).map(|entry| entry.len()).unwrap_or(0)
        } else {
            0
        };

        let mut plan = TransferPlan::new(url, output, &job_id).resuming_from(resume_from);
        for (name, value) in headers {
            plan = plan.with_header(name.clone(), value.clone());
        }

        let downloader = Downloader::new(limits_for(host)).with_allowlist(http.allowlist().clone());
        downloader
            .transfer(&plan, host.cancellation_flag(), |update| {
                report_progress(update.written, update.total.unwrap_or(0));
            })
            .map(|report| report.bytes_written)
            .map_err(|error| deny(&error))
    }

    /// Stream a segment manifest into one file: each URL is fetched to a
    /// scratch file, appended to a shared partial in order, and then dropped.
    /// With `persistentCheckpoint` a re-run that survived recordings skips the
    /// segments already inside the partial rather than re-fetching them.
    pub fn fetch_segments(
        host: &HostServices,
        urls: &[String],
        output: &Path,
        headers: &[(String, String)],
        persistent_checkpoint: bool,
        report_progress: &mut dyn FnMut(u64, u64, u64, u64),
    ) -> TransferOutcome {
        if urls.is_empty() {
            return Err(TransferFailure {
                message: "no segment urls to download".to_string(),
                kind: "invalid_input",
            });
        }
        if urls.len() > MAX_SEGMENT_COUNT {
            return Err(TransferFailure {
                message: format!("segment list is longer than the {MAX_SEGMENT_COUNT} urls accepted"),
                kind: "too_large",
            });
        }

        let job_id = job_id_for(&urls[0], &output.to_string_lossy());
        let partial = download::partial_path(output, &job_id).map_err(|error| deny(&error))?;
        let checkpoint = sibling(&partial, format!("{}.ck", job_id));

        let (mut completed, mut written) = carried_progress(
            persistent_checkpoint.then_some(checkpoint.as_path()),
            &partial,
            urls.len(),
        );

        let total = urls.len() as u64;
        let mut index = completed;
        while index < total {
            if host.is_cancelled() {
                return Err(deny(&TransferError::Cancelled));
            }

            let scratch = sibling(&partial, format!("{}.seg{index}", job_id));
            let scratch_partial = download::partial_path(&scratch, &job_id).map_err(|error| deny(&error))?;
            let mut appended = 0u64;
            let fetched = fetch_one(host, &urls[usize::try_from(index).unwrap_or(0)], &scratch, headers, false, &mut |chunk, _total| {
                appended = chunk;
                report_progress(written + chunk, 0, index + 1, total);
            });

            if let Err(failure) = fetched {
                remove_quietly(&scratch_partial);
                remove_quietly(&scratch);
                // A cancelled transfer keeps its recording: the caller is
                // pausing, not failing, and the next attempt continues there.
                if failure.kind != "cancelled" && !persistent_checkpoint {
                    remove_quietly(&partial);
                    remove_quietly(&checkpoint);
                }
                return Err(failure);
            }

            if let Some(failure) = append_file(&scratch, &partial) {
                remove_quietly(&scratch);
                return Err(failure);
            }
            remove_quietly(&scratch);

            written += appended;
            completed = index + 1;
            if persistent_checkpoint {
                let _ = fs::write(
                    &checkpoint,
                    json!({ "completed": completed, "bytes": written }).to_string(),
                );
            }
            report_progress(written, 0, completed, total);
            index += 1;
        }

        // Hand the file over under its real name; the partial naming is an
        // engine detail no package ever reads.
        fs::rename(&partial, output).map_err(|error| TransferFailure {
            message: format!("cannot finish {}: {error}", output.display()),
            kind: "io_error",
        })?;
        remove_quietly(&checkpoint);

        Ok(written)
    }

    /// `<name>.ck` next to one partial: where the segment recording lives.
    fn sibling(partial: &Path, suffix: String) -> PathBuf {
        let mut path = partial.to_path_buf();
        let name = path
            .file_name()
            .map(|entry| entry.to_string_lossy().into_owned())
            .unwrap_or_default();
        path.set_file_name(format!("{name}.{suffix}"));
        path
    }

    /// How much the partial already holds, and which segments that covers.
    /// A recording whose byte count disagrees with the partial is stale; start
    /// from zero rather than append a second copy of the same segment.
    fn carried_progress(
        checkpoint: Option<&Path>,
        partial: &Path,
        segment_total: usize,
    ) -> (u64, u64) {
        let Some(checkpoint) = checkpoint else {
            return (0, 0);
        };
        let Ok(text) = fs::read_to_string(checkpoint) else {
            return (0, 0);
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return (0, 0);
        };

        let recorded_done = value
            .get("completed")
            .and_then(|entry| entry.as_u64())
            .unwrap_or(0);
        let recorded_bytes = value
            .get("bytes")
            .and_then(|entry| entry.as_u64())
            .unwrap_or(0);
        let partial_bytes = fs::metadata(partial).map(|entry| entry.len()).unwrap_or(0);

        if recorded_done <= segment_total as u64 && partial_bytes == recorded_bytes {
            (recorded_done, recorded_bytes)
        } else {
            (0, 0)
        }
    }

    /// Append every byte of one scratch file to the shared partial.
    fn append_file(source: &Path, target: &Path) -> Option<TransferFailure> {
        let appending = OpenOptions::new().create(true).append(true).open(target);
        let mut appending = match appending {
            Ok(file) => file,
            Err(error) => {
                return Some(TransferFailure {
                    message: format!("cannot open {}: {error}", target.display()),
                    kind: "io_error",
                });
            }
        };
        let mut reading = match File::open(source) {
            Ok(file) => file,
            Err(error) => {
                return Some(TransferFailure {
                    message: format!("cannot read downloaded segment: {error}"),
                    kind: "io_error",
                });
            }
        };

        match io::copy(&mut reading, &mut appending) {
            Ok(_) => None,
            Err(error) => Some(TransferFailure {
                message: format!("cannot append segment bytes: {error}"),
                kind: "io_error",
            }),
        }
    }
}

/// `options` as the JS wrapper reduces them: headers, resume flags.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferOptions {
    #[serde(default)]
    headers: Option<Value>,
    #[serde(default)]
    resume: bool,
    #[serde(default)]
    persistent_checkpoint: bool,
}

/// Parsed transfer options: the headers a package sent plus the two flags the
/// transfer engine acts on.
#[derive(Debug, Default)]
struct TransferParams {
    headers: Vec<(String, String)>,
    resume: bool,
    persistent_checkpoint: bool,
}

/// Parse the options the JS wrapper serialised, tolerating an absent one
/// entirely. Non-string header values are dropped rather than rejected: the
/// packages send plain objects with plain strings.
fn parse_transfer_options(text: &str) -> Result<TransferParams, String> {
    let options: TransferOptions =
        serde_json::from_str(text).map_err(|error| format!("transfer options are invalid: {error}"))?;

    let mut headers = Vec::new();
    if let Some(Value::Object(map)) = &options.headers {
        for (name, value) in map {
            let Some(text) = value.as_str() else {
                continue;
            };
            if name.trim().is_empty() {
                return Err("a transfer header name is empty".to_string());
            }
            headers.push((name.clone(), text.to_string()));
        }
    }

    Ok(TransferParams {
        headers,
        resume: options.resume,
        persistent_checkpoint: options.persistent_checkpoint,
    })
}

/// The package-facing result for one host-side transfer.
fn transfer_envelope(requested: &str, outcome: transfer::TransferOutcome) -> String {
    match outcome {
        Ok(bytes) => ok_envelope(serde_json::json!({
            "success": true,
            "path": requested,
            "bytes": bytes,
        })),
        Err(failure) => ok_envelope(serde_json::json!({
            "success": false,
            "path": requested,
            "error": failure.message,
            "error_type": failure.kind,
        })),
    }
}

/// A directory opened to `file.*` for one host-initiated stage.
pub struct StageAccess<'a> {
    host: &'a HostServices,
}

impl Drop for StageAccess<'_> {
    fn drop(&mut self) {
        self.host.close_stage();
    }
}

impl Default for HostServices {
    fn default() -> Self {
        Self::new(HostConfig::default())
    }
}

fn ok_envelope(value: Value) -> String {
    serde_json::json!({ "ok": true, "value": value }).to_string()
}

fn error_envelope(message: impl Into<String>) -> String {
    serde_json::json!({ "ok": false, "error": message.into() }).to_string()
}

/// Register every native primitive and evaluate the JavaScript surface.
///
/// Must run inside `context.with` and before the package source is evaluated.
pub fn install<'js>(ctx: &Ctx<'js>, host: Arc<HostServices>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    // ---- storage ---------------------------------------------------------
    let get_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_storage_get", move |key: String| -> String {
        match get_host.storage().get(&key) {
            Some(value) => ok_envelope(value),
            None => ok_envelope(Value::Null),
        }
    })?;

    let set_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_storage_set",
        move |key: String, json: String| -> String {
            let value: Value = match serde_json::from_str(&json) {
                Ok(value) => value,
                Err(error) => return error_envelope(format!("value is not valid JSON: {error}")),
            };

            match set_host.storage().set(&key, value) {
                Ok(()) => ok_envelope(Value::Bool(true)),
                Err(error) => error_envelope(error.to_string()),
            }
        },
    )?;

    let remove_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_storage_remove",
        move |key: String| -> String {
            match remove_host.storage().remove(&key) {
                Ok(()) => ok_envelope(Value::Bool(true)),
                Err(error) => error_envelope(error.to_string()),
            }
        },
    )?;

    // ---- logging ---------------------------------------------------------
    let log_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_log",
        move |level: String, message: String| -> bool {
            log_host.record_log(&level, message);
            true
        },
    )?;

    // ---- progress --------------------------------------------------------
    // Called by the callback the prelude hands to a package; see
    // `adapter::CALLBACK_MARKER`.
    let progress_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_progress", move |payload: String| -> bool {
        match serde_json::from_str::<Value>(&payload) {
            Ok(event) => {
                progress_host.record_progress(event);
                true
            }
            Err(_) => false,
        }
    })?;

    // ---- crypto and encoding --------------------------------------------
    bind(ctx, &globals, "__qext_sha256", |text: String| -> String {
        ok_envelope(Value::String(crypto::sha256_hex(text.as_bytes())))
    })?;

    bind(ctx, &globals, "__qext_md5", |text: String| -> String {
        ok_envelope(Value::String(crypto::md5_hex(text.as_bytes())))
    })?;

    bind(
        ctx,
        &globals,
        "__qext_hmac_sha256",
        |key: String, message: String| -> String {
            ok_envelope(Value::String(crypto::hmac_sha256_hex(
                key.as_bytes(),
                message.as_bytes(),
            )))
        },
    )?;

    bind(
        ctx,
        &globals,
        "__qext_hmac_sha256_base64",
        |key: String, message: String| -> String {
            ok_envelope(Value::String(crypto::hmac_sha256_base64(
                key.as_bytes(),
                message.as_bytes(),
            )))
        },
    )?;

    bind(
        ctx,
        &globals,
        "__qext_hmac_sha1",
        |key_json: String, message_json: String| -> String {
            let key: Vec<u8> = match serde_json::from_str(&key_json) {
                Ok(bytes) => bytes,
                Err(error) => return error_envelope(format!("key is not a byte array: {error}")),
            };
            let message: Vec<u8> = match serde_json::from_str(&message_json) {
                Ok(bytes) => bytes,
                Err(error) => return error_envelope(format!("message is not a byte array: {error}")),
            };

            ok_envelope(serde_json::json!(crypto::hmac_sha1(&key, &message)))
        },
    )?;

    bind(ctx, &globals, "__qext_base64_encode", |text: String| -> String {
        ok_envelope(Value::String(crypto::base64_encode(text.as_bytes())))
    })?;

    bind(ctx, &globals, "__qext_base64_decode", |text: String| -> String {
        match crypto::base64_decode(&text) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(decoded) => ok_envelope(Value::String(decoded)),
                Err(_) => error_envelope("decoded bytes are not valid UTF-8 text"),
            },
            Err(error) => error_envelope(error.to_string()),
        }
    })?;

    bind(
        ctx,
        &globals,
        "__qext_encrypt",
        |text: String, key: String| -> String {
            match crypto::encrypt(text.as_bytes(), &key) {
                Ok(sealed) => ok_envelope(Value::String(sealed)),
                Err(error) => error_envelope(error.to_string()),
            }
        },
    )?;

    bind(
        ctx,
        &globals,
        "__qext_decrypt",
        |sealed: String, key: String| -> String {
            match crypto::decrypt(&sealed, &key) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => ok_envelope(Value::String(text)),
                    Err(_) => error_envelope("decrypted bytes are not valid UTF-8 text"),
                },
                Err(error) => error_envelope(error.to_string()),
            }
        },
    )?;

    bind(ctx, &globals, "__qext_generate_key", |length: u32| -> String {
        match crypto::generate_key(length as usize) {
            Ok((base64, hex)) => ok_envelope(serde_json::json!({ "base64": base64, "hex": hex })),
            Err(error) => error_envelope(error.to_string()),
        }
    })?;

    // ---- host facts ------------------------------------------------------
    bind(ctx, &globals, "__qext_local_time", || -> String {
        match serde_json::to_string(&local_now()) {
            Ok(encoded) => ok_envelope(Value::String(encoded)),
            Err(error) => error_envelope(format!("cannot encode local time: {error}")),
        }
    })?;

    let version_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_app_version", move || -> String {
        version_host.app_version().to_string()
    })?;

    let agent_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_user_agent", move || -> String {
        agent_host.user_agent().to_string()
    })?;

    let random_agent_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_random_user_agent", move || -> String {
        random_agent_host.random_user_agent().to_string()
    })?;

    // ---- cancellation and time budget -----------------------------------
    let sleep_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_sleep", move |milliseconds: f64| -> bool {
        let bounded = if milliseconds.is_finite() && milliseconds > 0.0 {
            milliseconds as u64
        } else {
            0
        };
        sleep_host.sleep_ms(bounded)
    })?;

    let cancel_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_is_cancelled", move || -> bool {
        cancel_host.is_cancelled()
    })?;

    let clock_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_remaining_ms", move || -> f64 {
        clock_host.remaining_ms() as f64
    })?;

    // ---- network ---------------------------------------------------------
    let http_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_http", move |spec_json: String| -> String {
        let Some(http) = http_host.http() else {
            return error_envelope("network access is not available for this extension");
        };

        let spec: HttpCallSpec = match serde_json::from_str(&spec_json) {
            Ok(spec) => spec,
            Err(error) => return error_envelope(format!("invalid request: {error}")),
        };

        match spec.execute(http) {
            Ok(response) => match serde_json::to_value(&response) {
                Ok(value) => ok_envelope(value),
                Err(error) => error_envelope(format!("cannot encode response: {error}")),
            },
            Err(error) => error_envelope(error.to_string()),
        }
    })?;

    let cookie_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_clear_cookies", move || -> bool {
        match cookie_host.http() {
            Some(http) => {
                http.clear_cookies();
                true
            }
            None => false,
        }
    })?;

    // ---- files -----------------------------------------------------------
    let exists_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_file_exists", move |path: String| -> bool {
        match exists_host.resolve_file(&path) {
            Ok(resolved) => resolved.is_file(),
            Err(_) => false,
        }
    })?;

    let read_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_file_read", move |path: String| -> String {
        let resolved = match read_host.resolve_file(&path) {
            Ok(resolved) => resolved,
            Err(error) => return error_envelope(error),
        };

        match fs::read_to_string(&resolved) {
            Ok(text) => ok_envelope(Value::String(text)),
            Err(error) => error_envelope(format!("cannot read {path:?}: {error}")),
        }
    })?;

    let write_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_file_write",
        move |path: String, text: String| -> String {
            let resolved = match write_host.resolve_file(&path) {
                Ok(resolved) => resolved,
                Err(error) => return error_envelope(error),
            };

            if let Some(parent) = resolved.parent() {
                if let Err(error) = fs::create_dir_all(parent) {
                    return error_envelope(format!("cannot create directory for {path:?}: {error}"));
                }
            }

            match fs::write(&resolved, text) {
                Ok(()) => ok_envelope(Value::Bool(true)),
                Err(error) => error_envelope(format!("cannot write {path:?}: {error}")),
            }
        },
    )?;

    let delete_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_file_delete", move |path: String| -> String {
        let resolved = match delete_host.resolve_file(&path) {
            Ok(resolved) => resolved,
            Err(error) => return error_envelope(error),
        };

        match fs::remove_file(&resolved) {
            Ok(()) => ok_envelope(Value::Bool(true)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ok_envelope(Value::Bool(true))
            }
            Err(error) => error_envelope(format!("cannot delete {path:?}: {error}")),
        }
    })?;

    let move_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_file_move",
        move |from: String, to: String| -> String {            let source = match move_host.resolve_file(&from) {
                Ok(resolved) => resolved,
                Err(error) => return error_envelope(error),
            };
            let target = match move_host.resolve_file(&to) {
                Ok(resolved) => resolved,
                Err(error) => return error_envelope(error),
            };

            if let Some(parent) = target.parent() {
                if let Err(error) = fs::create_dir_all(parent) {
                    return error_envelope(format!("cannot create directory for {to:?}: {error}"));
                }
            }

            match fs::rename(&source, &target) {
                Ok(()) => ok_envelope(Value::Bool(true)),
                // A rename across volumes is not possible; copy then drop the
                // source. The original error is reported if even that fails.
                Err(rename_error) => match fs::copy(&source, &target) {
                    Ok(_) => match fs::remove_file(&source) {
                        Ok(()) => ok_envelope(Value::Bool(true)),
                        Err(error) => error_envelope(format!(
                            "copied {from:?} but cannot remove it: {error}"
                        )),
                    },
                    Err(_) => error_envelope(format!("cannot move {from:?} to {to:?}: {rename_error}")),
                },
            }
        },
    )?;

    // ---- file transfers ----------------------------------------------------
    // The real byte streaming behind `file.download` / `file.downloadSegments`.
    // The natives return the outcome object the packages read; the JS wrapper
    // keeps the progress callback out of the JSON bridge, since a function has
    // no meaning there.
    let transfer_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_file_download",
        move |url: String,
              path: String,
              options_json: String,
              on_progress: Option<Function<'js>>|
              -> String {
            let params = match parse_transfer_options(&options_json) {
                Ok(parsed) => parsed,
                Err(error) => return error_envelope(error),
            };
            let resolved = match transfer_host.resolve_file(&path) {
                Ok(resolved) => resolved,
                Err(error) => return error_envelope(error),
            };

            let result = transfer::fetch_one(
                &transfer_host,
                &url,
                &resolved,
                &params.headers,
                params.resume,
                &mut |written, total| {
                    if let Some(report) = &on_progress {
                        // A progress listener's return value has no meaning to
                        // anyone, so a coercion failure is dropped, not fatal.
                        let _ = report.call::<_, ()>((written, total));
                    }
                },
            );
            transfer_envelope(&path, result)
        },
    )?;

    let segments_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_file_download_segments",
        move |urls_json: String,
              path: String,
              options_json: String,
              on_progress: Option<Function<'js>>|
              -> String {
            let urls: Vec<String> = match serde_json::from_str(&urls_json) {
                Ok(urls) => urls,
                Err(_) => return error_envelope("segment urls must be a list of strings"),
            };
            let params = match parse_transfer_options(&options_json) {
                Ok(parsed) => parsed,
                Err(error) => return error_envelope(error),
            };
            let resolved = match segments_host.resolve_file(&path) {
                Ok(resolved) => resolved,
                Err(error) => return error_envelope(error),
            };

            let result = transfer::fetch_segments(
                &segments_host,
                &urls,
                &resolved,
                &params.headers,
                params.persistent_checkpoint,
                &mut |written, total, segments_done, segments_total| {
                    if let Some(report) = &on_progress {
                        let _ = report.call::<_, ()>((written, total, segments_done, segments_total));
                    }
                },
            );
            transfer_envelope(&path, result)
        },
    )?;

    // ---- ffmpeg ----------------------------------------------------------
    // Only present when the host configured a binary for a package that declared
    // post-processing hooks; otherwise every call is a typed `unsupported`.
    let execute_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_ffmpeg_execute",
        move |arguments: String| -> String {
            let Some(ffmpeg) = execute_host.ffmpeg() else {
                return typed_error_envelope("unsupported", FfmpegError::Unsupported.to_string());
            };

            match ffmpeg.execute(&arguments) {
                Ok(output) => ok_envelope(serde_json::json!({ "success": true, "output": output })),
                Err(error) => ffmpeg_result_envelope(error),
            }
        },
    )?;

    let convert_host = Arc::clone(&host);
    bind(
        ctx,
        &globals,
        "__qext_ffmpeg_convert",
        move |input: String, output: String, options_json: String| -> String {
            let Some(ffmpeg) = convert_host.ffmpeg() else {
                return typed_error_envelope("unsupported", FfmpegError::Unsupported.to_string());
            };

            let options: ConvertOptions = match serde_json::from_str(&options_json) {
                Ok(options) => options,
                Err(error) => {
                    return typed_error_envelope(
                        "invalid_arguments",
                        format!("conversion options are not valid JSON: {error}"),
                    )
                }
            };

            match ffmpeg.convert(&input, &output, &options) {
                Ok(output) => ok_envelope(serde_json::json!({ "success": true, "output": output })),
                Err(error) => ffmpeg_result_envelope(error),
            }
        },
    )?;

    // `getInfo` is documented but not implemented: the host's own tag reader
    // already answers these questions, and a second implementation in ffmpeg
    // could disagree with it.
    let info_host = Arc::clone(&host);
    bind(ctx, &globals, "__qext_ffmpeg_info", move |path: String| -> String {
        let Some(ffmpeg) = info_host.ffmpeg() else {
            return typed_error_envelope("unsupported", "ffmpeg is not available in this host".to_string());
        };
        match ffmpeg.info(&path) {
            Ok(info) => ok_envelope(serde_json::json!({
                "success": true,
                "bit_depth": info.bit_depth,
                "sample_rate": info.sample_rate,
                "duration": info.duration_seconds,
            })),
            Err(error) => ffmpeg_result_envelope(error),
        }
    })?;

    ctx.eval::<(), _>(HOST_PRELUDE)?;
    Ok(())
}

/// An error the host can type precisely, as opposed to a package-shaped result.
fn typed_error_envelope(kind: &str, message: impl Into<String>) -> String {
    serde_json::json!({ "ok": false, "kind": kind, "error": message.into() }).to_string()
}

/// A failed ffmpeg run is a normal result the package inspects, not a throw:
/// the documented shape is `{success:false, error}`.
fn ffmpeg_result_envelope(error: FfmpegError) -> String {
    ok_envelope(serde_json::json!({
        "success": false,
        "error": error.message(),
        "error_type": error.kind(),
    }))
}

fn bind<'js, F, P>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    name: &str,
    body: F,
) -> rquickjs::Result<()>
where
    F: rquickjs::function::IntoJsFunc<'js, P> + 'js,
{
    globals.set(name, Function::new(ctx.clone(), body)?)
}

/// The JavaScript surface. Evaluated after the native primitives are bound.
const HOST_PRELUDE: &str = r#"
// ==================== envelope helpers ====================

function __qext_unavailable(what) {
  throw new Error(what + ' is not available in this host');
}

function __qext_unwrap(reply) {
  var parsed = JSON.parse(String(reply));
  if (!parsed || parsed.ok !== true) {
    throw new Error((parsed && parsed.error) || 'host call failed');
  }
  return parsed.value;
}

// ffmpeg keeps the documented {success, output|error} shape. A host refusal
// (no binary configured) carries error_type so a hook can report a reason
// instead of a mystery failure.
function __qext_ffmpeg_result(reply) {
  var parsed = JSON.parse(String(reply));
  if (!parsed || parsed.ok !== true) {
    return {
      success: false,
      error: (parsed && parsed.error) || 'ffmpeg call failed',
      error_type: (parsed && parsed.kind) || 'ffmpeg_error'
    };
  }
  return parsed.value;
}

function __qext_text(value) {
  if (value === null || value === undefined) return '';
  return String(value);
}

// Accepts a string, a Uint8Array or a plain array of byte numbers.
function __qext_bytes(value) {
  if (value === null || value === undefined) return [];
  if (typeof value === 'string') {
    var encoded = new TextEncoder().encode(value);
    var fromText = [];
    for (var i = 0; i < encoded.length; i++) fromText.push(encoded[i]);
    return fromText;
  }
  var bytes = [];
  for (var j = 0; j < value.length; j++) bytes.push(Number(value[j]) & 0xff);
  return bytes;
}

// ==================== text encoding ====================

function TextEncoder() {
  this.encoding = 'utf-8';
}

TextEncoder.prototype.encode = function (input) {
  var text = input === undefined || input === null ? '' : String(input);
  var bytes = [];

  for (var i = 0; i < text.length; i++) {
    var code = text.charCodeAt(i);

    if (code < 0x80) { bytes.push(code); continue; }
    if (code < 0x800) { bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f)); continue; }

    if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length) {
      var low = text.charCodeAt(i + 1);
      if (low >= 0xdc00 && low <= 0xdfff) {
        var point = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
        bytes.push(
          0xf0 | (point >> 18),
          0x80 | ((point >> 12) & 0x3f),
          0x80 | ((point >> 6) & 0x3f),
          0x80 | (point & 0x3f)
        );
        i++;
        continue;
      }
    }

    // A lone surrogate cannot be encoded; the standard substitute is U+FFFD.
    if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
    bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
  }

  return new Uint8Array(bytes);
};

function TextDecoder(encoding) {
  this.encoding = String(encoding || 'utf-8').toLowerCase();
}

TextDecoder.prototype.decode = function (input) {
  if (input === null || input === undefined) return '';
  var bytes = input;
  var out = '';
  var i = 0;

  while (i < bytes.length) {
    var first = bytes[i];
    var point = 0xfffd;
    var width = 1;

    if (first < 0x80) {
      point = first;
    } else if ((first & 0xe0) === 0xc0) {
      width = 2;
      point = first & 0x1f;
    } else if ((first & 0xf0) === 0xe0) {
      width = 3;
      point = first & 0x0f;
    } else if ((first & 0xf8) === 0xf0) {
      width = 4;
      point = first & 0x07;
    } else {
      out += '\ufffd';
      i += 1;
      continue;
    }

    if (width > 1 && i + width > bytes.length) {
      out += '\ufffd';
      i += 1;
      continue;
    }

    var valid = true;
    for (var k = 1; k < width; k++) {
      var next = bytes[i + k];
      if ((next & 0xc0) !== 0x80) { valid = false; break; }
      point = (point << 6) | (next & 0x3f);
    }

    if (!valid || point > 0x10ffff) {
      out += '\ufffd';
      i += 1;
      continue;
    }

    i += width;

    if (point > 0xffff) {
      var shifted = point - 0x10000;
      out += String.fromCharCode(0xd800 + (shifted >> 10), 0xdc00 + (shifted & 0x3ff));
    } else {
      out += String.fromCharCode(point);
    }
  }

  return out;
};

// ==================== URLSearchParams ====================

function __qext_form_encode(value) {
  return encodeURIComponent(String(value))
    .replace(/[!'()~]/g, function (character) {
      return '%' + character.charCodeAt(0).toString(16).toUpperCase();
    })
    .replace(/%20/g, '+');
}

function __qext_form_decode(value) {
  return decodeURIComponent(String(value).replace(/\+/g, ' '));
}

function URLSearchParams(init) {
  this._pairs = [];
  if (init === null || init === undefined) return;

  if (typeof init === 'string') {
    var text = init.charAt(0) === '?' ? init.slice(1) : init;
    if (!text) return;
    var pieces = text.split('&');
    for (var i = 0; i < pieces.length; i++) {
      if (!pieces[i]) continue;
      var split = pieces[i].indexOf('=');
      var name = split === -1 ? pieces[i] : pieces[i].slice(0, split);
      var value = split === -1 ? '' : pieces[i].slice(split + 1);
      this._pairs.push([__qext_form_decode(name), __qext_form_decode(value)]);
    }
    return;
  }

  if (typeof init === 'object') {
    if (typeof init.length === 'number') {
      for (var j = 0; j < init.length; j++) this.append(init[j][0], init[j][1]);
      return;
    }
    for (var key in init) {
      if (Object.prototype.hasOwnProperty.call(init, key)) this.append(key, init[key]);
    }
  }
}

URLSearchParams.prototype.append = function (name, value) {
  this._pairs.push([String(name), String(value)]);
};

URLSearchParams.prototype.set = function (name, value) {
  var key = String(name);
  var kept = [];
  var replaced = false;
  for (var i = 0; i < this._pairs.length; i++) {
    if (this._pairs[i][0] === key) {
      if (!replaced) {
        kept.push([key, String(value)]);
        replaced = true;
      }
      continue;
    }
    kept.push(this._pairs[i]);
  }
  if (!replaced) kept.push([key, String(value)]);
  this._pairs = kept;
};

URLSearchParams.prototype.get = function (name) {
  var key = String(name);
  for (var i = 0; i < this._pairs.length; i++) {
    if (this._pairs[i][0] === key) return this._pairs[i][1];
  }
  return null;
};

URLSearchParams.prototype.getAll = function (name) {
  var key = String(name);
  var values = [];
  for (var i = 0; i < this._pairs.length; i++) {
    if (this._pairs[i][0] === key) values.push(this._pairs[i][1]);
  }
  return values;
};

URLSearchParams.prototype.has = function (name) {
  return this.get(name) !== null;
};

URLSearchParams.prototype['delete'] = function (name) {
  var key = String(name);
  var kept = [];
  for (var i = 0; i < this._pairs.length; i++) {
    if (this._pairs[i][0] !== key) kept.push(this._pairs[i]);
  }
  this._pairs = kept;
};

URLSearchParams.prototype.toString = function () {
  var parts = [];
  for (var i = 0; i < this._pairs.length; i++) {
    parts.push(__qext_form_encode(this._pairs[i][0]) + '=' + __qext_form_encode(this._pairs[i][1]));
  }
  return parts.join('&');
};

URLSearchParams.prototype.forEach = function (callback, thisArg) {
  for (var i = 0; i < this._pairs.length; i++) {
    callback.call(thisArg, this._pairs[i][1], this._pairs[i][0], this);
  }
};

// ==================== URL ====================

function __qext_remove_dot_segments(text) {
  var match = /^([A-Za-z][A-Za-z0-9+.\-]*:)(\/\/[^\/?#]*)?([^?#]*)([\s\S]*)$/.exec(text);
  if (!match) return text;

  var segments = (match[3] || '').split('/');
  var output = [];

  for (var i = 0; i < segments.length; i++) {
    var segment = segments[i];
    if (segment === '.') {
      if (i === segments.length - 1) output.push('');
      continue;
    }
    if (segment === '..') {
      if (output.length > 1) output.pop();
      if (i === segments.length - 1) output.push('');
      continue;
    }
    output.push(segment);
  }

  return match[1] + (match[2] || '') + output.join('/') + (match[4] || '');
}

function __qext_split_url(text) {
  var match = /^([A-Za-z][A-Za-z0-9+.\-]*):([\s\S]*)$/.exec(text);
  if (!match) throw new TypeError('Invalid URL: ' + text);

  var protocol = match[1].toLowerCase() + ':';
  var rest = match[2];

  var hash = '';
  var hashAt = rest.indexOf('#');
  if (hashAt !== -1) {
    hash = rest.slice(hashAt);
    rest = rest.slice(0, hashAt);
  }

  var search = '';
  var searchAt = rest.indexOf('?');
  if (searchAt !== -1) {
    search = rest.slice(searchAt);
    rest = rest.slice(0, searchAt);
  }

  var authority = null;
  if (rest.slice(0, 2) === '//') {
    rest = rest.slice(2);
    var end = rest.indexOf('/');
    if (end === -1) {
      authority = rest;
      rest = '';
    } else {
      authority = rest.slice(0, end);
      rest = rest.slice(end);
    }
  }

  var username = '';
  var password = '';
  var hostname = '';
  var port = '';

  if (authority !== null) {
    var at = authority.lastIndexOf('@');
    if (at !== -1) {
      var credentials = authority.slice(0, at).split(':');
      username = credentials[0];
      password = credentials.length > 1 ? credentials.slice(1).join(':') : '';
      authority = authority.slice(at + 1);
    }

    var portAt = authority.lastIndexOf(':');
    if (portAt !== -1 && /^\d*$/.test(authority.slice(portAt + 1))) {
      port = authority.slice(portAt + 1);
      authority = authority.slice(0, portAt);
    }

    hostname = authority.toLowerCase();
  }

  return {
    protocol: protocol,
    username: username,
    password: password,
    hostname: hostname,
    port: port,
    pathname: authority !== null && rest === '' ? '/' : rest,
    search: search,
    hash: hash
  };
}

function __qext_serialise(parts) {
  var credentials = '';
  if (parts.username) {
    credentials = parts.username + (parts.password ? ':' + parts.password : '') + '@';
  }
  var authority = parts.hostname === '' ? '' : '//' + credentials + parts.hostname + (parts.port ? ':' + parts.port : '');
  return parts.protocol + authority + parts.pathname + parts.search + parts.hash;
}

function __qext_resolve_url(reference, base) {
  if (/^[A-Za-z][A-Za-z0-9+.\-]*:/.test(reference)) {
    return __qext_remove_dot_segments(reference);
  }

  if (base === null) throw new TypeError('Invalid URL: ' + reference);

  var baseParts = __qext_split_url(__qext_remove_dot_segments(base));
  var credentials = baseParts.username
    ? baseParts.username + (baseParts.password ? ':' + baseParts.password : '') + '@'
    : '';
  var authority = baseParts.hostname === ''
    ? ''
    : '//' + credentials + baseParts.hostname + (baseParts.port ? ':' + baseParts.port : '');
  var origin = baseParts.protocol + authority;

  if (reference === '') return __qext_serialise(baseParts);
  if (reference.slice(0, 2) === '//') return __qext_remove_dot_segments(baseParts.protocol + reference);
  if (reference.charAt(0) === '#') {
    return __qext_serialise({
      protocol: baseParts.protocol,
      username: baseParts.username,
      password: baseParts.password,
      hostname: baseParts.hostname,
      port: baseParts.port,
      pathname: baseParts.pathname,
      search: baseParts.search,
      hash: reference
    });
  }
  if (reference.charAt(0) === '?') {
    return __qext_serialise({
      protocol: baseParts.protocol,
      username: baseParts.username,
      password: baseParts.password,
      hostname: baseParts.hostname,
      port: baseParts.port,
      pathname: baseParts.pathname,
      search: reference,
      hash: ''
    });
  }
  if (reference.charAt(0) === '/') {
    return __qext_remove_dot_segments(origin + reference);
  }

  var directory = baseParts.pathname.replace(/[^\/]*$/, '');
  return __qext_remove_dot_segments(origin + directory + reference);
}

function URL(input, base) {
  var text = __qext_text(input).trim();
  var resolved = __qext_resolve_url(text, base === undefined || base === null ? null : __qext_text(base));
  var parts = __qext_split_url(resolved);

  this.href = resolved;
  this.protocol = parts.protocol;
  this.username = parts.username;
  this.password = parts.password;
  this.hostname = parts.hostname;
  this.port = parts.port;
  this.host = parts.port ? parts.hostname + ':' + parts.port : parts.hostname;
  this.pathname = parts.pathname;
  this.search = parts.search;
  this.hash = parts.hash;
  this.origin = parts.hostname ? parts.protocol + '//' + this.host : 'null';
  this.searchParams = new URLSearchParams(parts.search);
}

URL.prototype.toString = function () {
  return this.href;
};

URL.prototype.toJSON = function () {
  return this.href;
};

// ==================== responses and fetch ====================

function __qext_headers(raw) {
  var headers = {};
  var source = raw || {};

  for (var key in source) {
    if (Object.prototype.hasOwnProperty.call(source, key)) {
      headers[String(key).toLowerCase()] = source[key];
    }
  }

  // Non-enumerable so `for (var key in headers)` - which every shipped package
  // uses to scan for a header - never sees it.
  Object.defineProperty(headers, 'get', {
    value: function (name) {
      var values = headers[String(name).toLowerCase()];
      if (!values || !values.length) return null;
      return String(values[0]);
    },
    enumerable: false
  });

  return headers;
}

function __qext_make_response(raw) {
  var body = raw.body === null || raw.body === undefined ? '' : String(raw.body);

  return {
    ok: raw.ok === true,
    status: Number(raw.statusCode || 0),
    statusText: String(raw.status || ''),
    url: String(raw.url || ''),
    headers: __qext_headers(raw.headers),
    body: body,
    text: function () { return body; },
    json: function () { return JSON.parse(body); },
    arrayBuffer: function () { return new TextEncoder().encode(body).buffer; }
  };
}

function __qext_http_request(method, url, body, headers) {
  var payload = null;
  if (body !== undefined && body !== null) {
    payload = typeof body === 'string' ? body : JSON.stringify(body);
  }

  var reply = __qext_http(JSON.stringify({
    method: String(method || 'GET').toUpperCase(),
    url: __qext_text(url),
    body: payload,
    headers: headers || {}
  }));

  return __qext_unwrap(reply);
}

function fetch(url, init) {
  var options = init || {};
  return __qext_make_response(
    __qext_http_request(options.method || 'GET', url, options.body, options.headers)
  );
}

var http = {
  request: function (method, url, body, headers) {
    return __qext_http_request(method, url, body, headers);
  },
  get: function (url, headers) {
    return __qext_http_request('GET', url, null, headers);
  },
  post: function (url, body, headers) {
    return __qext_http_request('POST', url, body, headers);
  },
  put: function (url, body, headers) {
    return __qext_http_request('PUT', url, body, headers);
  },
  patch: function (url, body, headers) {
    return __qext_http_request('PATCH', url, body, headers);
  },
  'delete': function (url, headers) {
    return __qext_http_request('DELETE', url, null, headers);
  },
  clearCookies: function () {
    return __qext_clear_cookies() === true;
  }
};

// ==================== utils ====================

var utils = {
  parseJSON: function (text) { return JSON.parse(text); },
  stringifyJSON: function (value) { return JSON.stringify(value); },

  base64Encode: function (text) {
    return __qext_unwrap(__qext_base64_encode(__qext_text(text)));
  },
  base64Decode: function (text) {
    return __qext_unwrap(__qext_base64_decode(__qext_text(text)));
  },

  md5: function (text) { return __qext_unwrap(__qext_md5(__qext_text(text))); },
  sha256: function (text) { return __qext_unwrap(__qext_sha256(__qext_text(text))); },

  hmacSHA256: function (key, message) {
    return __qext_unwrap(__qext_hmac_sha256(__qext_text(key), __qext_text(message)));
  },
  hmacSHA256Base64: function (key, message) {
    return __qext_unwrap(__qext_hmac_sha256_base64(__qext_text(key), __qext_text(message)));
  },
  // Byte-oriented: TOTP needs the raw digest, and the shipped caller indexes it.
  hmacSHA1: function (key, message) {
    return __qext_unwrap(
      __qext_hmac_sha1(JSON.stringify(__qext_bytes(key)), JSON.stringify(__qext_bytes(message)))
    );
  },

  encrypt: function (text, key) {
    try {
      return { success: true, data: __qext_unwrap(__qext_encrypt(__qext_text(text), __qext_text(key))) };
    } catch (error) {
      return { success: false, error: error && error.message ? error.message : String(error) };
    }
  },
  decrypt: function (data, key) {
    try {
      return { success: true, data: __qext_unwrap(__qext_decrypt(__qext_text(data), __qext_text(key))) };
    } catch (error) {
      return { success: false, error: error && error.message ? error.message : String(error) };
    }
  },
  generateKey: function (length) {
    try {
      var generated = __qext_unwrap(__qext_generate_key(Number(length) || 32));
      return { success: true, key: generated.base64, hex: generated.hex };
    } catch (error) {
      return { success: false, error: error && error.message ? error.message : String(error) };
    }
  },

  sleep: function (milliseconds) {
    return __qext_sleep(Number(milliseconds) || 0) === true;
  },
  isDownloadCancelled: function () {
    return __qext_is_cancelled() === true;
  },
  getResolutionRemainingMs: function () {
    return Number(__qext_remaining_ms());
  },

  appVersion: function () { return String(__qext_app_version()); },
  appUserAgent: function () { return String(__qext_user_agent()); },
  randomUserAgent: function () { return String(__qext_random_user_agent()); }
};

// ==================== storage ====================

function __qext_storage_key(key) {
  var text = __qext_text(key);
  if (!text) throw new Error('storage keys must be non-empty strings');
  return text;
}

var storage = {
  get: function (key) {
    return __qext_unwrap(__qext_storage_get(__qext_storage_key(key)));
  },
  set: function (key, value) {
    var encoded = value === undefined ? 'null' : JSON.stringify(value);
    return __qext_unwrap(__qext_storage_set(__qext_storage_key(key), encoded));
  },
  remove: function (key) {
    return __qext_unwrap(__qext_storage_remove(__qext_storage_key(key)));
  }
};

// ==================== files ====================

var file = {
  exists: function (path) {
    return __qext_file_exists(__qext_text(path)) === true;
  },
  read: function (path) {
    return __qext_unwrap(__qext_file_read(__qext_text(path)));
  },
  write: function (path, content) {
    return __qext_unwrap(__qext_file_write(__qext_text(path), __qext_text(content)));
  },
  'delete': function (path) {
    return __qext_unwrap(__qext_file_delete(__qext_text(path)));
  },
  move: function (from, to) {
    return __qext_unwrap(__qext_file_move(__qext_text(from), __qext_text(to)));
  },
  download: function (url, path, options) {
    options = options || {};
    return __qext_unwrap(__qext_file_download(
      __qext_text(url),
      __qext_text(path),
      __qext_transfer_options(options),
      typeof options.onProgress === 'function' ? options.onProgress : null
    ));
  },
  downloadSegments: function (urls, path, options) {
    options = options || {};
    return __qext_unwrap(__qext_file_download_segments(
      JSON.stringify(urls),
      __qext_text(path),
      __qext_transfer_options(options),
      typeof options.onProgress === 'function' ? options.onProgress : null
    ));
  }
};

function __qext_transfer_options(options) {
  var headers = (options.headers && typeof options.headers === 'object') ? options.headers : null;
  return JSON.stringify({
    headers: headers,
    resume: options.resume === true,
    persistentCheckpoint: options.persistentCheckpoint === true
  });
}

// ==================== logging ====================

function __qext_log_line(level, args) {
  var parts = [];
  for (var i = 0; i < args.length; i++) {
    var value = args[i];
    if (typeof value === 'string') {
      parts.push(value);
      continue;
    }
    try {
      parts.push(JSON.stringify(value));
    } catch (error) {
      parts.push(String(value));
    }
  }
  __qext_log(level, parts.join(' '));
}

var log = {
  debug: function () { __qext_log_line('debug', arguments); },
  info: function () { __qext_log_line('info', arguments); },
  warn: function () { __qext_log_line('warn', arguments); },
  error: function () { __qext_log_line('error', arguments); }
};

// QuickJS ships no console. Map it to the same sink so a package that uses it
// produces output instead of a load-time ReferenceError.
var console = {
  log: log.info,
  info: log.info,
  warn: log.warn,
  error: log.error,
  debug: log.debug
};

// ==================== matching ====================

function __qext_normalise(text) {
  return __qext_text(text)
    .toLowerCase()
    .replace(/\[[^\]]*\]|\([^)]*\)/g, ' ')
    .replace(/[^a-z0-9\u00c0-\uffff]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function __qext_similarity(left, right) {
  if (left === right) return 1;
  if (!left || !right) return 0;

  var previous = [];
  var current = [];
  var i;
  var j;

  for (j = 0; j <= right.length; j++) previous[j] = j;

  for (i = 1; i <= left.length; i++) {
    current[0] = i;
    for (j = 1; j <= right.length; j++) {
      var cost = left.charAt(i - 1) === right.charAt(j - 1) ? 0 : 1;
      current[j] = Math.min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + cost);
    }
    for (j = 0; j <= right.length; j++) previous[j] = current[j];
  }

  var distance = previous[right.length];
  return 1 - distance / Math.max(left.length, right.length);
}

var matching = {
  normalizeString: function (text) {
    return __qext_normalise(text);
  },
  // Returns a 0..1 similarity: the shipped caller compares it against 0.86.
  compareStrings: function (left, right) {
    return __qext_similarity(__qext_normalise(left), __qext_normalise(right));
  },
  compareDuration: function (left, right, tolerance) {
    return Math.abs(Number(left || 0) - Number(right || 0)) <= Math.abs(Number(tolerance || 0));
  }
};

// ==================== gobackend ====================

var __qext_RESERVED_FILENAMES = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i;

function __qext_sanitize_filename(name) {
  var cleaned = __qext_text(name)
    .replace(/[<>:"|?*\u0000-\u001f]/g, '_')
    .replace(/[\\\/]/g, '_')
    .replace(/\s+/g, ' ')
    .replace(/^[. ]+/, '')
    .replace(/[. ]+$/, '');

  if (!cleaned) cleaned = 'untitled';

  var stem = cleaned.split('.')[0];
  if (__qext_RESERVED_FILENAMES.test(stem)) cleaned = '_' + cleaned;

  if (cleaned.length > 200) {
    cleaned = cleaned.slice(0, 200).replace(/[. ]+$/, '');
  }

  return cleaned;
}

function __qext_build_filename(template, metadata) {
  var text = __qext_text(template);
  var fields = [
    'title', 'artist', 'album', 'album_artist', 'track_number',
    'track', 'disc_number', 'year', 'isrc', 'quality', 'ext'
  ];

  for (var i = 0; i < fields.length; i++) {
    var key = fields[i];
    var value = metadata[key];
    // Sanitised before substitution so a field value can never inject a path
    // separator; the template's own separators still create directories.
    value = __qext_sanitize_filename(value === undefined || value === null ? '' : value);
    text = text.split('{' + key + '}').join(value);
    text = text.split('{' + key.toUpperCase() + '}').join(value);
  }

  return text;
}

var gobackend = {
  sanitizeFilename: function (name) {
    return __qext_sanitize_filename(name);
  },
  buildFilename: function (template, metadata) {
    return __qext_build_filename(template, metadata || {});
  },
  getLocalTime: function () {
    return JSON.parse(String(__qext_unwrap(__qext_local_time())));
  },
  getAudioQuality: function (path) {
    if (typeof ffmpeg !== "undefined" && ffmpeg && typeof ffmpeg.getInfo === "function") {
      return ffmpeg.getInfo(path);
    }
    return { error: "audio quality inspection is not available in this host" };
  },
  checkISRCExists: function () {
    return { exists: false };
  },
  getLyricsLRC: function () {
    return { error: 'lyrics lookup is not available in this host' };
  }
};

// ==================== unimplemented surfaces ====================

var __qext_cred_key = "qext-credentials-v1";
var credentials = {
  __bag: function () { return storage.get("__qext_credentials__") || {}; },
  __save: function (bag) { storage.set("__qext_credentials__", bag); },
  store: function (key, value) {
    var bag = credentials.__bag();
    var sealed = utils.encrypt(JSON.stringify(value), __qext_cred_key);
    if (!sealed || !sealed.success) {
      return;
    }
    bag[String(key)] = sealed.data;
    credentials.__save(bag);
  },
  get: function (key) {
    var bag = credentials.__bag();
    if (!Object.prototype.hasOwnProperty.call(bag, String(key))) {
      return undefined;
    }
    var out = utils.decrypt(bag[String(key)], __qext_cred_key);
    if (!out || !out.success) {
      return undefined;
    }
    try { return JSON.parse(out.data); } catch (e) { return undefined; }
  },
  has: function (key) {
    var bag = credentials.__bag();
    return Object.prototype.hasOwnProperty.call(bag, String(key));
  },
  remove: function (key) {
    var bag = credentials.__bag();
    delete bag[String(key)];
    credentials.__save(bag);
  }
};

var auth = {
  openAuthUrl: function () { __qext_unavailable('auth.openAuthUrl'); },
  getAuthCode: function () { __qext_unavailable('auth.getAuthCode'); },
  setAuthCode: function () { __qext_unavailable('auth.setAuthCode'); },
  isAuthenticated: function () { __qext_unavailable('auth.isAuthenticated'); },
  getTokens: function () { __qext_unavailable('auth.getTokens'); },
  clearAuth: function () { __qext_unavailable('auth.clearAuth'); },
  generatePKCE: function () { __qext_unavailable('auth.generatePKCE'); },
  getPKCE: function () { __qext_unavailable('auth.getPKCE'); },
  startOAuthWithPKCE: function () { __qext_unavailable('auth.startOAuthWithPKCE'); },
  exchangeCodeWithPKCE: function () { __qext_unavailable('auth.exchangeCodeWithPKCE'); }
};

var ffmpeg = {
  execute: function (args) {
    return __qext_ffmpeg_result(__qext_ffmpeg_execute(__qext_text(args)));
  },
  getInfo: function (path) {
    return __qext_ffmpeg_result(__qext_ffmpeg_info(__qext_text(path)));
  },
  convert: function (input, output, options) {
    return __qext_ffmpeg_result(
      __qext_ffmpeg_convert(__qext_text(input), __qext_text(output), JSON.stringify(options || {}))
    );
  }
};
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::http::HttpLimits;
    use crate::runtime::permissions::NetworkAllowlist;
    use crate::runtime::testing::spawn_server;
    use crate::runtime::{ExtensionRuntime, RuntimeLimits};
    use serde_json::json;

    fn probe(host: Arc<HostServices>, expression: &str) -> Value {
        let source = format!(
            "registerExtension({{ probe: function () {{ return {expression}; }} }});"
        );

        let runtime = ExtensionRuntime::load_with_host(
            "probe",
            &source,
            RuntimeLimits::default(),
            host,
        )
        .expect("loads");

        runtime.call("probe", &[]).expect("call")
    }

    fn default_probe(expression: &str) -> Value {
        probe(Arc::new(HostServices::default()), expression)
    }

    fn http_host(base_url: &str) -> Arc<HostServices> {
        let host = base_url
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string();

        Arc::new(HostServices::new(HostConfig {
            http: Some(HostHttp::new(
                NetworkAllowlist::from_patterns(&[host]),
                HttpLimits::default(),
            )),
            ..HostConfig::default()
        }))
    }

    #[test]
    fn hashes_and_hmac_match_published_vectors() {
        let value = default_probe(
            "[utils.sha256('abc'), utils.md5('abc'), utils.hmacSHA256('Jefe', 'what do ya want for nothing?'), utils.hmacSHA256Base64('Jefe', 'what do ya want for nothing?')]",
        );

        assert_eq!(
            value,
            json!([
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                "900150983cd24fb0d6963f7d28e17f72",
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
                "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM=",
            ])
        );
    }

    #[test]
    fn base64_helpers_round_trip_non_ascii_text() {
        let value = default_probe(
            "utils.base64Decode(utils.base64Encode('λύρα — 音楽 — 🎵'))",
        );
        assert_eq!(value, json!("λύρα — 音楽 — 🎵"));

        // atob/btoa are QuickJS builtins working on Latin-1 code units, so they
        // round-trip Latin-1 but reject anything above U+00FF. Callers with real
        // Unicode text use utils.base64Encode, tested above.
        let value = default_probe("[btoa('café'), atob(btoa('café'))]");
        assert_eq!(value, json!(["Y2Fm6Q==", "café"]));

        let value = default_probe(
            "(function () { try { btoa('音'); return 'allowed'; } catch (error) { return 'throws'; } })()",
        );
        assert_eq!(value, json!("throws"));
    }

    #[test]
    fn url_search_params_serialise_as_form_encoded() {
        let value = default_probe(
            "new URLSearchParams({ grant_type: 'authorization_code', code: 'a b&c', redirect_uri: 'https://x.test/cb?q=1' }).toString()",
        );

        assert_eq!(
            value,
            json!("grant_type=authorization_code&code=a+b%26c&redirect_uri=https%3A%2F%2Fx.test%2Fcb%3Fq%3D1")
        );

        let value = default_probe(
            "(function () { var params = new URLSearchParams('?a=1&a=2'); params.set('a', '3'); params.append('b', '4 5'); return [params.get('a'), params.getAll('a').length, params.toString()]; })()",
        );
        assert_eq!(value, json!(["3", 1, "a=3&b=4+5"]));
    }

    #[test]
    fn url_parses_the_shapes_the_amazon_package_uses() {
        let value = default_probe(
            "(function () { var u = new URL('https://music.amazon.com/tracks/B01?trackAsin=B01ABC'); return [u.protocol, u.hostname, u.pathname, u.searchParams.get('trackAsin')]; })()",
        );
        assert_eq!(
            value,
            json!(["https:", "music.amazon.com", "/tracks/B01", "B01ABC"])
        );

        let value = default_probe(
            "(function () { var u = new URL('deeplink', 'https://music.amazon.com/home'); return u.href; })()",
        );
        assert_eq!(value, json!("https://music.amazon.com/deeplink"));

        let value = default_probe(
            "(function () { var u = new URL('/a/./b/../c?x=1#h', 'https://music.amazon.com/base'); return [u.href, u.host, u.origin]; })()",
        );
        assert_eq!(
            value,
            json!(["https://music.amazon.com/a/c?x=1#h", "music.amazon.com", "https://music.amazon.com"])
        );
    }

    #[test]
    fn text_encoder_and_decoder_round_trip_utf8() {
        let value = default_probe(
            "(function () { var text = 'λύρα — 音楽 — 🎵'; return new TextDecoder().decode(new TextEncoder().encode(text)) === text; })()",
        );
        assert_eq!(value, json!(true));

        let value = default_probe(
            "(function () { var bytes = new TextEncoder().encode('é'); return [bytes.length, Array.prototype.slice.call(bytes)]; })()",
        );
        assert_eq!(value, json!([2, [0xc3, 0xa9]]));
    }

    #[test]
    fn hmac_sha1_totp_matches_the_independent_computation() {
        // The same byte-array call the shipped spotify-web package makes.
        let value = default_probe(
            "(function () { \
                var key = [0x31,0x32,0x33,0x34,0x35,0x36,0x37,0x38,0x39,0x30,0x31,0x32,0x33,0x34,0x35,0x36,0x37,0x38,0x39,0x30]; \
                var counter = [0,0,0,0,0,0,0,1]; \
                var hmac = utils.hmacSHA1(key, counter); \
                var offset = hmac[hmac.length - 1] & 0x0f; \
                var code = ((hmac[offset] & 0x7f) << 24) | ((hmac[offset + 1] & 0xff) << 16) | ((hmac[offset + 2] & 0xff) << 8) | (hmac[offset + 3] & 0xff); \
                return (code % 1000000).toString().padStart(6, '0'); \
            })()",
        );

        let expected = crypto::totp_code(b"12345678901234567890", 1, 6);
        assert_eq!(value, json!(expected));
        assert_eq!(expected, "287082", "RFC 6238 vector, counter 1");
    }

    #[test]
    fn encryption_round_trips_and_reports_failure_in_band() {
        let value = default_probe(
            "(function () { var sealed = utils.encrypt('secret payload', 'correct horse'); return [sealed.success, utils.decrypt(sealed.data, 'correct horse').data, utils.decrypt(sealed.data, 'wrong').success]; })()",
        );
        assert_eq!(value, json!([true, "secret payload", false]));
    }

    #[test]
    fn generated_keys_have_the_requested_length() {
        let value = default_probe(
            "(function () { var key = utils.generateKey(32); return [key.success, atob(key.key).length, key.hex.length]; })()",
        );
        assert_eq!(value, json!([true, 32, 64]));
    }

    #[test]
    fn local_time_reports_the_machine_zone_not_a_placeholder() {
        let value = default_probe("gobackend.getLocalTime()");
        let expected = local_now();

        let timezone = value["timezone"].as_str().expect("timezone string");
        assert!(!timezone.is_empty(), "timezone must name the zone");
        assert_ne!(timezone, "Local");

        assert_eq!(
            value["offsetMinutes"],
            json!(expected.offset_minutes),
            "offset must use the getTimezoneOffset sign convention"
        );
        assert_eq!(value["weekday"], json!(expected.weekday));

        let timestamp = value["timestamp"].as_i64().expect("timestamp");
        assert!(
            (timestamp - expected.timestamp).abs() <= 5,
            "timestamp {timestamp} is not close to {}",
            expected.timestamp
        );
        assert!(
            (0..24).contains(&value["hour"].as_u64().unwrap_or(99)),
            "hour must be 0..23, got {}",
            value["hour"]
        );
    }

    #[test]
    fn storage_survives_teardown_and_reload() {
        let directory = tempfile::tempdir().expect("temp dir");

        let writer = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            ..HostConfig::default()
        }));

        let source = "registerExtension({ probe: function () { storage.set('session', { id: 42, scopes: ['a'] }); return storage.get('session'); } });";
        let runtime = ExtensionRuntime::load_with_host(
            "writer",
            source,
            RuntimeLimits::default(),
            writer,
        )
        .expect("loads");

        assert_eq!(
            runtime.call("probe", &[]).expect("call"),
            json!({ "id": 42, "scopes": ["a"] })
        );
        drop(runtime);

        assert!(
            directory.path().join(STORAGE_FILE).is_file(),
            "storage.json must exist on disk"
        );

        let reader = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            ..HostConfig::default()
        }));

        let source = "registerExtension({ probe: function () { return storage.get('session'); } });";
        let runtime = ExtensionRuntime::load_with_host(
            "reader",
            source,
            RuntimeLimits::default(),
            reader,
        )
        .expect("loads");

        assert_eq!(
            runtime.call("probe", &[]).expect("call"),
            json!({ "id": 42, "scopes": ["a"] })
        );
    }

    #[test]
    fn storage_remove_clears_a_key() {
        let directory = tempfile::tempdir().expect("temp dir");
        let host = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            ..HostConfig::default()
        }));

        let value = probe(
            Arc::clone(&host),
            "(function () { storage.set('k', 1); storage.remove('k'); return storage.get('k'); })()",
        );

        assert_eq!(value, Value::Null);
        assert!(host.storage().is_empty());
    }

    #[test]
    fn file_api_reads_and_writes_inside_the_sandbox_only() {
        let directory = tempfile::tempdir().expect("temp dir");
        let host = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            ..HostConfig::default()
        }));

        let value = probe(
            Arc::clone(&host),
            "(function () { file.write('notes/track.txt', 'hello'); var text = file.read('notes/track.txt'); var exists = file.exists('notes/track.txt'); file.delete('notes/track.txt'); return [text, exists, file.exists('notes/track.txt')]; })()",
        );
        assert_eq!(value, json!(["hello", true, false]));

        let value = probe(
            host,
            "(function () { try { file.write('../escape.txt', 'x'); return 'allowed'; } catch (error) { return error.message; } })()",
        );
        let message = value.as_str().expect("message");
        assert!(
            message.contains("path") || message.contains(".."),
            "escape attempt reported {message:?}"
        );
    }

    #[test]
    fn file_api_fails_cleanly_without_a_data_directory() {
        let value = default_probe(
            "(function () { try { file.read('x.txt'); return 'allowed'; } catch (error) { return error.message; } })()",
        );
        assert_eq!(
            value,
            json!("file access is not available for this extension")
        );
    }

    #[test]
    fn fetch_is_refused_when_the_host_has_no_network_client() {
        let value = default_probe(
            "(function () { try { fetch('https://api.example.com/x'); return 'allowed'; } catch (error) { return error.message; } })()",
        );
        assert_eq!(
            value,
            json!("network access is not available for this extension")
        );
    }

    #[test]
    fn fetch_returns_a_json_body_and_status_from_a_real_socket() {
        let base = spawn_server(|request| match request.path.as_str() {
            "/json" => (
                200,
                vec![("Content-Type".into(), "application/json".into())],
                br#"{"hello":"world"}"#.to_vec(),
            ),
            "/echo" => (
                200,
                vec![],
                format!("{} {}", request.method, request.body.len()).into_bytes(),
            ),
            _ => (404, vec![], b"nope".to_vec()),
        });

        let host = http_host(&base);

        let value = probe(
            Arc::clone(&host),
            &format!(
                "(function () {{ var r = fetch('{base}/json'); return [r.ok, r.status, r.statusText, r.json().hello, r.headers.get('content-type')]; }})()"
            ),
        );
        assert_eq!(
            value,
            json!([true, 200, "OK", "world", "application/json"])
        );

        let value = probe(
            host,
            &format!(
                "(function () {{ var r = fetch('{base}/echo', {{ method: 'POST', body: 'abcd', headers: {{ 'X-Test': '1' }} }}); return [r.status, r.text()]; }})()"
            ),
        );
        assert_eq!(value, json!([200, "POST 4"]));
    }

    #[test]
    fn http_object_uses_the_same_transport_and_reports_status_code() {
        let base = spawn_server(|_| (404, vec![], b"missing".to_vec()));
        let host = http_host(&base);

        let value = probe(
            host,
            &format!(
                "(function () {{ var r = http.get('{base}/missing'); return [r.statusCode, r.ok, r.body]; }})()"
            ),
        );
        assert_eq!(value, json!([404, false, "missing"]));
    }

    /// The engine assigns every transferred address its own partial file name;
    /// tests need it to stage resumes the way a retry would.
    fn partial_name_for(base: &Path, url: &str, tracked: &str) -> PathBuf {
        let job = transfer::job_id_for(url, tracked);
        let name = base
            .file_name()
            .and_then(|entry| entry.to_str())
            .unwrap_or_default();
        base.with_file_name(format!("{name}.{job}.part"))
    }

    #[test]
    fn file_download_streams_bytes_into_the_sandbox_with_headers_and_progress() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        let base = spawn_server(move |request| {
            if let Some(header) = request.header("x-probe") {
                sink.lock().expect("sink lock").push(header.to_string());
            }
            (200, vec![], b"0123456789".to_vec())
        });

        let directory = tempfile::tempdir().expect("temp dir");
        let host = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            http: Some(HostHttp::new(
                NetworkAllowlist::from_patterns(&["127.0.0.1".to_string()]),
                HttpLimits::default(),
            )),
            ..HostConfig::default()
        }));

        let value = probe(
            Arc::clone(&host),
            &format!(
                "(function () {{ \
                    var progress = []; \
                    var result = file.download('{base}/audio.flac', 'downloads/song.flac', {{ \
                        headers: {{ 'X-Probe': 'on' }}, \
                        onProgress: function (written, total) {{ progress.push([written, total]); }} \
                    }}); \
                    var text = file.read('downloads/song.flac'); \
                    return [result.success, result.path, result.bytes, text, progress.length > 0, progress[progress.length - 1][0]]; \
                }})()"
            ),
        );

        assert_eq!(
            value,
            json!([true, "downloads/song.flac", 10, "0123456789", true, 10])
        );
        assert_eq!(*seen.lock().expect("sink lock"), vec!["on".to_string()]);
    }

    #[test]
    fn file_download_resumes_a_recorded_partial_with_a_range_request() {
        let seen_range = Arc::new(Mutex::new(Option::<String>::None));
        let records = seen_range.clone();
        let base = spawn_server(move |request| {
            match request.header("range") {
                // A real server answers a range with the remainder plus the
                // Content-Range the engine verifies.
                Some(range) => {
                    *records.lock().expect("sink lock") = Some(range.to_string());
                    (
                        206,
                        vec![("Content-Range".into(), "bytes 5-9/10".into())],
                        b"56789".to_vec(),
                    )
                }
                None => (200, vec![], b"0123456789".to_vec()),
            }
        });

        let directory = tempfile::tempdir().expect("temp dir");
        let root = directory.path().to_path_buf();
        let host = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(root.clone()),
            http: Some(HostHttp::new(
                NetworkAllowlist::from_patterns(&["127.0.0.1".to_string()]),
                HttpLimits::default(),
            )),
            ..HostConfig::default()
        }));

        let url = format!("{base}/audio.flac");
        std::fs::create_dir_all(root.join("downloads")).expect("staging");
        // The sandbox serves canonical paths (a verbatim prefix on Windows), so
        // the staged name has to be derived the same way.
        let downloads = std::fs::canonicalize(root.join("downloads")).expect("canonical dir");
        let output = downloads.join("song.flac");
        // Stage the bytes a first attempt recorded, exactly where the engine
        // names them.
        let staged = partial_name_for(&output, &url, &output.to_string_lossy());
        std::fs::write(&staged, b"01234").expect("stage partial");

        let value = probe(
            Arc::clone(&host),
            &format!(
                "(function () {{ \
                    var result = file.download('{url}', 'downloads/song.flac', {{ resume: true }}); \
                    return [result.success, file.read('downloads/song.flac')]; \
                }})()"
            ),
        );

        // The resumed file is the staged prefix plus what the 206 delivered.
        assert_eq!(value, json!([true, "0123456789"]));
        assert_eq!(
            *seen_range.lock().expect("sink lock"),
            Some("bytes=5-".to_string())
        );
    }

    #[test]
    fn file_download_segments_concatenate_in_order_and_report_each_one() {
        let base = spawn_server(move |request| match request.path.as_str() {
            "/init" => (200, vec![], b"aa".to_vec()),
            "/media-1" => (200, vec![], b"bb".to_vec()),
            "/media-2" => (200, vec![], b"ccc".to_vec()),
            "/media-3" => (200, vec![], b"dddd".to_vec()),
            _ => (404, vec![], b"missing".to_vec()),
        });

        let directory = tempfile::tempdir().expect("temp dir");
        let host = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            http: Some(HostHttp::new(
                NetworkAllowlist::from_patterns(&["127.0.0.1".to_string()]),
                HttpLimits::default(),
            )),
            ..HostConfig::default()
        }));

        let value = probe(
            Arc::clone(&host),
            &format!(
                "(function () {{ \
                    var line = []; \
                    var result = file.downloadSegments( \
                        ['{base}/init', '{base}/media-1', '{base}/media-2', '{base}/media-3'], \
                        'downloads/manifest.m4a', \
                        {{ \
                            persistentCheckpoint: true, \
                            onProgress: function (written, total, completed, totalSegments) {{ \
                                line.push([completed, totalSegments]); \
                            }} \
                        }} \
                    ); \
                    return [result.success, result.bytes, file.read('downloads/manifest.m4a'), line[line.length - 1]]; \
                }})()"
            ),
        );

        assert_eq!(value, json!([true, 11, "aabbcccdddd", [4, 4]]));
    }

    #[test]
    fn file_download_reports_a_host_without_network_access_as_unavailable() {
        let base = spawn_server(|_| (200, vec![], b"secret".to_vec()));
        let directory = tempfile::tempdir().expect("temp dir");
        let host = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            // Deliberately no network permission at all.
            ..HostConfig::default()
        }));

        let value = probe(
            Arc::clone(&host),
            &format!(
                "(function () {{ var result = file.download('{base}/audio.flac', 'song.flac', {{}}); return [result.success, result.error_type]; }})()"
            ),
        );

        assert_eq!(value, json!([false, "unavailable"]));
    }

    #[test]
    fn file_download_reports_a_host_cancellation_in_package_words() {
        let base = spawn_server(|_| (200, vec![], b"never read".to_vec()));
        let directory = tempfile::tempdir().expect("temp dir");
        let host = Arc::new(HostServices::new(HostConfig {
            data_dir: Some(directory.path().to_path_buf()),
            http: Some(HostHttp::new(
                NetworkAllowlist::from_patterns(&["127.0.0.1".to_string()]),
                HttpLimits::default(),
            )),
            ..HostConfig::default()
        }));
        host.cancel();

        let value = probe(
            host,
            &format!(
                "(function () {{ var result = file.download('{base}/audio.flac', 'song.flac', {{}}); return [result.success, result.error, result.error_type]; }})()"
            ),
        );

        assert_eq!(value, json!([false, "download cancelled", "cancelled"]));
    }

    #[test]
    fn log_records_reach_the_host_sink() {
        let host = Arc::new(HostServices::default());
        probe(
            Arc::clone(&host),
            "(function () { log.info('hello', { a: 1 }); log.warn('careful'); return true; })()",
        );

        assert_eq!(
            host.logs(),
            vec![
                LogRecord {
                    level: "info".to_string(),
                    message: "hello {\"a\":1}".to_string()
                },
                LogRecord {
                    level: "warn".to_string(),
                    message: "careful".to_string()
                },
            ]
        );
    }

    #[test]
    fn sleep_returns_false_once_cancelled() {
        let host = Arc::new(HostServices::default());
        host.cancel();

        let value = probe(
            Arc::clone(&host),
            "(function () { return [utils.sleep(1000), utils.isDownloadCancelled()]; })()",
        );
        assert_eq!(value, json!([false, true]));

        host.reset_cancellation();
        let value = probe(host, "utils.sleep(5)");
        assert_eq!(value, json!(true));
    }

    #[test]
    fn matching_reports_a_similarity_not_a_boolean() {
        let value = default_probe(
            "[matching.compareStrings('Song Name', 'song name'), matching.compareStrings('Song Name', 'Song Nam'), matching.compareStrings('Song Name (Remastered)', 'Song Name'), matching.compareStrings('A', 'Z')]",
        );

        let values = value.as_array().expect("array");
        assert_eq!(values[0].as_f64(), Some(1.0));
        assert!(
            values[1].as_f64().unwrap_or(0.0) > 0.8,
            "one missing character must stay above 0.8, got {}",
            values[1]
        );
        assert_eq!(
            values[2].as_f64(),
            Some(1.0),
            "a bracketed suffix must not change the comparison"
        );
        assert_eq!(values[3].as_f64(), Some(0.0));
    }

    #[test]
    fn filenames_are_sanitised_and_templates_filled() {
        let value = default_probe(
            "[gobackend.sanitizeFilename('a/b\\\\c:d*e?.flac'), gobackend.buildFilename('{artist} - {track_number} {title}', { artist: 'AC/DC', track_number: 3, title: 'Back In Black' })]",
        );

        assert_eq!(
            value,
            json!(["a_b_c_d_e_.flac", "AC_DC - 3 Back In Black"])
        );
    }

    #[test]
    fn unimplemented_surfaces_name_themselves() {
        for (expression, needle) in [
            ("auth.openAuthUrl('https://x.test')", "auth.openAuthUrl"),
        ] {
            let value = default_probe(&format!(
                "(function () {{ try {{ {expression}; return 'allowed'; }} catch (error) {{ return error.message; }} }})()"
            ));
            let message = value.as_str().expect("message");
            assert!(
                message.contains(needle),
                "{expression} reported {message:?}, expected {needle:?}"
            );
        }
    }

    #[test]
    fn ffmpeg_without_a_binary_returns_a_typed_unsupported_result() {
        let value = default_probe(
            "(function () { var r = ffmpeg.execute('-i a.flac b.mp3'); return [r.success, r.error_type, r.error]; })()",
        );

        let fields = value.as_array().expect("array");
        assert_eq!(fields[0], json!(false));
        assert_eq!(fields[1], json!("unsupported"));
        assert!(
            fields[2].as_str().unwrap_or_default().contains("not configured"),
            "unexpected error text: {}",
            fields[2]
        );

        // `getInfo` is a named gap, so it says so rather than pretending.
        let value = default_probe(
            "(function () { var r = ffmpeg.getInfo('a.flac'); return [r.success, r.error_type]; })()",
        );
        assert_eq!(value, json!([false, "unsupported"]));
    }

    #[test]
    fn utils_tolerate_a_byte_array_and_a_string_alike() {
        let value = default_probe(
            "[utils.hmacSHA1('abc', 'x').length, utils.hmacSHA1([0x61,0x62,0x63], 'x').length]",
        );
        assert_eq!(value, json!([20, 20]));
    }

    #[test]
    fn storage_rejects_a_write_that_would_exceed_the_limit() {
        let storage = Storage::in_memory();
        let huge = "x".repeat(MAX_STORAGE_BYTES + 1);

        let error = storage
            .set("big", Value::String(huge))
            .expect_err("must be refused");
        assert_eq!(
            error,
            StorageError::TooLarge {
                limit: MAX_STORAGE_BYTES
            }
        );
        assert_eq!(storage.get("big"), None, "a refused write must not stick");
    }

    #[test]
    fn corrupt_storage_is_preserved_not_destroyed() {
        let directory = tempfile::tempdir().expect("temp dir");
        fs::write(directory.path().join(STORAGE_FILE), b"{not json").expect("write");

        let storage = Storage::at(directory.path());
        assert!(storage.is_empty());
        assert!(
            directory.path().join(STORAGE_CORRUPT_FILE).is_file(),
            "the unreadable file must be kept"
        );

        storage
            .set("k", json!(1))
            .expect("writes after recovery");
        assert_eq!(storage.get("k"), Some(json!(1)));
    }

    #[test]
    fn a_sleep_is_interrupted_by_cancellation() {
        let host = Arc::new(HostServices::default());
        let canceller = Arc::clone(&host);

        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            canceller.cancel();
        });

        let started = Instant::now();
        assert!(!host.sleep_ms(5_000), "a cancelled sleep must report false");
        assert!(
            started.elapsed() < Duration::from_millis(1_000),
            "sleep woke only after {:?}",
            started.elapsed()
        );

        handle.join().expect("canceller thread");
    }
}
