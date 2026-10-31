//! Sandboxed JavaScript runtime: one OS thread per extension.
//!
//! Why a thread per extension rather than a shared pool: extension code is
//! synchronous, untrusted and may block on the network for seconds. Giving each
//! package its own thread keeps one slow provider from stalling another, and
//! makes teardown a thread join instead of a shared-state cleanup.
//!
//! Why the JSON bridge: values cross the boundary as JSON strings rather than
//! through typed bindings. Every shape an extension returns is JSON by
//! definition, so this keeps the host free of per-type conversion code while
//! still letting Rust parse and validate the result.
//!
//! Lifecycle: [`ExtensionRuntime::load`] is the only constructor. It evaluates
//! the package source, captures what `registerExtension()` received, and
//! returns once the thread is ready. Nothing is created at app startup.

use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

use crate::runtime::polyfills::{CallClock, HostServices};

pub mod crypto;
pub mod ffmpeg;
pub mod http;
pub mod permissions;
pub mod polyfills;
#[cfg(test)]
pub(crate) mod testing;

pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 128 * 1024 * 1024;
pub const DEFAULT_STACK_LIMIT_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("extension did not call registerExtension()")]
    NotRegistered,

    #[error("extension does not implement `{function}`")]
    MissingFunction { function: String },

    #[error("extension function `{function}` exceeded its {timeout_ms} ms deadline")]
    Timeout { function: String, timeout_ms: u64 },

    #[error("extension function `{function}` threw: {message}")]
    Threw { function: String, message: String },

    #[error("extension runtime failed to start: {0}")]
    Start(String),

    #[error("extension runtime is not running")]
    Stopped,

    #[error("extension returned invalid data: {0}")]
    InvalidResult(String),
}

/// Resource ceilings for one extension thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub call_timeout: Duration,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            memory_bytes: DEFAULT_MEMORY_LIMIT_BYTES,
            stack_bytes: DEFAULT_STACK_LIMIT_BYTES,
            call_timeout: DEFAULT_CALL_TIMEOUT,
        }
    }
}

/// One function a package exposed through `registerExtension()`.
///
/// `arity` is the declared parameter count, which is how a 2-argument
/// `download` is told apart from the 5-argument form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionInfo {
    pub name: String,
    pub arity: u32,
}

enum Job {
    Call {
        function: String,
        args_json: String,
        reply: Sender<Result<String, RuntimeError>>,
    },
    Shutdown,
}

/// Handle to a running extension thread.
///
/// Calls go through a channel, so one runtime can be driven from any thread.
/// The channel sender is behind a mutex because `Sender` itself is not `Sync`.
pub struct ExtensionRuntime {
    name: String,
    tx: Mutex<Sender<Job>>,
    join: Mutex<Option<JoinHandle<()>>>,
    surface: Vec<FunctionInfo>,
    call_timeout: Duration,
    host: Arc<HostServices>,
}

impl ExtensionRuntime {
    /// Start a thread and load `source` into it with a default host surface.
    ///
    /// Returns only once the package has been evaluated and its surface read,
    /// so a caller never holds a handle to a runtime that failed to boot.
    pub fn load(
        name: impl Into<String>,
        source: &str,
        limits: RuntimeLimits,
    ) -> Result<Self, RuntimeError> {
        Self::load_with_host(name, source, limits, Arc::new(HostServices::default()))
    }

