//! Streamed file transfer: HTTP body to disk, resumable, cancellable.
//!
//! The queue decides *what* to download; this module moves the bytes. Keeping
//! them apart means every lifecycle rule is testable without a socket and every
//! transfer rule is testable without a queue.
//!
//! Invariants:
//!
//! - **Bytes land in a temp file first.** A job's temp path is
//!   `<output>.<job id>.part`, so two concurrent jobs can never write the same
//!   file, and a half-written file is never mistaken for a finished track.
//! - **Completion is a rename.** The final file appears only when every byte has
//!   been written and flushed, so a crash mid-transfer leaves the destination
//!   untouched.
//! - **Cancel removes everything it wrote.** A cancelled transfer deletes its
//!   temp file; there is no "resume later" for something the user cancelled.
//! - **Failure removes the partial unless it was a resumed one.** A fresh
//!   transfer that breaks deletes its temp file (a broken download is not worth
//!   keeping). A transfer that *appended* to an existing partial keeps it: those
//!   bytes were expensive and are exactly what the next attempt resumes from.
//! - **Progress never goes backwards.** The throttled updates the caller sees
//!   are monotonic in `written`, and the last one reports the final size.

use crate::adapter::DownloadFailureKind;
use crate::queue::{ProgressThrottle, ProgressUpdate};
use crate::runtime::permissions::NetworkAllowlist;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use thiserror::Error;
use ureq::http::{HeaderValue, Method, Request};
use url::Url;

/// Bytes read from the socket per iteration.
pub const DEFAULT_CHUNK_BYTES: usize = 64 * 1024;
/// Per-read socket timeout when the caller does not choose one.
pub const DEFAULT_READ_TIMEOUT_SECONDS: u64 = 30;
/// Ceiling for a single transfer, so a hostile server cannot fill the disk.
pub const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Longest job id accepted in a temp path.
pub const MAX_JOB_ID_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferLimits {
    /// Per socket read. Not a whole-transfer deadline: a large legitimate file
    /// may take minutes, while a stalled socket has to fail quickly.
    pub read_timeout: Duration,
    pub max_bytes: u64,
    pub chunk_bytes: usize,
    /// How often progress is forwarded.
    pub progress_interval: Duration,
}

impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_secs(DEFAULT_READ_TIMEOUT_SECONDS),
            max_bytes: DEFAULT_MAX_BYTES,
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            progress_interval: Duration::from_millis(crate::queue::PROGRESS_INTERVAL_MS),
        }
    }
}

/// One file to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    pub url: String,
    pub output_path: PathBuf,
    /// Distinguishes concurrent jobs writing into the same directory.
    pub job_id: String,
    pub headers: Vec<(String, String)>,
    /// Bytes already present in the temp file that should be kept. `0` starts
    /// from scratch and discards any stale temp file.
    pub resume_from: u64,
    /// Size the caller expects. Used when the server sends no `Content-Length`.
    pub expected_total: Option<u64>,
}

impl TransferPlan {
    pub fn new(
        url: impl Into<String>,
        output_path: impl Into<PathBuf>,
        job_id: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            output_path: output_path.into(),
            job_id: job_id.into(),
            headers: Vec::new(),
            resume_from: 0,
            expected_total: None,
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn resuming_from(mut self, offset: u64) -> Self {
        self.resume_from = offset;
        self
    }

    pub fn expecting(mut self, total: u64) -> Self {
        self.expected_total = Some(total);
        self
    }
}

/// What a finished transfer actually did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReport {
    pub bytes_written: u64,
    pub bytes_total: Option<u64>,
    /// Offset the transfer appended from; `0` means it started at the beginning.
    pub resumed_from: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransferError {
    #[error("{url:?} is not a usable http or https URL")]
    InvalidUrl { url: String },

    #[error("job id {job_id:?} cannot be used in a temp path")]
    InvalidJobId { job_id: String },

    #[error("network access denied: {host} is not declared by the provider")]
    Denied { host: String },

    #[error("server answered {status} for {url}")]
    Http { status: u16, url: String },

    #[error("server answered a range request with a different offset ({start})")]
    RangeMismatch { start: u64, expected: u64 },

    #[error("transfer timed out after {seconds} seconds without progress")]
    Timeout { seconds: u64 },

    #[error("transfer exceeded the {limit} byte limit")]
    TooLarge { limit: u64 },

    #[error("transfer ended after {received} of {expected} bytes")]
    ShortBody { expected: u64, received: u64 },

    #[error("transfer cancelled")]
    Cancelled,

    #[error("{path}: {message}")]
    Io { path: String, message: String },

    #[error("transfer failed: {0}")]
    Transport(String),
}

impl TransferError {
    /// The kind the queue records, so a log line and a job failure agree.
    pub fn failure_kind(&self) -> DownloadFailureKind {
        match self {
            Self::InvalidUrl { .. } | Self::InvalidJobId { .. } => DownloadFailureKind::InvalidInput,
            Self::Denied { .. } => DownloadFailureKind::Authentication,
            Self::Http { status, .. } if *status == 404 => DownloadFailureKind::NotFound,
            Self::Http { status, .. } if *status == 429 => DownloadFailureKind::RateLimited,
            Self::Timeout { .. } => DownloadFailureKind::Timeout,
            Self::Cancelled => DownloadFailureKind::Cancelled,
            _ => DownloadFailureKind::DownloadError,
        }
    }
}

/// The temp path for one job. Derived from the job id so two jobs targeting the
/// same output still write different files.
pub fn partial_path(output: &Path, job_id: &str) -> Result<PathBuf, TransferError> {
    if !is_safe_job_id(job_id) {
        return Err(TransferError::InvalidJobId {
            job_id: job_id.to_string(),
        });
    }

    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| TransferError::InvalidJobId {
            job_id: job_id.to_string(),
        })?;

