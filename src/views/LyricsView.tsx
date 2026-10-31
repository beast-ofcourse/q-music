import { useEffect, useMemo, useRef } from "react";

import { Icon } from "../components/Icon";
import { useArtwork } from "../lib/artwork";
import { useStore } from "../state/store";
import "../styles/lyrics-view.css";

/**
 * Full-screen synced lyrics view.
 *
 * Big, minimal, visually synced: the active line is centered and scaled,
 * past lines fade up, future lines fade down. Auto-scrolls to keep the
 * active line in the middle. No chrome, no borders — just the words.
 */
export function LyricsView() {
  const { lyrics, lyricsLoading, snapshot, seek, currentTrack, setView, loadLyrics } =
    useStore();
  const artwork = useArtwork(currentTrack?.artId ?? null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);

  const activeIndex = useMemo(() => {
    if (!lyrics || !lyrics.synced) return -1;
    let found = -1;
    lyrics.lines.forEach((line, index) => {
      if (line.timeMs !== null && line.timeMs <= snapshot.positionMs) found = index;
    });
    return found;
  }, [lyrics, snapshot.positionMs]);

  useEffect(() => {
    if (currentTrack) {
      void loadLyrics(currentTrack.id);
    }
  }, [currentTrack?.id, loadLyrics]);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [activeIndex]);

  if (!currentTrack) {
    return (
      <div className="lyrics-view">
        <div className="lyrics-view-empty">
          <Icon name="disc" size={48} />
          <p className="lyrics-view-empty-title">No track playing</p>
          <p className="lyrics-view-empty-body">
            Start a track to see its lyrics here.
          </p>
        </div>
      </div>
    );
  }

  if (lyricsLoading) {
    return (
      <div className="lyrics-view">
        <div className="lyrics-view-loading" role="status">
          Reading lyrics&hellip;
        </div>
      </div>
    );
  }

  if (!lyrics || lyrics.lines.length === 0) {
    return (
      <div className="lyrics-view">
        <div className="lyrics-view-empty">
          <Icon name="disc" size={48} />
          <p className="lyrics-view-empty-title">No lyrics for this track</p>
          <p className="lyrics-view-empty-body">
            {currentTrack.title} &mdash; {currentTrack.artist}
          </p>
          <button
            type="button"
            className="lyrics-view-back"
            onClick={() => setView({ kind: "editor" })}
          >
            Write them
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="lyrics-view">
      {/* Blurred artwork background */}
      {artwork && (
        <div
          className="lyrics-view-bg"
          style={{ backgroundImage: `url(${artwork})` }}
          aria-hidden="true"
        />
      )}

      {/* Close button */}
      <button
        type="button"
        className="lyrics-view-close"
        onClick={() => setView({ kind: "library" })}
        aria-label="Close lyrics view"
      >
        <Icon name="close" size={20} />
      </button>

      {/* Track info */}
      <div className="lyrics-view-track">
        <span className="lyrics-view-track-title">{currentTrack.title}</span>
        <span className="lyrics-view-track-artist">{currentTrack.artist}</span>
      </div>

      {/* Lyrics */}
      <div className="lyrics-view-scroll" ref={scrollRef}>
        <div className="lyrics-view-spacer" />
        {lyrics.lines.map((line, index) => {
          const isActive = index === activeIndex;
          const isPast = activeIndex >= 0 && index < activeIndex;
          const isFuture = activeIndex >= 0 && index > activeIndex;
          return (
            <button
              key={`${index}-${line.text}`}
              ref={isActive ? activeRef : undefined}
              type="button"
              className="lyrics-view-line"
              data-active={isActive}
              data-past={isPast}
              data-future={isFuture}
              disabled={line.timeMs === null}
              onClick={() => {
                if (line.timeMs !== null) seek(line.timeMs);
              }}
            >
              {line.text}
            </button>
          );
        })}
        <div className="lyrics-view-spacer" />
      </div>
    </div>
  );
}