    /// Start a thread with an explicit host surface.
    ///
    /// `host` carries the network client, sandbox and storage directory, so a
    /// caller that wants a package to reach the outside world must say so
    /// explicitly. The default host can do neither.
    pub fn load_with_host(
        name: impl Into<String>,
        source: &str,
        limits: RuntimeLimits,
        host: Arc<HostServices>,
    ) -> Result<Self, RuntimeError> {
        let name = name.into();
        let (job_tx, job_rx) = channel();
        let (ready_tx, ready_rx) = channel();

        let source = source.to_string();
        let thread_name = format!("qext-{}", name.replace(|ch: char| !ch.is_ascii_alphanumeric(), "-"));
        let thread_host = Arc::clone(&host);

        let join = thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let result = run_thread(&source, limits, thread_host, job_rx, &ready_tx);
                if let Err(error) = result {
                    // Startup failures are reported through `ready_tx`; a
                    // failure after startup can only mean the thread died.
                    let _ = ready_tx.send(Err(error));
                }
            })
            .map_err(|source| RuntimeError::Start(format!("cannot spawn runtime thread: {source}")))?;

        let surface = match ready_rx.recv() {
            Ok(Ok(surface)) => surface,
            Ok(Err(error)) => {
                let _ = join.join();
                return Err(error);
            }
            Err(_) => {
                let _ = join.join();
                return Err(RuntimeError::Start(
                    "extension thread exited before reporting readiness".to_string(),
                ));
            }
        };

        Ok(Self {
            name,
            join: Mutex::new(Some(join)),
            tx: Mutex::new(job_tx),
            surface,
            call_timeout: limits.call_timeout,
            host,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The host surface this runtime was started with.
    ///
    /// A host-initiated stage (post-processing) uses it to open a scoped file
    /// grant for the directory it is about to hand to the package.
    pub fn host(&self) -> &Arc<HostServices> {
        &self.host
    }

    /// Functions the package registered, in registration order.
    pub fn surface(&self) -> &[FunctionInfo] {
        &self.surface
    }

    pub fn supports(&self, function: &str) -> bool {
        self.surface.iter().any(|entry| entry.name == function)
    }

    pub fn arity(&self, function: &str) -> Option<u32> {
        self.surface
            .iter()
            .find(|entry| entry.name == function)
            .map(|entry| entry.arity)
    }

    /// Invoke `function` with JSON arguments, returning its parsed result.
    pub fn call(&self, function: &str, args: &[Value]) -> Result<Value, RuntimeError> {
        if !self.supports(function) {
            return Err(RuntimeError::MissingFunction {
                function: function.to_string(),
            });
        }

        let args_json = serde_json::to_string(args)
            .map_err(|error| RuntimeError::InvalidResult(error.to_string()))?;

        let (reply_tx, reply_rx) = channel();
        self.tx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .send(Job::Call {
                function: function.to_string(),
                args_json,
                reply: reply_tx,
            })
            .map_err(|_| RuntimeError::Stopped)?;

        let raw = reply_rx.recv().map_err(|_| RuntimeError::Stopped)??;

        serde_json::from_str(&raw).map_err(|error| RuntimeError::InvalidResult(error.to_string()))
    }

    /// Stop the thread and wait for it. Idempotent.
    pub fn shutdown(&mut self) {
        let _ = self
            .tx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .send(Job::Shutdown);
        let join = self
            .join
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(join) = join {
            let _ = join.join();
        }
    }

    /// Declared deadline for a single call.
    pub fn call_timeout(&self) -> Duration {
        self.call_timeout
    }
}

impl Drop for ExtensionRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for ExtensionRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionRuntime")
            .field("name", &self.name)
            .field("surface", &self.surface)
            .finish()
    }
}

