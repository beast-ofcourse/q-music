import type { InstallPhase, InstallProgress } from "../../lib/extensions";

const PHASE_LABEL: Record<InstallPhase, string> = {
  queued: "Queued",
  downloading: "Downloading",
  verifying: "Verifying",
  installing: "Installing",
  done: "Installed",
  failed: "Failed",
};

interface InstallProgressBarProps {
  name: string;
  progress: InstallProgress;
}

/** The in-flight state of one install, drawn where the Install button was. */
export function InstallProgressBar({ name, progress }: InstallProgressBarProps) {
  const percent = Math.min(100, Math.max(0, Math.round(progress.percent)));
  const phase = PHASE_LABEL[progress.phase];

  return (
    <div className="ext-progress">
      <div className="ext-progress-head">
        <span className="ext-phase" aria-live="polite">
          {phase}
        </span>
        <span className="ext-percent">{percent}%</span>
      </div>
      <div
        className="ext-progress-track"
        role="progressbar"
        aria-label={`${name} install progress`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={`${phase}, ${percent} percent`}
      >
        <span className="ext-progress-fill" style={{ width: `${percent}%` }} />
      </div>
      {progress.message ? <p className="ext-progress-note">{progress.message}</p> : null}
    </div>
  );
}