    let mut partial = output.to_path_buf();
    partial.set_file_name(format!("{name}.{job_id}.part"));
    Ok(partial)
}

/// Job ids appear in a file name, so only characters that cannot change the
/// directory are accepted.
fn is_safe_job_id(job_id: &str) -> bool {
    !job_id.is_empty()
        && job_id.len() <= MAX_JOB_ID_LENGTH
        && job_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// Blocking HTTP transfer client.
///
/// One instance can run many transfers concurrently: `ureq::Agent` is `Sync` and
/// every transfer owns its file handle.
pub struct Downloader {
    agent: ureq::Agent,
    limits: TransferLimits,
    allowlist: Option<NetworkAllowlist>,
}

impl std::fmt::Debug for Downloader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Downloader")
            .field("limits", &self.limits)
            .field("allowlist", &self.allowlist)
            .finish()
    }
}

impl Downloader {
    pub fn new(limits: TransferLimits) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(10)
            .timeout_global(None)
            .timeout_per_read(Some(limits.read_timeout))
            .build()
            .into();

        Self {
            agent,
            limits,
            allowlist: None,
        }
    }

    /// Refuse hosts the provider did not declare. Host-initiated transfers use
    /// the same allowlist the package's own `http` calls go through.
    pub fn with_allowlist(mut self, allowlist: NetworkAllowlist) -> Self {
        self.allowlist = Some(allowlist);
        self
    }

    pub fn limits(&self) -> &TransferLimits {
        &self.limits
    }

    /// Fetch one plan. Cancellation is polled between chunks, so the flag may be
    /// set from another thread or from the progress callback.
    pub fn transfer<F>(
        &self,
        plan: &TransferPlan,
        cancel: &AtomicBool,
        mut on_progress: F,
    ) -> Result<TransferReport, TransferError>
    where
        F: FnMut(ProgressUpdate),
    {
        let url = self.check_url(&plan.url)?;
        let partial = partial_path(&plan.output_path, &plan.job_id)?;

        if let Some(parent) = plan.output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| io_error(parent, &error))?;
            }
        }

        let resume_at = resumable_offset(&partial, plan.resume_from)?;
        let appended = resume_at > 0;

        let file = open_partial(&partial, resume_at)?;
        let outcome = self.stream(&url, plan, file, partial.clone(), resume_at, cancel, &mut on_progress);

        match outcome {
            Ok(report) => Ok(report),
            Err(error) => {
                let keep_partial = appended && !matches!(error, TransferError::Cancelled);
                if !keep_partial {
                    remove_if_present(&partial);
                }
                Err(error)
            }
        }
    }

    fn check_url(&self, raw: &str) -> Result<Url, TransferError> {
        let url = Url::parse(raw).map_err(|_| TransferError::InvalidUrl {
            url: raw.to_string(),
        })?;

        if !matches!(url.scheme(), "http" | "https") {
            return Err(TransferError::InvalidUrl {
                url: raw.to_string(),
            });
        }

        if let Some(allowlist) = &self.allowlist {
            return allowlist.check_url(url.as_str()).map_err(|error| match error {
                crate::runtime::permissions::PermissionError::NetworkDenied { host } => {
                    TransferError::Denied { host }
                }
                other => TransferError::InvalidUrl {
                    url: other.to_string(),
                },
            });
        }

        Ok(url)
    }

    #[allow(clippy::too_many_arguments)]
    fn stream<F>(
        &self,
        url: &Url,
        plan: &TransferPlan,
        mut file: File,
        partial: PathBuf,
        mut written: u64,
        cancel: &AtomicBool,
        on_progress: &mut F,
    ) -> Result<TransferReport, TransferError>
    where
        F: FnMut(ProgressUpdate),
    {
        let mut builder = Request::builder()
            .method(Method::GET)
            .uri(url.as_str());

        for (name, value) in &plan.headers {
            let header = HeaderValue::from_str(value).map_err(|_| {
                TransferError::Transport(format!("header {name:?} is not a valid HTTP header"))
            })?;
            builder = builder.header(name.as_str(), header);
        }

        if written > 0 {
            builder = builder.header("Range", format!("bytes={written}-"));
        }

        let request = builder
            .body(Vec::new())
            .map_err(|error| TransferError::Transport(error.to_string()))?;

        let mut response = self
            .agent
            .run(request)
            .map_err(|error| map_transport_error(error, self.limits.read_timeout))?;

        let status = response.status().as_u16();
        let mut resumed_from = 0;

        match status {
            200 => {
                if written > 0 {
                    // The server ignored the range request: start over rather
                    // than append a second copy of the same bytes.
                    file.set_len(0).map_err(|error| io_error(&partial, &error))?;
                    file.seek(std::io::SeekFrom::Start(0))
                        .map_err(|error| io_error(&partial, &error))?;
                    written = 0;
                }
            }
            206 => {
                let start = content_range_start(response.headers().get("content-range"))?;
                if start != written {
                    return Err(TransferError::RangeMismatch {
                        start,
                        expected: written,
                    });
                }
                resumed_from = written;
            }
            other => {
                return Err(TransferError::Http {
                    status: other,
                    url: url.to_string(),
                })
            }
        }

        let content_length = content_length(response.headers().get("content-length"));
        let total = content_length
            .map(|length| length.saturating_add(written))
            .or(plan.expected_total);

        if let Some(total) = total {
            if total > self.limits.max_bytes {
                return Err(TransferError::TooLarge {
                    limit: self.limits.max_bytes,
                });
            }
        }

        let mut reader = response
            .body_mut()
            .with_config()
            .limit(self.limits.max_bytes)
            .reader();

        let mut throttle = ProgressThrottle::with_interval(self.limits.progress_interval);
        let mut buffer = vec![0u8; self.limits.chunk_bytes.max(1024)];
        let mut last_emitted: Option<u64> = None;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(TransferError::Cancelled);
            }

            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    return Err(truncation_or(error, written, total, self.limits.read_timeout))
                }
            };

            file.write_all(&buffer[..read])
                .map_err(|error| io_error(&partial, &error))?;
            written += read as u64;

            if written > self.limits.max_bytes {
                return Err(TransferError::TooLarge {
                    limit: self.limits.max_bytes,
                });
            }

            if let Some(update) = throttle.record(written, total, Instant::now()) {
                last_emitted = Some(update.written);
                on_progress(update);
            }
        }

        if let Some(total) = total {
            if written < total {
                return Err(TransferError::ShortBody {
                    expected: total,
                    received: written,
                });
            }
        }

        file.flush().map_err(|error| io_error(&partial, &error))?;
        file.sync_all().map_err(|error| io_error(&partial, &error))?;
        drop(file);

        if last_emitted != Some(written) {
            on_progress(ProgressUpdate {
                written,
                total,
                percent: percent_of(written, total),
            });
        }

        fs::rename(&partial, &plan.output_path).map_err(|error| {
            // The temp file is gone from the caller's point of view only when
            // the rename succeeded; report both paths so the failure is usable.
            io_error(&plan.output_path, &error)
        })?;

        Ok(TransferReport {
            bytes_written: written,
            bytes_total: total.or(Some(written)),
            resumed_from,
        })
    }
}

