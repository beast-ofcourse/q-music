//! Post-processing hooks: a package's last chance to change the bytes before
//! the host embeds metadata.
//!
//! A hook runs between two host-owned stages. The download has finished and a
//! real file sits at a path the host chose; embedding has not started, so
//! nothing has been written to the file's tags yet. That leaves exactly one job
//! for this module: invoke the package, decide what its answer means, and record
//! a failure on the queue without ever destroying the file it was given.
//!
//! Three rules the tests enforce:
//!
//! - **`postProcessV2` wins when both exist.** It is the documented forward
//!   shape and the one a package will keep maintaining.
//! - **Success is verified, not trusted.** A hook that returns
//!   `{success: true}` for a path that does not exist is a failure; the host
//!   reported a file that is not there, and the next stage would fail obscurely.
//! - **A failed hook leaves the file.** Post-processing is a transformation of
//!   something the user already downloaded; a broken hook must not cost them the
//!   download as well.
//!
//! `file.*` access during the call is a scoped grant: the host opens the file's
//! own directory for the duration of the hook, because the package is being
//! asked to operate on a path it never chose itself.

use crate::adapter::{self, DownloadFailureKind};
use crate::manifest::PostProcessHook;
use crate::queue::{DownloadQueue, JobState};
use crate::runtime::ExtensionRuntime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Forward-compatible entry point, called first when a package has both.
pub const FUNCTION_V2: &str = "postProcessV2";
/// Original entry point.
pub const FUNCTION_V1: &str = "postProcess";

/// One file to hand to a hook.
#[derive(Debug, Clone, PartialEq)]
pub struct PostProcessRequest {
    pub file_path: PathBuf,
    pub metadata: Value,
    /// Overrides the extension-derived type; packages never set this.
    pub mime_type: Option<String>,
    /// Always false here: this host has no content-URI storage layer.
    pub is_saf: bool,
}