fn run_thread(
    source: &str,
    limits: RuntimeLimits,
    host: Arc<HostServices>,
    jobs: Receiver<Job>,
    ready: &Sender<Result<Vec<FunctionInfo>, RuntimeError>>,
) -> Result<(), RuntimeError> {
    let runtime = Runtime::new().map_err(|error| RuntimeError::Start(error.to_string()))?;
    runtime.set_memory_limit(limits.memory_bytes);
    runtime.set_max_stack_size(limits.stack_bytes);

    let base = Instant::now();
    let deadline_ms = Arc::new(AtomicU64::new(0));
    let tripped = Arc::new(AtomicBool::new(false));

    {
        let deadline_ms = Arc::clone(&deadline_ms);
        let tripped = Arc::clone(&tripped);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let deadline = deadline_ms.load(Ordering::Relaxed);
            if deadline == 0 {
                return false;
            }
            if base.elapsed().as_millis() as u64 >= deadline {
                tripped.store(true, Ordering::Relaxed);
                return true;
            }
            false
        })));
    }

    // Packages read the remaining-time budget through `utils`, so the host
    // needs the same clock the interrupt handler watches.
    host.bind_clock(CallClock::new(base, Arc::clone(&deadline_ms)));

    let context = Context::full(&runtime)
        .map_err(|error| RuntimeError::Start(format!("cannot create JS context: {error}")))?;

    // Load the package and read its surface.
    let surface = context.with(|ctx| {
        polyfills::install(&ctx, Arc::clone(&host))
            .map_err(|error| RuntimeError::Start(format!("host polyfills failed: {error}")))?;

        ctx.eval::<(), _>(PRELUDE)
            .map_err(|error| RuntimeError::Start(format!("host prelude failed: {error}")))?;

        ctx.eval::<(), _>(source)
            .map_err(|error| RuntimeError::Start(format!("package failed to load: {error}")))?;

        let globals = ctx.globals();
        let read_surface: Function = globals
            .get("__qext_surface")
            .map_err(|error| RuntimeError::Start(format!("surface reader missing: {error}")))?;

        let encoded: Option<String> = read_surface
            .call(())
            .map_err(|error| RuntimeError::Start(format!("surface read failed: {error}")))?;

        let encoded = encoded.ok_or(RuntimeError::NotRegistered)?;
        serde_json::from_str(&encoded)
            .map_err(|error| RuntimeError::Start(format!("surface is not valid JSON: {error}")))
    })?;

    if ready.send(Ok(surface)).is_err() {
        return Ok(());
    }

    while let Ok(job) = jobs.recv() {
        match job {
            Job::Shutdown => break,
            Job::Call {
                function,
                args_json,
                reply,
            } => {
                let timeout_ms = limits.call_timeout.as_millis() as u64;
                deadline_ms.store(base.elapsed().as_millis() as u64 + timeout_ms, Ordering::Relaxed);
                tripped.store(false, Ordering::Relaxed);

                let result = context.with(|ctx| invoke(&ctx, &function, &args_json, &tripped, timeout_ms));

                deadline_ms.store(0, Ordering::Relaxed);
                let _ = reply.send(result);
            }
        }
    }

    Ok(())
}

fn invoke(
    ctx: &rquickjs::Ctx<'_>,
    function: &str,
    args_json: &str,
    tripped: &AtomicBool,
    timeout_ms: u64,
) -> Result<String, RuntimeError> {
    let globals = ctx.globals();
    let bridge: Function = globals
        .get("__qext_call")
        .map_err(|error| RuntimeError::Start(format!("call bridge missing: {error}")))?;

    match bridge.call::<_, String>((function, args_json)) {
        Ok(encoded) => Ok(encoded),
        Err(error) => {
            if tripped.load(Ordering::Relaxed) {
                return Err(RuntimeError::Timeout {
                    function: function.to_string(),
                    timeout_ms,
                });
            }

            let message = ctx
                .catch()
                .as_exception()
                .and_then(|exception| exception.message())
                .unwrap_or_else(|| error.to_string());

            Err(RuntimeError::Threw {
                function: function.to_string(),
                message,
            })
        }
    }
}

/// Host surface available to packages before the rest of the host is built.
///
/// Every stub that could reach the outside world throws, so a package that
/// performs I/O while loading fails loudly instead of silently doing nothing.
/// The minimum a package needs to register and be invoked.
///
/// The rest of the host surface - `http`, `storage`, `file`, `utils`,
/// `gobackend`, `matching`, `fetch` and the URL/text encoding builtins - is
/// installed by [`polyfills`], which must run before this prelude but does not
/// depend on it.
const PRELUDE: &str = r#"
var __qext_registered = null;

function registerExtension(api) {
  if (api === null || typeof api !== 'object') {
    throw new Error('registerExtension() expects an object');
  }
  __qext_registered = api;
  return true;
}

function __qext_surface() {
  if (!__qext_registered) {
    return null;
  }
  var entries = [];
  for (var key in __qext_registered) {
    if (Object.prototype.hasOwnProperty.call(__qext_registered, key)) {
      var value = __qext_registered[key];
      entries.push({ name: key, arity: typeof value === 'function' ? value.length : 0 });
    }
  }
  return JSON.stringify(entries);
}