/// Offset this transfer may append from. A temp file shorter than the requested
/// offset cannot be trusted, so the transfer restarts from zero.
fn resumable_offset(partial: &Path, requested: u64) -> Result<u64, TransferError> {
    if requested == 0 {
        return Ok(0);
    }

    match fs::metadata(partial) {
        Ok(metadata) if metadata.len() >= requested => Ok(requested),
        Ok(_) => Ok(0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(io_error(partial, &error)),
    }
}

fn open_partial(partial: &Path, resume_at: u64) -> Result<File, TransferError> {
    if resume_at == 0 {
        return File::create(partial).map_err(|error| io_error(partial, &error));
    }

    let file = OpenOptions::new()
        .write(true)
        .open(partial)
        .map_err(|error| io_error(partial, &error))?;

    // A longer temp file than claimed would leave stale bytes in the middle.
    file.set_len(resume_at)
        .map_err(|error| io_error(partial, &error))?;

    let mut file = file;
    file.seek(std::io::SeekFrom::End(0))
        .map_err(|error| io_error(partial, &error))?;
    Ok(file)
}

fn content_length(header: Option<&HeaderValue>) -> Option<u64> {
    header?.to_str().ok()?.trim().parse().ok()
}

/// `Content-Range: bytes 100-199/200` -> `100`.
fn content_range_start(header: Option<&HeaderValue>) -> Result<u64, TransferError> {
    let text = header
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| TransferError::Transport("206 response has no Content-Range".to_string()))?;

    text.split_whitespace()
        .nth(1)
        .and_then(|range| range.split('-').next())
        .and_then(|start| start.parse().ok())
        .ok_or_else(|| TransferError::Transport(format!("unusable Content-Range {text:?}")))
}

