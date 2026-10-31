//! Download queue state machine.
//!
//! The queue owns job state. It does not transfer bytes; the executor in
//! `download.rs` drives a job through [`JobState`] and reports progress here.
//! Splitting the two keeps every lifecycle rule testable without a network.
//!
//! Rules this module enforces:
//!
//! - **One legal transition table.** A job cannot jump from `queued` to `done`,
//!   and a terminal job cannot be edited.
//! - **Cancel works in every non-terminal state** and deletes the recorded
//!   partial file, so cancelling never leaves a temp file behind.
//! - **Album cancellation cancels remaining children and keeps completed ones.**
//!   A finished track is not undone by cancelling its album.
//! - **A failed job records a typed failure and releases its slot**, so the
//!   next queued job starts instead of the queue stalling on one bad track.
//! - **Progress is throttled to 10 Hz**, with the final update always emitted.

use crate::adapter::DownloadFailureKind;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Concurrent active jobs when the caller does not choose a number.
pub const DEFAULT_CONCURRENCY: usize = 2;

/// Minimum gap between progress updates the executor may forward.
pub const PROGRESS_INTERVAL_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Resolving,
    Downloading,
    PostProcessing,
    Embedding,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub const ALL: [JobState; 8] = [
        JobState::Queued,
        JobState::Resolving,
        JobState::Downloading,
        JobState::PostProcessing,
        JobState::Embedding,
        JobState::Done,
        JobState::Failed,
        JobState::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Resolving => "resolving",
            Self::Downloading => "downloading",
            Self::PostProcessing => "post_processing",
            Self::Embedding => "embedding",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// States that occupy a concurrency slot.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Resolving | Self::Downloading | Self::PostProcessing | Self::Embedding
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use JobState::*;
        match (self, next) {
            (Queued, Resolving | Failed | Cancelled) => true,
            (Resolving, Downloading | Failed | Cancelled) => true,
            (Downloading, PostProcessing | Failed | Cancelled) => true,
            (PostProcessing, Embedding | Done | Failed | Cancelled) => true,
            (Embedding, Done | Failed | Cancelled) => true,
            // Retry: a failed or cancelled job may be queued again.
            (Failed | Cancelled, Queued) => true,
            _ => false,
        }
    }
}

/// A typed failure with the stage it happened in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobFailure {
    pub stage: JobState,
    pub kind: DownloadFailureKind,
    pub message: String,
}

/// One downloadable track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub track_id: String,
    pub provider: String,
    pub quality: String,
    pub state: JobState,
    pub output_path: PathBuf,
    /// `.part` file while downloading; cleared once the job finishes or is
    /// cancelled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_path: Option<PathBuf>,
    pub bytes_written: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<JobFailure>,
}

impl Job {
    pub fn percent(&self) -> Option<u8> {
        let total = self.bytes_total?;
        if total == 0 {
            return None;
        }
        let percent = self.bytes_written.saturating_mul(100) / total;
        Some(percent.min(100) as u8)
    }

    pub fn is_cancelled(&self) -> bool {
        self.state == JobState::Cancelled
    }
}

/// What the caller supplies to enqueue a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    pub track_id: String,
    pub provider: String,
    pub quality: String,
    pub output_path: PathBuf,
}

impl JobSpec {
    pub fn new(
        track_id: impl Into<String>,
        provider: impl Into<String>,
        quality: impl Into<String>,
        output_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            track_id: track_id.into(),
            provider: provider.into(),
            quality: quality.into(),
            output_path: output_path.into(),
        }
    }
}

/// A group of track jobs created from one album request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumBatch {
    pub id: String,
    pub album_id: String,
    pub children: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueueError {
    #[error("job `{id}` is not queued")]
    UnknownJob { id: String },

    #[error("album `{id}` is not queued")]
    UnknownAlbum { id: String },

    #[error("job `{id}` cannot move from `{from}` to `{to}`")]
    InvalidTransition {
        id: String,
        from: &'static str,
        to: &'static str,
    },

    #[error("cannot remove partial file `{path}`: {message}")]
    RemovePartial { path: String, message: String },
}

/// Download queue. Jobs are kept in insertion order; selection for dispatch is
/// separate so the UI can list them by state.
#[derive(Debug, Clone)]
pub struct DownloadQueue {
    jobs: Vec<Job>,
    albums: Vec<AlbumBatch>,
    max_concurrent: usize,
    next_job: u64,
    next_album: u64,
}