function __qext_revive(value) {
  if (value === null || typeof value !== 'object') {
    return value;
  }

  // A function cannot cross the JSON bridge, so the host sends a marker and the
  // package receives a real callback that forwards its arguments back.
  if (value.__qext_callback__ !== undefined) {
    var kind = value.__qext_callback__;
    return function () {
      if (typeof __qext_progress !== 'function') {
        return;
      }
      var forwarded = [];
      for (var index = 0; index < arguments.length; index++) {
        forwarded.push(arguments[index]);
      }
      __qext_progress(JSON.stringify({ kind: kind, args: forwarded }));
    };
  }

  if (Array.isArray(value)) {
    var items = [];
    for (var itemIndex = 0; itemIndex < value.length; itemIndex++) {
      items.push(__qext_revive(value[itemIndex]));
    }
    return items;
  }

  var mapped = {};
  for (var key in value) {
    if (Object.prototype.hasOwnProperty.call(value, key)) {
      mapped[key] = __qext_revive(value[key]);
    }
  }
  return mapped;
}

function __qext_call(name, argsJson) {
  if (!__qext_registered) {
    throw new Error('extension did not call registerExtension()');
  }
  var fn = __qext_registered[name];
  if (typeof fn !== 'function') {
    throw new Error('extension does not implement ' + name);
  }
  var result = fn.apply(null, __qext_revive(JSON.parse(argsJson)));
  if (result === undefined) {
    return 'null';
  }
  return JSON.stringify(result);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testing::fixture_source;

    fn surface_names(runtime: &ExtensionRuntime) -> Vec<String> {
        runtime
            .surface()
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    }

    #[test]
    fn loads_spotify_web_and_reports_its_surface() {
        let runtime = ExtensionRuntime::load(
            "spotify-web",
            &fixture_source("spotify-web"),
            RuntimeLimits::default(),
        )
        .expect("loads");

        let mut names = surface_names(&runtime);
        names.sort();
        assert_eq!(
            names,
            vec![
                "cleanup",
                "customSearch",
                "enrichTrack",
                "getAlbum",
                "getArtist",
                "getBrowseCategories",
                "getHomeFeed",
                "getPlaylist",
                "getTrack",
                "handleUrl",
                "initialize",
                "searchTracks",
            ]
        );

        assert!(runtime.supports("initialize"));
        assert!(!runtime.supports("download"));
    }

    #[test]
    fn loads_amazon_and_reports_download_arity() {
        let runtime =
            ExtensionRuntime::load("amazon", &fixture_source("amazon"), RuntimeLimits::default())
                .expect("loads");

        let mut names = surface_names(&runtime);
        names.sort();
        assert_eq!(
            names,
            vec![
                "checkAvailability",
                "cleanup",
                "completeGrant",
                "customSearch",
                "download",
                "enrichTrack",
                "getAlbum",
                "getArtist",
                "getDownloadUrl",
                "getHomeFeed",
                "getPlaylist",
                "getTrack",
                "handleUrl",
                "initialize",
                "matchTrack",
                "validateTrackForDownload",
            ]
        );

        // The shipped package uses the five-argument form; the guide documents
        // two arguments. The adapter needs the real number to dispatch.
        assert_eq!(runtime.arity("download"), Some(5));
        assert_eq!(runtime.arity("getTrack"), Some(1));
    }

    #[test]
    fn loads_tidal_web() {
        let runtime =
            ExtensionRuntime::load("tidal-web", &fixture_source("tidal-web"), RuntimeLimits::default())
                .expect("loads");

        assert!(runtime.supports("searchTracks"));
        assert!(runtime.supports("download"));
        assert_eq!(runtime.arity("download"), Some(5));
    }

    #[test]
    fn packages_do_not_reach_the_network_while_loading() {
        // The stubs throw on any I/O. A load that completes proves the package
        // touched nothing external at module scope.
        for name in ["spotify-web", "amazon", "tidal-web"] {
            ExtensionRuntime::load(name, &fixture_source(name), RuntimeLimits::default())
                .unwrap_or_else(|error| panic!("{name} tried to do I/O while loading: {error}"));
        }
    }

    #[test]
    fn invokes_a_registered_function() {
        let source = r#"
            registerExtension({
                echo: function (value) { return { got: value, doubled: value * 2 }; },
                nothing: function () { return undefined; },
                arityTwo: function (a, b) { return a + b; }
            });
        "#;

        let runtime = ExtensionRuntime::load("echo", source, RuntimeLimits::default()).expect("loads");

        let result = runtime.call("echo", &[serde_json::json!(21)]).expect("call");
        assert_eq!(result, serde_json::json!({ "got": 21, "doubled": 42 }));

        let result = runtime.call("nothing", &[]).expect("call");
        assert_eq!(result, serde_json::Value::Null);

        let result = runtime.call("arityTwo", &[serde_json::json!(1), serde_json::json!(2)]).expect("call");
        assert_eq!(result, serde_json::json!(3));
    }

    #[test]
    fn calling_an_unregistered_function_is_a_typed_error() {
        let runtime = ExtensionRuntime::load(
            "minimal",
            "registerExtension({ initialize: function () { return true; } });",
            RuntimeLimits::default(),
        )
        .expect("loads");

        let error = runtime.call("download", &[]).expect_err("missing");
        assert_eq!(
            error,
            RuntimeError::MissingFunction {
                function: "download".to_string()
            }
        );
    }

    #[test]
    fn package_without_register_extension_is_rejected() {
        let error = ExtensionRuntime::load("empty", "var x = 1;", RuntimeLimits::default())
            .expect_err("must be rejected");
        assert_eq!(error, RuntimeError::NotRegistered);
    }

    #[test]
    fn syntax_error_is_reported_at_load() {
        let error = ExtensionRuntime::load("broken", "registerExtension({", RuntimeLimits::default())
            .expect_err("must be rejected");
        assert!(matches!(error, RuntimeError::Start(_)), "got {error}");
    }

    #[test]
    fn throwing_function_reports_its_message() {
        let runtime = ExtensionRuntime::load(
            "thrower",
            r#"registerExtension({ fail: function () { throw new Error('boom'); } });"#,
            RuntimeLimits::default(),
        )
        .expect("loads");

        let error = runtime.call("fail", &[]).expect_err("throws");
        match error {
            RuntimeError::Threw { function, message } => {
                assert_eq!(function, "fail");
                assert!(message.contains("boom"), "message was {message:?}");
            }
            other => panic!("expected Threw, got {other:?}"),
        }
    }

    #[test]
    fn infinite_loop_is_interrupted_by_the_deadline() {
        let limits = RuntimeLimits {
            call_timeout: Duration::from_millis(250),
            ..RuntimeLimits::default()
        };

        let runtime = ExtensionRuntime::load(
            "looper",
            r#"registerExtension({ spin: function () { while (true) {} }, ping: function () { return 'pong'; } });"#,
            limits,
        )
        .expect("loads");

        let error = runtime.call("spin", &[]).expect_err("must time out");
        assert!(
            matches!(error, RuntimeError::Timeout { .. }),
            "expected Timeout, got {error:?}"
        );

        // The runtime stays usable after an interrupt.
        let result = runtime.call("ping", &[]).expect("call after interrupt");
        assert_eq!(result, serde_json::json!("pong"));
    }

    #[test]
    fn memory_limit_stops_runaway_allocation() {
        let limits = RuntimeLimits {
            memory_bytes: 16 * 1024 * 1024,
            call_timeout: Duration::from_millis(2000),
            ..RuntimeLimits::default()
        };

        let runtime = ExtensionRuntime::load(
            "hog",
            r#"registerExtension({
                hog: function () { var chunks = []; for (var i = 0; i < 4096; i++) { chunks.push('x'.repeat(65536)); } return chunks.length; },
                ping: function () { return 'alive'; }
            });"#,
            limits,
        )
        .expect("loads");

        assert!(runtime.call("hog", &[]).is_err(), "allocation must fail");

        let result = runtime.call("ping", &[]).expect("call after OOM");
        assert_eq!(result, serde_json::json!("alive"));
    }

    #[test]
    fn shutdown_is_idempotent_and_further_calls_are_rejected() {
        let mut runtime = ExtensionRuntime::load(
            "closer",
            "registerExtension({ ping: function () { return 1; } });",
            RuntimeLimits::default(),
        )
        .expect("loads");

        runtime.shutdown();
        runtime.shutdown();

        assert_eq!(runtime.call("ping", &[]), Err(RuntimeError::Stopped));
    }

    #[test]
    fn non_function_entries_are_reported_with_zero_arity() {
        let runtime = ExtensionRuntime::load(
            "values",
            "registerExtension({ metadata: { version: 2 }, initialize: function (a) { return a; } });",
            RuntimeLimits::default(),
        )
        .expect("loads");

        assert_eq!(runtime.arity("metadata"), Some(0));
        assert_eq!(runtime.arity("initialize"), Some(1));
    }
}



