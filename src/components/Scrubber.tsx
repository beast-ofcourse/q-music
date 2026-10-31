import { useState, type PointerEvent as ReactPointerEvent } from "react";

import { formatTime } from "../lib/format";

const SEEK_STEP_MS = 5_000;
const COARSE_SEEK_MS = 30_000;

interface ScrubberProps {
  positionMs: number;
  durationMs: number;
  onSeek: (positionMs: number) => void;
  disabled: boolean;
}

/** A hairline playhead. Dragging previews the target; releasing commits the seek. */
export function Scrubber({ positionMs, durationMs, onSeek, disabled }: ScrubberProps) {
  const [dragging, setDragging] = useState(false);
  const [preview, setPreview] = useState<number | null>(null);

  const playedRatio =
    dragging && preview !== null
      ? preview
      : durationMs > 0
        ? Math.min(1, Math.max(0, positionMs / durationMs))
        : 0;

  function ratioAt(element: HTMLElement, clientX: number): number {
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0) return 0;
    return Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
  }

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
    setPreview(ratioAt(event.currentTarget, event.clientX));
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (!dragging) return;
    setPreview(ratioAt(event.currentTarget, event.clientX));
  }

  function handlePointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    if (!dragging) return;
    const ratio = ratioAt(event.currentTarget, event.clientX);
    setDragging(false);
    setPreview(null);
    onSeek(Math.round(ratio * durationMs));
  }

  function nudge(deltaMs: number) {
    if (disabled || durationMs <= 0) return;
    onSeek(Math.min(durationMs, Math.max(0, positionMs + deltaMs)));
  }

  return (
    <div className="scrub">
      <span className="scrub-time">{formatTime(positionMs)}</span>

      <div
        className="scrub-track"
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label="Playback position"
        aria-valuemin={0}
        aria-valuemax={Math.round(durationMs)}
        aria-valuenow={Math.round(positionMs)}
        aria-valuetext={`${formatTime(positionMs)} of ${formatTime(durationMs)}`}
        aria-disabled={disabled}
        data-dragging={dragging}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={() => {
          setDragging(false);
          setPreview(null);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowRight") {
            event.preventDefault();
            nudge(event.shiftKey ? COARSE_SEEK_MS : SEEK_STEP_MS);
          } else if (event.key === "ArrowLeft") {
            event.preventDefault();
            nudge(event.shiftKey ? -COARSE_SEEK_MS : -SEEK_STEP_MS);
          } else if (event.key === "Home") {
            event.preventDefault();
            onSeek(0);
          } else if (event.key === "End") {
            event.preventDefault();
            onSeek(durationMs);
          }
        }}
      >
        <span className="scrub-line" />
        <span className="scrub-fill" style={{ width: `${playedRatio * 100}%` }} />
        <span className="scrub-head" style={{ left: `${playedRatio * 100}%` }} />
      </div>

      <span className="scrub-time">{formatTime(durationMs)}</span>
    </div>
  );
}