impl Default for DownloadQueue {
    fn default() -> Self {
        Self::new(DEFAULT_CONCURRENCY)
    }
}

impl DownloadQueue {
    /// A queue that runs at most `max_concurrent` jobs at once. Zero is treated
    /// as one: a queue that can never start a job is a misconfiguration, not a
    /// mode.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            jobs: Vec::new(),
            albums: Vec::new(),
            max_concurrent: max_concurrent.max(1),
            next_job: 1,
            next_album: 1,
        }
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn albums(&self) -> &[AlbumBatch] {
        &self.albums
    }

    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|job| job.id == id)
    }

    pub fn album(&self, id: &str) -> Option<&AlbumBatch> {
        self.albums.iter().find(|album| album.id == id)
    }

    /// Number of jobs holding a concurrency slot.
    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|job| job.state.is_active()).count()
    }

    pub fn has_capacity(&self) -> bool {
        self.active_count() < self.max_concurrent
    }

    /// The next job to start: oldest queued job while a slot is free.
    pub fn next_queued(&self) -> Option<&Job> {
        if !self.has_capacity() {
            return None;
        }
        self.jobs
            .iter()
            .find(|job| job.state == JobState::Queued)
    }

    pub fn enqueue(&mut self, spec: JobSpec) -> String {
        let id = format!("job-{}", self.next_job);
        self.next_job += 1;

        self.jobs.push(Job {
            id: id.clone(),
            track_id: spec.track_id,
            provider: spec.provider,
            quality: spec.quality,
            state: JobState::Queued,
            output_path: spec.output_path,
            partial_path: None,
            bytes_written: 0,
            bytes_total: None,
            failure: None,
        });

        id
    }

    /// Enqueue every track of an album as one cancellable batch.
    pub fn enqueue_album(
        &mut self,
        album_id: impl Into<String>,
        specs: Vec<JobSpec>,
    ) -> AlbumBatch {
        let id = format!("album-{}", self.next_album);
        self.next_album += 1;

        let children = specs
            .into_iter()
            .map(|spec| self.enqueue(spec))
            .collect();

        let batch = AlbumBatch {
            id: id.clone(),
            album_id: album_id.into(),
            children,
        };
        self.albums.push(batch.clone());
        batch
    }

    pub fn transition(&mut self, id: &str, next: JobState) -> Result<(), QueueError> {
        let job = self.job_mut(id)?;
        if !job.state.can_transition_to(next) {
            return Err(QueueError::InvalidTransition {
                id: id.to_string(),
                from: job.state.as_str(),
                to: next.as_str(),
            });
        }

        job.state = next;
        if next == JobState::Queued {
            job.bytes_written = 0;
            job.failure = None;
        }
        if next.is_terminal() {
            job.partial_path = None;
        }
        Ok(())
    }

    /// Enter `downloading` and record the temp file the executor writes.
    pub fn begin_download(
        &mut self,
        id: &str,
        partial_path: impl Into<PathBuf>,
        total: Option<u64>,
    ) -> Result<(), QueueError> {
        self.transition(id, JobState::Downloading)?;
        let job = self.job_mut(id)?;
        job.partial_path = Some(partial_path.into());
        job.bytes_total = total;
        job.bytes_written = 0;
        Ok(())
    }

    pub fn record_progress(
        &mut self,
        id: &str,
        written: u64,
        total: Option<u64>,
    ) -> Result<(), QueueError> {
        let job = self.job_mut(id)?;
        job.bytes_written = written;
        if total.is_some() {
            job.bytes_total = total;
        }
        Ok(())
    }

    pub fn fail(
        &mut self,
        id: &str,
        kind: DownloadFailureKind,
        message: impl Into<String>,
    ) -> Result<(), QueueError> {
        let stage = self.job(id).ok_or_else(|| QueueError::UnknownJob {
            id: id.to_string(),
        })?.state;

        self.transition(id, JobState::Failed)?;
        let job = self.job_mut(id)?;
        job.failure = Some(JobFailure {
            stage,
            kind,
            message: message.into(),
        });
        Ok(())
    }

    /// Complete a job, keeping the final byte counters for the UI.
    pub fn complete(&mut self, id: &str) -> Result<(), QueueError> {
        self.transition(id, JobState::Done)
    }

    /// Point a job at a different output file.
    ///
    /// A post-processing hook may convert the download to another container, in
    /// which case every later stage (embedding, the library import, the retry
    /// path) has to use the file that actually exists.
    pub fn retarget(
        &mut self,
        id: &str,
        output_path: impl Into<PathBuf>,
    ) -> Result<(), QueueError> {
        let job = self.job_mut(id)?;
        job.output_path = output_path.into();
        Ok(())
    }

    /// Cancel one job in any non-terminal state and delete its partial file.
    ///
    /// Returns `false` when the job was already terminal: cancelling a finished
    /// job is not an error, it is simply nothing to do.
    pub fn cancel(&mut self, id: &str) -> Result<bool, QueueError> {
        let job = self.job_mut(id)?;
        if job.state.is_terminal() {
            return Ok(false);
        }

        let partial = job.partial_path.take();
        job.state = JobState::Cancelled;
        job.failure = None;

        if let Some(path) = partial {
            remove_file_if_present(&path)?;
        }

        Ok(true)
    }

    /// Cancel an album: every unfinished child is cancelled and its partial file
    /// removed. Completed children stay done.
    pub fn cancel_album(&mut self, album_id: &str) -> Result<usize, QueueError> {
        let children = self
            .album(album_id)
            .ok_or_else(|| QueueError::UnknownAlbum {
                id: album_id.to_string(),
            })?
            .children
            .clone();

        let mut cancelled = 0;
        for child in children {
            if self.cancel(&child)? {
                cancelled += 1;
            }
        }

        Ok(cancelled)
    }

    fn job_mut(&mut self, id: &str) -> Result<&mut Job, QueueError> {
        self.jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| QueueError::UnknownJob {
                id: id.to_string(),
            })
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), QueueError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(QueueError::RemovePartial {
            path: path.display().to_string(),
            message: error.to_string(),
        }),
    }
}