fn percent_of(written: u64, total: Option<u64>) -> Option<u8> {
    let total = total?;
    if total == 0 {
        return None;
    }
    Some((written.saturating_mul(100) / total).min(100) as u8)
}

fn io_error(path: &Path, error: &std::io::Error) -> TransferError {
    TransferError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

fn remove_if_present(path: &Path) {
    let _ = fs::remove_file(path);
}

fn map_transport_error(error: ureq::Error, timeout: Duration) -> TransferError {
    match error {
        ureq::Error::Timeout(_) => TransferError::Timeout {
            seconds: timeout.as_secs(),
        },
        ureq::Error::Io(io) => read_error(io, timeout),
        ureq::Error::BodyExceedsLimit(limit) => TransferError::TooLarge { limit },
        other => TransferError::Transport(other.to_string()),
    }
}

/// A body that ends early is reported as a short body; a stalled socket is
/// still a timeout. Distinguishing them matters because only one of the two is
/// worth retrying immediately.
fn truncation_or(
    error: std::io::Error,
    written: u64,
    total: Option<u64>,
    timeout: Duration,
) -> TransferError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => TransferError::Timeout {
            seconds: timeout.as_secs(),
        },
        _ => match total {
            Some(total) if written < total => TransferError::ShortBody {
                expected: total,
                received: written,
            },
            _ => TransferError::Transport(error.to_string()),
        },
    }
}

