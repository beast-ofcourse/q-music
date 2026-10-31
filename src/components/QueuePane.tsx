import { formatTime } from "../lib/format";
import type { Track } from "../lib/types";
import { useStore } from "../state/store";

/** The queue the current track was started from, not a separate hand-managed list. */
export function QueuePane() {
  const { queue, known, currentId, play } = useStore();
  const items = queue
    .map((id) => known[id])
    .filter((track): track is Track => track !== undefined);

  if (items.length === 0) {
    return (
      <div className="empty">
        <p className="empty-title">Nothing queued</p>
        <p className="empty-body">
          Press play on any row and the rows around it become the queue. Shuffle and repeat
          then decide what follows.
        </p>
      </div>
    );
  }

  return (
    <ol aria-label="Play queue">
      {items.map((track, index) => (
        <li key={`${track.id}-${index}`}>
          <button
            type="button"
            className="queue-item"
            data-current={track.id === currentId}
            onClick={() => void play(track, queue)}
          >
            <span className="queue-index">{String(index + 1).padStart(2, "0")}</span>
            <span className="queue-text">
              <span className="queue-title">{track.title || "Untitled"}</span>
              <span className="queue-artist">{track.artist || "—"}</span>
            </span>
            <span className="queue-time">{formatTime(track.durationMs)}</span>
          </button>
        </li>
      ))}
    </ol>
  );
}