/// One progress update worth forwarding to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressUpdate {
    pub written: u64,
    pub total: Option<u64>,
    pub percent: Option<u8>,
}

/// Rate limiter for progress events.
///
/// The executor calls [`ProgressThrottle::record`] for every chunk; only the
/// updates it returns should be emitted. The final update (written == total) is
/// always returned exactly once, however late it arrives.
#[derive(Debug, Clone)]
pub struct ProgressThrottle {
    interval: Duration,
    last_emit: Option<Instant>,
    finished: bool,
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressThrottle {
    pub fn new() -> Self {
        Self::with_interval(Duration::from_millis(PROGRESS_INTERVAL_MS))
    }

    pub fn with_interval(interval: Duration) -> Self {
        Self {
            interval,
            last_emit: None,
            finished: false,
        }
    }

    pub fn record(
        &mut self,
        written: u64,
        total: Option<u64>,
        now: Instant,
    ) -> Option<ProgressUpdate> {
        let complete = matches!(total, Some(total) if total > 0 && written >= total);
        if complete && self.finished {
            return None;
        }

        let due = match self.last_emit {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval,
        };

        if !complete && !due {
            return None;
        }

        self.last_emit = Some(now);
        self.finished = complete;

        Some(ProgressUpdate {
            written,
            total,
            percent: percent_of(written, total),
        })
    }
}

fn percent_of(written: u64, total: Option<u64>) -> Option<u8> {
    let total = total?;
    if total == 0 {
        return None;
    }
    Some((written.saturating_mul(100) / total).min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(track: &str) -> JobSpec {
        JobSpec::new(track, "provider", "best", format!("out/{track}.flac"))
    }

    fn finish(queue: &mut DownloadQueue, id: &str) {
        for state in [
            JobState::Resolving,
            JobState::Downloading,
            JobState::PostProcessing,
            JobState::Embedding,
            JobState::Done,
        ] {
            if queue.job(id).expect("job").state == state {
                continue;
            }
            queue.transition(id, state).expect("legal step");
        }
    }

    /// Put a job directly into `state` so the transition table can be tested
    /// from every starting point.
    fn queue_with(state: JobState) -> (DownloadQueue, String) {
        let mut queue = DownloadQueue::default();
        let id = queue.enqueue(spec("t1"));
        queue.jobs[0].state = state;
        (queue, id)
    }

    #[test]
    fn the_transition_table_allows_exactly_the_documented_moves() {
        use JobState::*;

        let allowed = [
            (Queued, Resolving),
            (Queued, Failed),
            (Queued, Cancelled),
            (Resolving, Downloading),
            (Resolving, Failed),
            (Resolving, Cancelled),
            (Downloading, PostProcessing),
            (Downloading, Failed),
            (Downloading, Cancelled),
            (PostProcessing, Embedding),
            (PostProcessing, Done),
            (PostProcessing, Failed),
            (PostProcessing, Cancelled),
            (Embedding, Done),
            (Embedding, Failed),
            (Embedding, Cancelled),
            (Failed, Queued),
            (Cancelled, Queued),
        ];

        for from in JobState::ALL {
            for to in JobState::ALL {
                let expected = allowed.contains(&(from, to));
                assert_eq!(
                    from.can_transition_to(to),
                    expected,
                    "{} -> {} should be {}",
                    from.as_str(),
                    to.as_str(),
                    if expected { "allowed" } else { "refused" }
                );

                let (mut queue, id) = queue_with(from);
                let result = queue.transition(&id, to);
                assert_eq!(result.is_ok(), expected, "queue disagreement for {from:?} -> {to:?}");
                if expected {
                    assert_eq!(queue.job(&id).expect("job").state, to);
                } else {
                    assert_eq!(queue.job(&id).expect("job").state, from);
                }
            }
        }
    }

    #[test]
    fn every_stage_reaches_done_in_order() {
        let mut queue = DownloadQueue::default();
        let id = queue.enqueue(spec("t1"));

        for state in [
            JobState::Resolving,
            JobState::Downloading,
            JobState::PostProcessing,
            JobState::Embedding,
            JobState::Done,
        ] {
            queue.transition(&id, state).expect("legal step");
            assert_eq!(queue.job(&id).expect("job").state, state);
        }
        assert!(queue.job(&id).expect("job").state.is_terminal());
    }

    #[test]
    fn cancelling_mid_download_removes_the_partial_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let partial = dir.path().join("track.flac.part");
        fs::write(&partial, b"half a file").expect("write partial");

        let mut queue = DownloadQueue::default();
        let id = queue.enqueue(spec("t1"));
        queue.transition(&id, JobState::Resolving).expect("resolving");
        queue
            .begin_download(&id, &partial, Some(20))
            .expect("downloading");
        queue.record_progress(&id, 10, None).expect("progress");

        assert_eq!(queue.job(&id).expect("job").percent(), Some(50));
        assert!(queue.cancel(&id).expect("cancel"));

        assert_eq!(queue.job(&id).expect("job").state, JobState::Cancelled);
        assert!(!partial.exists(), "the partial file must be gone");
        assert_eq!(queue.job(&id).expect("job").partial_path, None);
        assert!(!queue.cancel(&id).expect("second cancel"), "already terminal");
    }

    #[test]
    fn cancelling_an_album_keeps_completed_tracks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut queue = DownloadQueue::default();
        let batch = queue.enqueue_album(
            "album-1",
            vec![spec("a"), spec("b"), spec("c")],
        );

        let first = batch.children[0].clone();
        finish(&mut queue, &first);

        let second = batch.children[1].clone();
        queue.transition(&second, JobState::Resolving).expect("resolving");
        let partial = dir.path().join("b.part");
        fs::write(&partial, b"x").expect("write");
        queue
            .begin_download(&second, &partial, Some(100))
            .expect("downloading");

        let cancelled = queue.cancel_album(&batch.id).expect("cancels");
        assert_eq!(cancelled, 2);

        assert_eq!(queue.job(&first).expect("a").state, JobState::Done);
        assert_eq!(queue.job(&second).expect("b").state, JobState::Cancelled);
        assert_eq!(
            queue.job(&batch.children[2]).expect("c").state,
            JobState::Cancelled
        );
        assert!(!partial.exists());
        assert_eq!(queue.active_count(), 0);
    }