impl PostProcessRequest {
    pub fn new(file_path: impl Into<PathBuf>) -> Self {
        Self {
            file_path: file_path.into(),
            metadata: Value::Object(Map::new()),
            mime_type: None,
            is_saf: false,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    fn mime_type(&self) -> String {
        if let Some(explicit) = &self.mime_type {
            return explicit.clone();
        }
        mime_for(&self.file_path).to_string()
    }

    /// The `postProcessV2` first argument, exactly as documented.
    fn input_object(&self) -> Value {
        let size = std::fs::metadata(&self.file_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        json!({
            "path": self.file_path.to_string_lossy(),
            "uri": "",
            "name": self
                .file_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "mime_type": self.mime_type(),
            "size": size,
            "is_saf": self.is_saf,
        })
    }
}

/// What one hook call produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum PostProcessOutcome {
    /// The hook finished and the file it named exists.
    Success { file_path: PathBuf },
    /// The package asked for something this host does not provide, such as an
    /// ffmpeg binary that was never configured. Named so the UI can say why.
    Unsupported { message: String },
    /// The hook ran and refused, threw, timed out, or lied about its result.
    Failed {
        kind: DownloadFailureKind,
        message: String,
    },
}

impl PostProcessOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub fn file_path(&self) -> Option<&Path> {
        match self {
            Self::Success { file_path } => Some(file_path),
            _ => None,
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Success { .. } => None,
            Self::Unsupported { message } | Self::Failed { message, .. } => Some(message),
        }
    }

    /// The failure kind the queue records, if this outcome is not a success.
    pub fn failure_kind(&self) -> Option<DownloadFailureKind> {
        match self {
            Self::Success { .. } => None,
            Self::Unsupported { .. } => Some(DownloadFailureKind::Unavailable),
            Self::Failed { kind, .. } => Some(kind.clone()),
        }
    }
}

/// Invoke one hook. The stage grant is opened for the file's directory and
/// cleared before this function returns.
pub fn run(
    runtime: &ExtensionRuntime,
    hook: &PostProcessHook,
    request: &PostProcessRequest,
) -> PostProcessOutcome {
    let function = if runtime.supports(FUNCTION_V2) {
        FUNCTION_V2
    } else if runtime.supports(FUNCTION_V1) {
        FUNCTION_V1
    } else {
        return PostProcessOutcome::Unsupported {
            message: format!(
                "{} declares post-processing but registers neither `{FUNCTION_V2}` nor `{FUNCTION_V1}`",
                runtime.name()
            ),
        };
    };

    let args = if function == FUNCTION_V2 {
        vec![
            request.input_object(),
            request.metadata.clone(),
            Value::String(hook.id.clone()),
        ]
    } else {
        vec![
            Value::String(request.file_path.to_string_lossy().into_owned()),
            request.metadata.clone(),
            Value::String(hook.id.clone()),
        ]
    };

    // Not an error when the parent cannot be opened: a hook that only inspects
    // its arguments still works, and one that needs the file reports the denial
    // itself. A stage left over from an earlier call is impossible: the guard
    // clears it on drop, and calls on one runtime are serialised.
    let _stage = request
        .file_path
        .parent()
        .and_then(|directory| runtime.host().open_stage(directory).ok());

    let value = match adapter::call(runtime, function, &args) {
        Ok(value) => value,
        Err(error) => {
            return PostProcessOutcome::Failed {
                kind: DownloadFailureKind::PostProcess,
                message: format!("{function} failed: {error}"),
            }
        }
    };

    interpret(value, request, function, &hook.id)
}

/// Run one hook for a queued job and record the result on the job.
///
/// A success leaves the job in `PostProcessing`: the embedding stage owns the
/// next transition. A failure is recorded with `stage = PostProcessing`, which
/// is what the queue drawer shows and what a retry reads.
pub fn run_for_job(
    runtime: &ExtensionRuntime,
    queue: &mut DownloadQueue,
    job_id: &str,
    hook: &PostProcessHook,
    request: &PostProcessRequest,
) -> PostProcessOutcome {
    if let Some(job) = queue.job(job_id) {
        if job.state == JobState::Downloading {
            let _ = queue.transition(job_id, JobState::PostProcessing);
        }
    }

    let outcome = run(runtime, hook, request);

    match &outcome {
        PostProcessOutcome::Success { file_path } => {
            // A conversion hook legitimately changes the extension, and every
            // later stage has to look at the new name.
            if file_path != &request.file_path {
                let _ = queue.retarget(job_id, file_path.clone());
            }
        }
        PostProcessOutcome::Unsupported { message } => {
            let _ = queue.fail(job_id, DownloadFailureKind::Unavailable, message.clone());
        }
        PostProcessOutcome::Failed { kind, message } => {
            let _ = queue.fail(job_id, kind.clone(), message.clone());
        }
    }

    outcome
}

/// Turn a package's return value into an outcome, verifying the path it names.
fn interpret(
    value: Value,
    request: &PostProcessRequest,
    function: &str,
    hook_id: &str,
) -> PostProcessOutcome {
    let Some(object) = value.as_object() else {
        return PostProcessOutcome::Failed {
            kind: DownloadFailureKind::PostProcess,
            message: format!(
                "{function} returned {} instead of a result object",
                describe(&value)
            ),
        };
    };

    if !is_true(object.get("success")) {
        let hint = object.get("error_type").and_then(Value::as_str);
        let message = error_text(object).unwrap_or_else(|| {
            format!("{function} reported failure without a reason for hook {hook_id:?}")
        });

        if matches!(hint, Some("unsupported" | "not_supported" | "unavailable")) {
            return PostProcessOutcome::Unsupported { message };
        }

        return PostProcessOutcome::Failed {
            kind: hint
                .map(|hint| DownloadFailureKind::from_hint(Some(hint)))
                .filter(|kind| !matches!(kind, DownloadFailureKind::Unknown))
                .unwrap_or(DownloadFailureKind::PostProcess),
            message,
        };
    }

    let requested_path = match named_path(object) {
        Some(named) => match resolve_named(&named, &request.file_path) {
            Ok(path) => path,
            Err(message) => {
                return PostProcessOutcome::Failed {
                    kind: DownloadFailureKind::PostProcess,
                    message,
                }
            }
        },
        None => request.file_path.clone(),
    };

    if !requested_path.is_file() {
        return PostProcessOutcome::Failed {
            kind: DownloadFailureKind::PostProcess,
            message: format!(
                "{function} reported success for hook {:?} but {} is not a file",
                hook_id,
                requested_path.display()
            ),
        };
    }

    PostProcessOutcome::Success {
        file_path: requested_path,
    }
}

/// The path a hook claims it produced, if it named one.
fn named_path(object: &Map<String, Value>) -> Option<String> {
    ["new_file_path", "newFilePath", "file_path", "path"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_string)
}

fn resolve_named(named: &str, requested: &Path) -> Result<PathBuf, String> {
    let path = PathBuf::from(named);

    // A relative answer is read as relative to the file the hook was given,
    // which is the only directory the package was granted.
    let path = if path.is_absolute() {
        path
    } else {
        match requested.parent() {
            Some(directory) => directory.join(path),
            None => {
                return Err(format!(
                    "hook named {named:?} but there is no directory to resolve it against"
                ))
            }
        }
    };

    Ok(path)
}

fn is_true(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(true)) => true,
        Some(Value::Number(number)) => number.as_f64().map(|value| value != 0.0).unwrap_or(false),
        Some(Value::String(text)) => text.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn error_text(object: &Map<String, Value>) -> Option<String> {
    ["error", "error_message", "message"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_string)
}

fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(_) => "a string".to_string(),
        Value::Array(_) => "an array".to_string(),
        Value::Object(_) => "an object".to_string(),
    }
}