fn read_error(error: std::io::Error, timeout: Duration) -> TransferError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => TransferError::Timeout {
            seconds: timeout.as_secs(),
        },
        _ => TransferError::Transport(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    const TEN_MB: usize = 10 * 1024 * 1024;

    fn payload(len: usize) -> Arc<Vec<u8>> {
        Arc::new((0..len).map(|index| (index % 251) as u8).collect())
    }

    /// A loopback server that honours `Range` and can be told to cut the body
    /// short. Written by hand because the fixture server in `runtime::testing`
    /// always writes a complete, non-ranged response.
    struct Server {
        base: String,
        ranges: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl Server {
        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        fn ranges(&self) -> Vec<Option<String>> {
            self.ranges.lock().expect("ranges").clone()
        }
    }

    fn read_head(stream: &mut TcpStream) -> Vec<(String, String)> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).expect("read head");
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }

        String::from_utf8_lossy(&buffer)
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect()
    }

    fn serve(body: Arc<Vec<u8>>, honoured_range: bool, cut_at: Option<usize>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&ranges);

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let body = Arc::clone(&body);
                let observed = Arc::clone(&observed);
                thread::spawn(move || {
                    let request = read_head(&mut stream);
                    let range = request
                        .iter()
                        .find(|(name, _)| name == "range")
                        .map(|(_, value)| value.clone());
                    observed.lock().expect("record").push(range.clone());

                    let start = range
                        .as_deref()
                        .filter(|_| honoured_range)
                        .and_then(|text| text.strip_prefix("bytes="))
                        .and_then(|rest| rest.strip_suffix('-'))
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);

                    let slice = &body[start.min(body.len())..];
                    let mut head = String::new();
                    if start > 0 {
                        head.push_str(&format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\n",
                            start,
                            body.len() - 1,
                            body.len()
                        ));
                    } else {
                        head.push_str("HTTP/1.1 200 OK\r\n");
                    }
                    head.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    ));
                    let _ = stream.write_all(head.as_bytes());

                    match cut_at {
                        Some(limit) => {
                            let _ = stream.write_all(&slice[..slice.len().min(limit)]);
                            let _ = stream.flush();
                            let _ = stream.shutdown(Shutdown::Write);
                        }
                        None => {
                            let _ = stream.write_all(slice);
                            let _ = stream.flush();
                        }
                    }
                });
            }
        });

        Server { base, ranges }
    }

    fn status_server(status: u16) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                thread::spawn(move || {
                    let _ = read_head(&mut stream);
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 {status} Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                    let _ = stream.flush();
                });
            }
        });

        Server {
            base,
            ranges: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn downloader() -> Downloader {
        Downloader::new(TransferLimits::default())
    }

    #[test]
    fn a_ten_megabyte_payload_completes_with_monotonic_progress() {
        let body = payload(TEN_MB);
        let server = serve(Arc::clone(&body), true, None);
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");

        let plan = TransferPlan::new(server.url("/audio"), &output, "job-1");
        let mut updates = Vec::new();
        let report = downloader()
            .transfer(&plan, &AtomicBool::new(false), |update| updates.push(update))
            .expect("transfer");

        assert_eq!(report.bytes_written, TEN_MB as u64);
        assert_eq!(report.bytes_total, Some(TEN_MB as u64));
        assert_eq!(report.resumed_from, 0);
        assert_eq!(fs::read(&output).expect("read output"), *body);
        assert!(
            !partial_path(&output, "job-1").expect("partial").exists(),
            "the temp file must be renamed away"
        );

        assert!(updates.len() >= 2, "expected several progress updates");
        assert!(updates
            .windows(2)
            .all(|pair| pair[1].written >= pair[0].written));
        let last = updates.last().expect("final update");
        assert_eq!(last.written, TEN_MB as u64);
        assert_eq!(last.percent, Some(100));
    }

    #[test]
    fn cancelling_halfway_leaves_nothing_on_disk() {
        let body = payload(TEN_MB);
        let server = serve(Arc::clone(&body), true, None);
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");

        let limits = TransferLimits {
            // Every chunk reports, so the halfway point is always observed.
            progress_interval: Duration::ZERO,
            ..TransferLimits::default()
        };
        let cancel = AtomicBool::new(false);
        let plan = TransferPlan::new(server.url("/audio"), &output, "job-2");

        let error = Downloader::new(limits)
            .transfer(&plan, &cancel, |update| {
                if update.written >= (TEN_MB / 2) as u64 {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .expect_err("cancelled");

        assert_eq!(error, TransferError::Cancelled);
        assert_eq!(error.failure_kind(), DownloadFailureKind::Cancelled);
        assert!(!output.exists(), "no finished file");
        assert!(
            !partial_path(&output, "job-2").expect("partial").exists(),
            "no partial file"
        );
    }

    #[test]
    fn a_server_dropping_mid_stream_fails_and_removes_the_partial() {
        let body = payload(1024 * 1024);
        let server = serve(Arc::clone(&body), true, Some(256 * 1024));
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");

        let plan = TransferPlan::new(server.url("/audio"), &output, "job-3");
        let error = downloader()
            .transfer(&plan, &AtomicBool::new(false), |_| {})
            .expect_err("truncated");

        let TransferError::ShortBody { expected, received } = error else {
            panic!("unexpected error: {error:?}");
        };
        assert_eq!(expected, 1_048_576);
        assert_eq!(received, 262_144);
        assert_eq!(error.failure_kind(), DownloadFailureKind::DownloadError);
        assert!(!output.exists());
        assert!(!partial_path(&output, "job-3").expect("partial").exists());
    }

    #[test]
    fn resuming_from_an_offset_produces_a_byte_identical_file() {
        let body = payload(TEN_MB);
        let server = serve(Arc::clone(&body), true, None);
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        let partial = partial_path(&output, "job-4").expect("partial");

        let half = TEN_MB / 2;
        fs::write(&partial, &body[..half]).expect("seed partial");

        let plan = TransferPlan::new(server.url("/audio"), &output, "job-4").resuming_from(half as u64);
        let report = downloader()
            .transfer(&plan, &AtomicBool::new(false), |_| {})
            .expect("resumed transfer");

        assert_eq!(report.resumed_from, half as u64);
        assert_eq!(report.bytes_written, TEN_MB as u64);
        assert_eq!(fs::read(&output).expect("read output"), *body);
        assert_eq!(
            server.ranges(),
            vec![Some(format!("bytes={half}-"))],
            "the resumed request must ask only for the missing bytes"
        );
    }

    #[test]
    fn a_server_that_ignores_the_range_restarts_from_zero() {
        let body = payload(512 * 1024);
        let server = serve(Arc::clone(&body), false, None);
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");
        let partial = partial_path(&output, "job-5").expect("partial");

        let half = body.len() / 2;
        fs::write(&partial, vec![0xAAu8; half]).expect("seed partial");

        let plan = TransferPlan::new(server.url("/audio"), &output, "job-5")
            .resuming_from(half as u64);
        let report = downloader()
            .transfer(&plan, &AtomicBool::new(false), |_| {})
            .expect("transfer");

        assert_eq!(report.resumed_from, 0, "a 200 response means a fresh start");
        assert_eq!(fs::read(&output).expect("read output"), *body);
    }

    #[test]
    fn concurrent_jobs_never_write_the_same_temp_path() {
        let body = payload(2 * 1024 * 1024);
        let server = serve(Arc::clone(&body), true, None);
        let dir = tempfile::tempdir().expect("tempdir");
        let first_output = dir.path().join("a.flac");
        let second_output = dir.path().join("b.flac");

        let first = partial_path(&first_output, "job-a").expect("partial");
        let second = partial_path(&second_output, "job-b").expect("partial");
        assert_ne!(first, second);

        let first_base = server.base.clone();
        let second_base = server.base.clone();
        let first_path = first_output.clone();
        let second_path = second_output.clone();

        let first_thread = thread::spawn(move || {
            downloader()
                .transfer(
                    &TransferPlan::new(format!("{first_base}/a"), &first_path, "job-a"),
                    &AtomicBool::new(false),
                    |_| {},
                )
                .expect("first transfer")
        });
        let second_thread = thread::spawn(move || {
            downloader()
                .transfer(
                    &TransferPlan::new(format!("{second_base}/b"), &second_path, "job-b"),
                    &AtomicBool::new(false),
                    |_| {},
                )
                .expect("second transfer")
        });

        first_thread.join().expect("first join");
        second_thread.join().expect("second join");

        assert_eq!(fs::read(&first_output).expect("a"), *body);
        assert_eq!(fs::read(&second_output).expect("b"), *body);
        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[test]
    fn a_missing_target_is_a_typed_http_error() {
        let server = status_server(404);
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");

        let error = downloader()
            .transfer(
                &TransferPlan::new(server.url("/gone"), &output, "job-6"),
                &AtomicBool::new(false),
                |_| {},
            )
            .expect_err("404");

        assert_eq!(
            error,
            TransferError::Http {
                status: 404,
                url: server.url("/gone")
            }
        );
        assert_eq!(error.failure_kind(), DownloadFailureKind::NotFound);
        assert!(!output.exists());
    }

    #[test]
    fn job_ids_that_could_escape_the_output_directory_are_refused() {
        let output = Path::new("dir/track.flac");

        assert_eq!(
            partial_path(output, "..").unwrap_err(),
            TransferError::InvalidJobId {
                job_id: "..".to_string()
            }
        );
        assert!(partial_path(output, "a/b").is_err());
        assert!(partial_path(output, "").is_err());
        assert!(partial_path(output, &"x".repeat(MAX_JOB_ID_LENGTH + 1)).is_err());
        assert!(partial_path(output, "job-1_ok").is_ok());
    }

    #[test]
    fn the_temp_path_is_derived_from_the_job_id() {
        let partial = partial_path(Path::new("dir/track.flac"), "job-7").expect("partial");
        assert_eq!(partial, Path::new("dir/track.flac.job-7.part"));
    }

    #[test]
    fn unusable_urls_are_typed_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("track.flac");

        for url in ["file:///etc/passwd", "not a url", "ftp://example.test/a"] {
            let error = downloader()
                .transfer(
                    &TransferPlan::new(url, &output, "job-8"),
                    &AtomicBool::new(false),
                    |_| {},
                )
                .expect_err("invalid url");
            assert!(
                matches!(error, TransferError::InvalidUrl { .. }),
                "{url}: {error:?}"
            );
            assert_eq!(error.failure_kind(), DownloadFailureKind::InvalidInput);
        }
    }
}