    #[test]
    fn cancel_honours_every_non_terminal_state() {
        for state in [
            JobState::Queued,
            JobState::Resolving,
            JobState::Downloading,
            JobState::PostProcessing,
            JobState::Embedding,
        ] {
            let (mut queue, id) = queue_with(state);
            assert!(queue.cancel(&id).expect("cancel"), "{state:?}");
            assert_eq!(queue.job(&id).expect("job").state, JobState::Cancelled);
        }
    }

    #[test]
    fn a_failed_job_records_its_stage_and_releases_its_slot() {
        let mut queue = DownloadQueue::new(1);
        let first = queue.enqueue(spec("a"));
        let second = queue.enqueue(spec("b"));

        queue.transition(&first, JobState::Resolving).expect("resolving");
        queue
            .begin_download(&first, "out/a.part", None)
            .expect("downloading");
        queue
            .fail(&first, DownloadFailureKind::Timeout, "connection died")
            .expect("fails");

        let job = queue.job(&first).expect("job");
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(
            job.failure,
            Some(JobFailure {
                stage: JobState::Downloading,
                kind: DownloadFailureKind::Timeout,
                message: "connection died".to_string(),
            })
        );
        assert_eq!(queue.active_count(), 0);
        assert_eq!(
            queue.next_queued().map(|job| job.id.as_str()),
            Some(second.as_str()),
            "the failure must not block the next job"
        );
    }

