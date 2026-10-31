import { useCallback, useEffect, useState } from "react";

import { backend } from "../lib/ipc";
import { formatBytes, formatTransferRate } from "../lib/format";
import { toExtensionFailure, type JobStage, type QueueJob } from "../lib/extensions";
import "../styles/extensions-queue.css";

const STAGE_LABEL: Record<JobStage, string> = {
  queued: "Queued",
  resolving: "Resolving",
  downloading: "Downloading",
  post_processing: "Post-processing",
  embedding: "Embedding tags",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

const TERMINAL_STAGES: JobStage[] = ["done", "failed", "cancelled"];

function withoutKey(record: Record<string, string>, key: string): Record<string, string> {
  if (!(key in record)) {
    return record;
  }
  const next = { ...record };
  delete next[key];
  return next;
}

function figuresFor(job: QueueJob): string {
  const written = formatBytes(job.bytesWritten);
  const total = job.bytesTotal === null ? "unknown size" : formatBytes(job.bytesTotal);
  const rate = job.stage === "downloading" ? ` at ${formatTransferRate(job.speedBps)}` : "";
  return `${written} of ${total}${rate}`;
}

/**
 * Download jobs the host is running, as the queue reports them. This is not the play
 * queue: a job is a file being fetched, converted and tagged, and it can fail for reasons
 * the transport bar has nowhere to show.
 */
export function DownloadsPane() {
  const [jobs, setJobs] = useState<QueueJob[] | null>(null);
  const [readFailure, setReadFailure] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [noticeById, setNoticeById] = useState<Record<string, string>>({});

  const readJobs = useCallback(async () => {
    setReadFailure(null);
    try {
      setJobs(await backend.queueJobs());
    } catch (error) {
      setReadFailure(toExtensionFailure(error).message);
    }
  }, []);

  useEffect(() => {
    void readJobs();
  }, [readJobs]);

  useEffect(() => {
    let stop: (() => void) | null = null;
    let cancelled = false;

    void backend
      .onQueue((next) => setJobs(next))
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
          return;
        }
        stop = unlisten;
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setReadFailure(toExtensionFailure(error).message);
        }
      });

    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  async function run(jobId: string, action: () => Promise<void>) {
    setNoticeById((current) => withoutKey(current, jobId));
    setBusyId(jobId);
    try {
      await action();
      await readJobs();
    } catch (error) {
      setNoticeById((current) => ({
        ...current,
        [jobId]: toExtensionFailure(error).message,
      }));
    } finally {
      setBusyId(null);
    }
  }

  function cancel(jobId: string) {
    return run(jobId, () => backend.cancelJob(jobId));
  }

  function retry(jobId: string) {
    return run(jobId, () => backend.retryJob(jobId));
  }

  if (jobs === null) {
    return readFailure === null ? (
      <p className="dl-loading" role="status">
        Reading the download queue...
      </p>
    ) : (
      <div className="dl-banner" role="alert">
        <p className="dl-banner-text">
          <strong className="dl-banner-label">The queue could not be read.</strong> {readFailure}
        </p>
        <button type="button" className="dl-action" onClick={() => void readJobs()}>
          Try again
        </button>
      </div>
    );
  }

  if (jobs.length === 0) {
    return (
      <div className="empty">
        <p className="empty-title">No downloads</p>
        <p className="empty-body">
          A download appears here while its provider resolves the track, fetches the file and
          writes the tags. Nothing is queued right now.
        </p>
      </div>
    );
  }

  return (
    <div className="dl-pane">
      {readFailure === null ? null : (
        <div className="dl-banner" role="alert">
          <p className="dl-banner-text">
            <strong className="dl-banner-label">The queue stopped updating.</strong> {readFailure}
          </p>
          <button type="button" className="dl-action" onClick={() => void readJobs()}>
            Try again
          </button>
        </div>
      )}

      <ol className="dl-list" aria-label="Download queue">
        {jobs.map((job) => {
          const terminal = TERMINAL_STAGES.includes(job.stage);
          const busy = busyId === job.id;
          return (
            <li className="dl-job" key={job.id} data-job-id={job.id} data-stage={job.stage}>
              <div className="dl-head">
                <span className="dl-stage" aria-live="polite">
                  {STAGE_LABEL[job.stage]}
                </span>
                {job.percent === null ? null : (
                  <span className="dl-percent">{job.percent}%</span>
                )}
              </div>

              <span className="dl-title">{job.title}</span>
              <span className="dl-sub">
                {job.artist} / {job.provider} / {job.quality}
              </span>

              {job.percent === null ? null : (
                <div
                  className="dl-track"
                  role="progressbar"
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={job.percent}
                  aria-valuetext={`${STAGE_LABEL[job.stage]}, ${job.percent} percent`}
                >
                  <span className="dl-fill" style={{ width: `${job.percent}%` }} />
                </div>
              )}

              <span className="dl-figures">{figuresFor(job)}</span>

              {job.failure === null ? null : (
                <p className="dl-failure">
                  <span className="dl-failure-kind">{job.failure.kind}</span>
                  <span className="dl-failure-text">{job.failure.message}</span>
                  <span className="dl-failure-stage">
                    The job stopped while {STAGE_LABEL[job.failure.stage].toLowerCase()}.
                  </span>
                </p>
              )}

              {terminal ? (
                job.stage === "failed" ? (
                  <div className="dl-actions">
                    <button
                      type="button"
                      className="dl-action"
                      disabled={busy}
                      onClick={() => void retry(job.id)}
                      aria-label={`Retry ${job.title}`}
                    >
                      {busy ? "Retrying..." : "Retry"}
                    </button>
                  </div>
                ) : null
              ) : (
                <div className="dl-actions">
                  <button
                    type="button"
                    className="dl-action"
                    disabled={busy}
                    onClick={() => void cancel(job.id)}
                    aria-label={`Cancel ${job.title}`}
                  >
                    {busy ? "Cancelling..." : "Cancel"}
                  </button>
                </div>
              )}

              {noticeById[job.id] === undefined ? null : (
                <p className="dl-banner" role="alert">
                  <span className="dl-banner-text">{noticeById[job.id]}</span>
                </p>
              )}
            </li>
          );
        })}
      </ol>
    </div>
  );
}