/// Content type handed to `postProcessV2` for the formats this host downloads.
fn mime_for(path: &Path) -> &'static str {
    match extension_of(path).as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "alac" => "audio/alac",
        _ => "application/octet-stream",
    }
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, PostProcessHook};
    use crate::queue::JobSpec;
    use crate::runtime::{ExtensionRuntime, RuntimeLimits};
    use std::fs;
    use std::sync::Arc;
    use crate::runtime::polyfills::{HostConfig, HostServices};

    fn runtime_with(source: &str) -> ExtensionRuntime {
        let host = Arc::new(HostServices::new(HostConfig::default()));
        ExtensionRuntime::load_with_host("fixture-postprocess", source, RuntimeLimits::default(), host)
            .expect("runtime loads")
    }

    fn hook(id: &str) -> PostProcessHook {
        PostProcessHook {
            id: id.to_string(),
            name: Some(id.to_string()),
            default_enabled: true,
            ..PostProcessHook::default()
        }
    }

    fn queued_job(output: &Path) -> DownloadQueue {
        let mut queue = DownloadQueue::new(1);
        queue.enqueue(JobSpec::new("track-1", "fixture", "best", output));
        let id = queue.jobs()[0].id.clone();
        queue
            .transition(&id, JobState::Resolving)
            .expect("resolving");
        queue
            .begin_download(&id, output.with_extension("part"), Some(5))
            .expect("downloading");
        queue
    }

    const REAL_SHAPE: &str = r#"
        registerExtension({
          postProcess: function (filePath, metadata, hookId) {
            return { success: true, new_file_path: filePath };
          }
        });
    "#;

    #[test]
    fn a_no_op_hook_reports_success_and_keeps_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"audio").expect("seed");

        let runtime = runtime_with(REAL_SHAPE);
        let request = PostProcessRequest::new(&output);

        let outcome = run(&runtime, &hook("normalize"), &request);

        assert_eq!(
            outcome,
            PostProcessOutcome::Success {
                file_path: output.clone()
            }
        );
        assert_eq!(fs::read(&output).expect("file survives"), b"audio");
    }

    #[test]
    fn the_v2_entry_point_is_preferred_when_both_are_registered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"audio").expect("seed");

        let runtime = runtime_with(
            r#"
            registerExtension({
              postProcess: function () {
                throw new Error('v1 must not be called');
              },
              postProcessV2: function (input, metadata, hookId) {
                if (!input.path || input.mime_type !== 'audio/flac' || input.is_saf !== false) {
                  return { success: false, error: 'unexpected input shape' };
                }
                return { success: true, new_file_path: input.path };
              }
            });
            "#,
        );

        let outcome = run(
            &runtime,
            &hook("normalize"),
            &PostProcessRequest::new(&output),
        );

        assert!(outcome.is_success(), "{outcome:?}");
    }

    #[test]
    fn a_hook_that_throws_fails_the_job_and_leaves_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"audio").expect("seed");

        let runtime = runtime_with(
            r#"
            registerExtension({
              postProcess: function () { throw new Error('normalizer exploded'); }
            });
            "#,
        );
        let mut queue = queued_job(&output);
        let job_id = queue.jobs()[0].id.clone();

        let outcome = run_for_job(
            &runtime,
            &mut queue,
            &job_id,
            &hook("normalize"),
            &PostProcessRequest::new(&output),
        );

        match &outcome {
            PostProcessOutcome::Failed { kind, message } => {
                assert_eq!(kind, &DownloadFailureKind::PostProcess);
                assert!(message.contains("normalizer exploded"), "{message}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }

        let job = queue.job(&job_id).expect("job");
        assert_eq!(job.state, JobState::Failed);
        let failure = job.failure.as_ref().expect("failure recorded");
        assert_eq!(failure.stage, JobState::PostProcessing);
        assert_eq!(failure.kind, DownloadFailureKind::PostProcess);
        assert_eq!(
            fs::read(&output).expect("file survives"),
            b"audio",
            "a failed hook must not cost the download"
        );
    }

    #[test]
    fn ffmpeg_without_a_configured_binary_fails_the_job_as_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"audio").expect("seed");

        let runtime = runtime_with(
            r#"
            registerExtension({
              postProcess: function (filePath, metadata, hookId) {
                var result = ffmpeg.execute('-i "' + filePath + '" -af loudnorm -y "' + filePath + '.tmp"');
                if (!result.success) {
                  return { success: false, error: result.error, error_type: result.error_type };
                }
                return { success: true, new_file_path: filePath };
              }
            });
            "#,
        );
        let mut queue = queued_job(&output);
        let job_id = queue.jobs()[0].id.clone();

        let outcome = run_for_job(
            &runtime,
            &mut queue,
            &job_id,
            &hook("normalize"),
            &PostProcessRequest::new(&output),
        );

        match &outcome {
            PostProcessOutcome::Unsupported { message } => {
                assert!(message.contains("not configured"), "{message}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(
            outcome.failure_kind(),
            Some(DownloadFailureKind::Unavailable)
        );

        let job = queue.job(&job_id).expect("job");
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(
            job.failure.as_ref().expect("failure").stage,
            JobState::PostProcessing
        );
        assert_eq!(
            fs::read(&output).expect("file survives"),
            b"audio",
            "the download is kept when the host lacks a tool"
        );
        assert!(!dir.path().join("track.flac.tmp").exists());
    }

    #[test]
    fn a_success_claim_for_a_missing_file_is_a_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"audio").expect("seed");
        let elsewhere = dir.path().join("never-written.mp3");

        let runtime = runtime_with(&format!(
            r#"
            registerExtension({{
              postProcess: function () {{
                return {{ success: true, new_file_path: {missing:?} }};
              }}
            }});
            "#,
            missing = elsewhere.to_string_lossy()
        ));

        let outcome = run(
            &runtime,
            &hook("convert_mp3"),
            &PostProcessRequest::new(&output),
        );

        match outcome {
            PostProcessOutcome::Failed { message, .. } => {
                assert!(message.contains("is not a file"), "{message}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn a_conversion_retargets_the_job_to_the_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"flac bytes").expect("seed");

        let runtime = runtime_with(
            r#"
            registerExtension({
              postProcess: function (filePath) {
                var converted = filePath.replace('.flac', '.mp3');
                file.write(converted, 'mp3 bytes');
                file['delete'](filePath);
                return { success: true, new_file_path: converted };
              }
            });
            "#,
        );
        let mut queue = queued_job(&output);
        let job_id = queue.jobs()[0].id.clone();

        let outcome = run_for_job(
            &runtime,
            &mut queue,
            &job_id,
            &hook("convert_mp3"),
            &PostProcessRequest::new(&output),
        );

        let converted = dir.path().join("track.mp3");
        assert_eq!(
            outcome,
            PostProcessOutcome::Success {
                file_path: converted.clone()
            }
        );
        assert!(!output.exists(), "the hook deleted the original");
        assert_eq!(fs::read(&converted).expect("converted"), b"mp3 bytes");
        assert_eq!(
            queue.job(&job_id).expect("job").output_path,
            converted,
            "the queue must follow the file the hook produced"
        );
    }

    #[test]
    fn a_file_grant_is_scoped_to_the_hook_and_cleared_afterwards() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"audio").expect("seed");

        let runtime = runtime_with(
            r#"
            registerExtension({
              postProcess: function (filePath) {
                var allowed = file.exists(filePath);
                var refused = file.exists('C:/definitely/elsewhere.flac');
                return { success: true, can_read: allowed, can_read_outside: refused };
              }
            });
            "#,
        );

        let value = adapter::call(
            &runtime,
            FUNCTION_V1,
            &[
                json!(output.to_string_lossy()),
                json!({}),
                json!("normalize"),
            ],
        )
        .expect("direct call without a grant");

        assert_eq!(
            value["can_read"], json!(false),
            "no stage is open outside `run`, so even the file itself is denied"
        );
        assert_eq!(value["can_read_outside"], json!(false));

        // Through `run`, the same call reaches the file it was handed.
        let runtime = runtime_with(
            r#"
            registerExtension({
              postProcess: function (filePath) {
                return { success: true, saw_file: file.exists(filePath) };
              }
            });
            "#,
        );
        let outcome = run(
            &runtime,
            &hook("normalize"),
            &PostProcessRequest::new(&output),
        );
        assert!(outcome.is_success(), "{outcome:?}");
        assert_eq!(adapter::call(&runtime, FUNCTION_V1, &[json!(output.to_string_lossy()), json!({}), json!("x")]).expect("after")["saw_file"], json!(false));
    }

    #[test]
    fn a_package_without_either_entry_point_is_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        fs::write(&output, b"audio").expect("seed");

        let runtime = runtime_with("registerExtension({ initialize: function () {} });");

        let outcome = run(
            &runtime,
            &hook("normalize"),
            &PostProcessRequest::new(&output),
        );

        match outcome {
            PostProcessOutcome::Unsupported { message } => {
                assert!(message.contains("neither"), "{message}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn the_documented_manifest_shape_parses() {
        let manifest: Manifest = serde_json::from_value(json!({
            "name": "fixture",
            "version": "1.0.0",
            "postProcessing": {
                "enabled": true,
                "hooks": [
                    {
                        "id": "convert_mp3",
                        "name": "Convert to MP3",
                        "description": "Convert FLAC to MP3 320kbps",
                        "defaultEnabled": false,
                        "supportedFormats": ["flac"]
                    },
                    {
                        "id": "normalize",
                        "name": "Normalize Audio",
                        "description": "Apply ReplayGain normalization",
                        "defaultEnabled": true,
                        "supportedFormats": ["flac", "mp3"]
                    }
                ]
            }
        }))
        .expect("manifest parses");

        let post = manifest.post_processing.expect("post-processing present");
        assert!(post.is_active());
        assert_eq!(post.hooks.len(), 2);
        assert_eq!(post.hook("convert_mp3").expect("hook").name.as_deref(), Some("Convert to MP3"));
        assert!(!post.hook("convert_mp3").expect("hook").default_enabled);
        assert!(post.hook("normalize").expect("hook").default_enabled);

        let for_flac: Vec<_> = post.hooks_for("flac").iter().map(|hook| hook.id.as_str()).collect();
        assert_eq!(for_flac, vec!["normalize"], "disabled hooks are not offered");
        assert!(post.hooks_for("ogg").is_empty());
    }

    #[test]
    fn an_absent_post_processing_block_is_inert() {
        let manifest: Manifest =
            serde_json::from_value(json!({ "name": "fixture", "version": "1.0.0" }))
                .expect("parses");
        assert!(manifest.post_processing.is_none());

        // An unknown key inside the block survives a round trip.
        let manifest: Manifest = serde_json::from_value(json!({
            "name": "fixture",
            "version": "1.0.0",
            "postProcessing": { "enabled": true, "futureKey": 7, "hooks": [] }
        }))
        .expect("parses");
        let post = manifest.post_processing.expect("present");
        assert!(!post.is_active());
        assert_eq!(post.extra.get("futureKey"), Some(&json!(7)));
    }
}