    #[test]
    fn concurrency_defaults_to_two_and_is_configurable() {
        let mut queue = DownloadQueue::default();
        assert_eq!(queue.max_concurrent(), DEFAULT_CONCURRENCY);

        let ids: Vec<String> = (0..3).map(|index| queue.enqueue(spec(&format!("t{index}")))).collect();

        for id in &ids[..2] {
            queue.transition(id, JobState::Resolving).expect("resolving");
        }
        assert_eq!(queue.active_count(), 2);
        assert!(!queue.has_capacity());
        assert_eq!(queue.next_queued(), None);

        finish(&mut queue, &ids[0]);
        assert!(queue.has_capacity());
        assert_eq!(
            queue.next_queued().map(|job| job.id.as_str()),
            Some(ids[2].as_str())
        );

        let single = DownloadQueue::new(1);
        assert_eq!(single.max_concurrent(), 1);
        assert_eq!(DownloadQueue::new(0).max_concurrent(), 1);
    }

    #[test]
    fn retrying_a_failed_job_resets_counters_and_failure() {
        let mut queue = DownloadQueue::default();
        let id = queue.enqueue(spec("t1"));
        queue.record_progress(&id, 10, Some(100)).expect("progress");
        queue
            .fail(&id, DownloadFailureKind::ApiError, "nope")
            .expect("fails");
        assert!(queue.job(&id).expect("job").failure.is_some());

        queue.transition(&id, JobState::Queued).expect("retry");
        let job = queue.job(&id).expect("job");
        assert_eq!(job.bytes_written, 0);
        assert_eq!(job.failure, None);
    }

    #[test]
    fn unknown_ids_are_typed_errors() {
        let mut queue = DownloadQueue::default();

        assert_eq!(
            queue.transition("ghost", JobState::Resolving).unwrap_err(),
            QueueError::UnknownJob {
                id: "ghost".to_string()
            }
        );
        assert_eq!(
            queue.cancel("ghost").unwrap_err(),
            QueueError::UnknownJob {
                id: "ghost".to_string()
            }
        );
        assert_eq!(
            queue.cancel_album("ghost").unwrap_err(),
            QueueError::UnknownAlbum {
                id: "ghost".to_string()
            }
        );
    }

    #[test]
    fn progress_is_throttled_to_ten_hertz_and_finishes_exactly_once() {
        let start = Instant::now();
        let mut throttle = ProgressThrottle::new();
        let mut emissions = Vec::new();

        for millisecond in 0..=1000u64 {
            let now = start + Duration::from_millis(millisecond);
            if let Some(update) = throttle.record(millisecond, Some(1000), now) {
                emissions.push(update);
            }
        }

        // First update at 0 ms, then one per 100 ms, plus the final at 1000 ms.
        assert!(
            (10..=12).contains(&emissions.len()),
            "expected about 11 updates, got {}",
            emissions.len()
        );
        assert!(
            emissions.windows(2).all(|pair| pair[1].written >= pair[0].written),
            "progress must be monotonic"
        );
        assert_eq!(emissions.last().map(|update| update.percent), Some(Some(100)));

        let final_again = throttle.record(1000, Some(1000), start + Duration::from_millis(2000));
        assert_eq!(final_again, None, "the final update is emitted once");
    }

    #[test]
    fn a_job_serialises_with_its_state_and_failure_kind_codes() {
        let mut queue = DownloadQueue::default();
        let id = queue.enqueue(spec("t1"));
        queue
            .fail(&id, DownloadFailureKind::RateLimited, "slow down")
            .expect("fails");

        let json = serde_json::to_value(queue.job(&id).expect("job")).expect("serialises");
        assert_eq!(json["state"], serde_json::json!("failed"));
        assert_eq!(json["failure"]["kind"], serde_json::json!("rate_limited"));

        let round_trip: Job = serde_json::from_value(json).expect("deserialises");
        assert_eq!(round_trip.state, JobState::Failed);
        assert_eq!(
            round_trip.failure.expect("failure").kind,
            DownloadFailureKind::RateLimited
        );
    }
}
