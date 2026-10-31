import { useEffect, useMemo, useRef, useState } from "react";

import { backend } from "../lib/ipc";
import { describe, useStore } from "../state/store";
import type { ProviderLyrics } from "../lib/extensions";

const SOURCE_LABEL: Record<string, string> = {
  embedded: "embedded in the file",
  sidecar: "from a .lrc beside the file",
  manual: "written in Q-Music",
  none: "none",
};

export function LyricsPane() {
  const {
    lyrics,
    lyricsLoading,
    snapshot,
    seek,
    currentTrack,
    setView,
    notify,
    loadLyrics,
  } = useStore();
  const [providerLyricsLoading, setProviderLyricsLoading] = useState(false);
  const [providerLyricsError, setProviderLyricsError] = useState<string | null>(null);
  const [providerLyricsData, setProviderLyricsData] = useState<ProviderLyrics | null>(null);

  const activeIndex = useMemo(() => {
    if (!lyrics || !lyrics.synced) return -1;
    let found = -1;
    lyrics.lines.forEach((line, index) => {
      if (line.timeMs !== null && line.timeMs <= snapshot.positionMs) found = index;
    });
    return found;
  }, [lyrics, snapshot.positionMs]);

  const activeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex]);

  async function importFile() {
    if (!currentTrack) return;
    try {
      const imported = await backend.importLyrics(currentTrack.id);
      if (!imported) return;
      await loadLyrics(currentTrack.id);
      notify(`Imported lyrics for ${currentTrack.title}`);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function fetchProviderLyrics() {
    if (!currentTrack) return;
    setProviderLyricsLoading(true);
    setProviderLyricsError(null);
    try {
      const result = await backend.providerLyrics(currentTrack.id);
      if (result) {
        setProviderLyricsData(result);
        notify(`Found lyrics from ${result.providerName}`);
      } else {
        setProviderLyricsError("No lyrics found from any provider");
      }
    } catch (error) {
      setProviderLyricsError(describe(error));
    } finally {
      setProviderLyricsLoading(false);
    }
  }

  if (!currentTrack) {
    return (
      <div className="empty">
        <p className="empty-title">No track playing</p>
        <p className="empty-body">
          Lyrics appear here once something is playing. Embedded lyrics and a `.lrc` or
          `.txt` file beside the audio are both read.
        </p>
      </div>
    );
  }

  if (lyricsLoading) {
    return <p className="hint" style={{ padding: "16px" }}>Reading lyrics…</p>;
  }

  // If we have provider lyrics, render them even if there are no local lyrics
  const hasProviderLyrics = providerLyricsData !== null;

  if ((!lyrics || lyrics.lines.length === 0) && !hasProviderLyrics) {
    return (
      <div className="empty">
        <p className="empty-title">No lyrics for this track</p>
        <p className="empty-body">
          Import a `.lrc` or `.txt` file, or write them by hand and Q-Music will keep the
          timings beside the audio file.
        </p>
        <div className="actions">
          <button type="button" className="button" onClick={() => void importFile()}>
            Import file
          </button>
          <button
            type="button"
            className="button button-quiet"
            onClick={() => setView({ kind: "editor" })}
          >
            Write them
          </button>
          <button
            type="button"
            className="button button-quiet"
            onClick={() => void fetchProviderLyrics()}
            disabled={providerLyricsLoading}
          >
            {providerLyricsLoading ? "Searching providers…" : "Search providers"}
          </button>
        </div>
        {providerLyricsError && <p className="lyric-banner" role="alert">{providerLyricsError}</p>}
      </div>
    );
  }

  return (
    <>
      {lyrics?.warning ? <p className="lyric-banner">{lyrics.warning}</p> : null}

      <div className="lyrics">
        {(providerLyricsData ? providerLyricsData.lines : (lyrics?.lines ?? [])).map((line, index) => {
          const isActive = index === activeIndex;
          const isPast = activeIndex >= 0 && index < activeIndex;
          return (
            <button
              key={`${index}-${line.text}`}
              ref={isActive ? activeRef : undefined}
              type="button"
              className="lyric-line"
              data-active={isActive}
              data-past={isPast}
              disabled={line.timeMs === null}
              onClick={() => {
                if (line.timeMs !== null) seek(line.timeMs);
              }}
            >
              {line.text}
            </button>
          );
        })}
      </div>

      <p className="lyric-source">
        {providerLyricsData
          ? `${providerLyricsData.providerName} · ${providerLyricsData.synced ? "timed" : "no timings"}`
          : lyrics
          ? `${SOURCE_LABEL[lyrics.source] ?? lyrics.source} · ${lyrics.synced ? "timed" : "no timings"}`
          : "provider lyrics"}
      </p>

      <div className="lyric-actions">
        <button
          type="button"
          className="button button-quiet"
          onClick={() => setView({ kind: "lyrics-view" })}
        >
          Full screen
        </button>
      </div>

      <div className="lyric-actions">
        {providerLyricsData ? (
          <button
            type="button"
            className="button button-quiet"
            onClick={() => {
              setProviderLyricsData(null);
              setProviderLyricsError(null);
            }}
          >
            Back to local lyrics
          </button>
        ) : (
          <button
            type="button"
            className="button button-quiet"
            onClick={() => void fetchProviderLyrics()}
            disabled={providerLyricsLoading}
          >
            {providerLyricsLoading ? "Searching providers…" : "Search providers"}
          </button>
        )}
        {providerLyricsError && <span className="lyric-banner" role="alert">{providerLyricsError}</span>}
      </div>
    </>
  );
}
