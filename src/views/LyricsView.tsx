import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Icon } from "../components/Icon";
import { useArtwork } from "../lib/artwork";
import { useStore } from "../state/store";
import "../styles/lyrics-view.css";

/**
 * Full-screen synced lyrics view.
 *
 * Two-column layout: album artwork (35%) on the left, lyrics (65%) on the right.
 * Clean, minimal, high-contrast. No glassmorphism, no excessive blur.
 */
export function LyricsView() {
  const { lyrics, lyricsLoading, snapshot, seek, currentTrack, setView, loadLyrics } =
    useStore();
  const artwork = useArtwork(currentTrack?.artId ?? null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);
  const [userScrolled, setUserScrolled] = useState(false);
  const scrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

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

  // Auto-scroll to active line, but only if user hasn't manually scrolled recently.
  useEffect(() => {
    if (userScrolled) return;
    activeRef.current?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [activeIndex, userScrolled]);

  const handleScroll = useCallback(() => {
    if (scrollTimerRef.current) {
      clearTimeout(scrollTimerRef.current);
    }
    setUserScrolled(true);
    scrollTimerRef.current = setTimeout(() => {
      setUserScrolled(false);
    }, 4000);
  }, []);

  const returnToCurrent = useCallback(() => {
    setUserScrolled(false);
    activeRef.current?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, []);

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
      {/* Close button */}
      <button
        type="button"
        className="lyrics-view-close"
        onClick={() => setView({ kind: "library" })}
        aria-label="Close lyrics view"
      >
        <Icon name="close" size={20} />
      </button>

      {/* Two-column layout */}
      <div className="lyrics-view-columns">
        {/* Left: Album artwork + song info (35%) */}
        <div className="lyrics-view-album">
          {artwork && (
            <img
              src={artwork}
              alt=""
              className="lyrics-view-artwork"
            />
          )}
          {!artwork && (
            <div className="lyrics-view-artwork-fallback" aria-hidden="true">
              <Icon name="disc" size={64} />
            </div>
          )}
          <div className="lyrics-view-song-info">
            <span className="lyrics-view-song-title">{currentTrack.title}</span>
            <span className="lyrics-view-song-artist">{currentTrack.artist}</span>
          </div>
        </div>

        {/* Right: Lyrics (65%) */}
        <div
          className="lyrics-view-lyrics"
          ref={scrollRef}
          onScroll={handleScroll}
        >
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

      {/* Return to current button */}
      {userScrolled && (
        <button
          type="button"
          className="lyrics-view-return"
          onClick={returnToCurrent}
        >
          Return to current
        </button>
      )}
    </div>
  );
}