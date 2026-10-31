import { useEffect, useState } from "react";

import { backend } from "../lib/ipc";
import { formatTime } from "../lib/format";
import type { ProviderSearchOutcome, ProviderTrack } from "../lib/extensions";
import { useStore } from "../state/store";

interface SearchResultRowProps {
  track: ProviderTrack;
  source: string;
  onPlay: (track: ProviderTrack) => void;
}

/** A provider track row with its source chip. */
function SearchResultRow({ track, source, onPlay }: SearchResultRowProps) {
  return (
    <div
      role="row"
      className="search-result-row"
      onClick={() => onPlay(track)}
    >
      <span role="cell" className="search-cell-play">
        <button type="button" className="row-play" aria-label={`Play ${track.title}`}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
        </button>
      </span>
      <span role="cell" className="search-cell-title">
        {track.title}
        <span className="search-source-chip">{source}</span>
      </span>
      <span role="cell" className="search-cell-artist">{track.artist}</span>
      <span role="cell" className="search-cell-album">{track.album}</span>
      <span role="cell" className="search-cell-time">{formatTime(track.durationMs)}</span>
    </div>
  );
}

/**
 * Search view that merges local tracks with provider results.
 * Local results render immediately; provider results append as they arrive under a source chip.
 * A provider timeout shows a quiet inline message without delaying other providers.
 */
export function SearchResults() {
  const { query, tracks, tracksLoading, play, notify, openNow } = useStore();
  const [providerResults, setProviderResults] = useState<ProviderSearchOutcome[]>([]);
  const [providerLoading, setProviderLoading] = useState(false);
  const [downloadingId, setDownloadingId] = useState<string | null>(null);

  async function handleDownload(track: ProviderTrack) {
    setDownloadingId(track.providerTrackId);
    try {
      await backend.enqueueDownload(track);
      notify(`"${track.title}" added to the download queue`);
      openNow("downloads");
    } catch (error) {
      notify(`Download failed: ${error instanceof Error ? error.message : String(error)}`, "error");
    } finally {
      setDownloadingId(null);
    }
  }

  // Search providers when the query changes
  useEffect(() => {
    if (!query.search || query.search.trim() === "") {
      setProviderResults([]);
      return;
    }

    const abort = new AbortController();

    const doSearch = async () => {
      setProviderLoading(true);
      try {
        const results = await backend.searchProviders(query.search!, 5);
        if (!abort.signal.aborted) {
          setProviderResults(results);
        }
      } catch {
        if (!abort.signal.aborted) {
          setProviderResults([]);
        }
      } finally {
        if (!abort.signal.aborted) {
          setProviderLoading(false);
        }
      }
    };

    const timer = setTimeout(doSearch, 50);
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
  }, [query.search]);

  // Flatten provider results for rendering
  const providerTracks = providerResults.flatMap((outcome) =>
    outcome.results.map((track) => ({ ...track, source: outcome.providerName, failure: outcome.failure }))
  );

  // Local tracks from the store
  const localTracks = tracks;

  // Check if we have any results at all
  const hasLocalResults = localTracks.length > 0;
  const hasProviderResults = providerTracks.length > 0;
  const hasFailures = providerResults.some((r) => r.failure !== null);

  if (!query.search) {
    return null;
  }

  return (
    <div className="search-results">
      {hasLocalResults && (
        <section className="search-section">
          <h3 className="search-section-title">Local library</h3>
          <div role="table" aria-label="Local search results">
            <div className="search-table-head" role="row">
              <span role="columnheader"><span className="sr-only">Play</span></span>
              <span role="columnheader">Title</span>
              <span role="columnheader">Artist</span>
              <span role="columnheader" className="col-album">Album</span>
              <span role="columnheader" className="col-time">Time</span>
            </div>
            <div className="search-table-body" role="rowgroup">
              {localTracks.map((track) => (
                <div
                  key={track.id}
                  role="row"
                  className="search-result-row"
                  onClick={() => void play(track, [track.id])}
                >
                  <span role="cell" className="search-cell-play">
                    <button type="button" className="row-play" aria-label={`Play ${track.title || "Untitled"}`}>
                      <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
                    </button>
                  </span>
                  <span role="cell" className="search-cell-title">
                    {track.title || "Untitled"}
                    <span className="search-source-chip local">Local</span>
                  </span>
                  <span role="cell" className="search-cell-artist">{track.artist || "—"}</span>
                  <span role="cell" className="search-cell-album">{track.album || "—"}</span>
                  <span role="cell" className="search-cell-time">{formatTime(track.durationMs)}</span>
                </div>
              ))}
            </div>
          </div>
        </section>
      )}

      {(hasProviderResults || hasFailures) && (
        <section className="search-section">
          <h3 className="search-section-title">Providers</h3>
          <div className="provider-results">
            {providerResults.map((outcome) => (
              <div key={outcome.provider} className="provider-group">
                <div className="provider-group-header">
                  <span className="provider-group-name">{outcome.providerName}</span>
                  {outcome.failure && (
                    <span className="provider-failure" role="status">{outcome.failure}</span>
                  )}
                </div>
                {outcome.results.length > 0 ? (
                  <div role="table" aria-label={`${outcome.providerName} results`}>
                    <div className="search-table-body" role="rowgroup">
                      {outcome.results.map((track) => (
                        <div key={track.providerTrackId} className="provider-result-row">
                          <SearchResultRow
                            track={track}
                            source={outcome.providerName}
                            onPlay={(t) => {
                              // For provider tracks, we'd need to download first.
                              console.log("Play provider track:", t);
                            }}
                          />
                          <button
                            type="button"
                            className="provider-download-btn"
                            onClick={() => void handleDownload(track)}
                            disabled={downloadingId === track.providerTrackId}
                            title={`Download ${track.title}`}
                          >
                            {downloadingId === track.providerTrackId ? "Adding..." : "Download"}
                          </button>
                        </div>
                      ))}
                    </div>
                  </div>
                ) : (
                  outcome.failure ? null : (
                    <p className="provider-empty">No matches</p>
                  )
                )}
              </div>
            ))}
          </div>
        </section>
      )}

      {!hasLocalResults && !hasProviderResults && !tracksLoading && !providerLoading && (
        <div className="search-empty">
          <p className="empty-title">No results</p>
          <p className="empty-body">
            Nothing matches "{query.search}" in the local library or from providers.
          </p>
        </div>
      )}

      {(tracksLoading || providerLoading) && (
        <div className="search-loading" role="status" aria-live="polite">
          {tracksLoading && <span>Searching library…</span>}
          {providerLoading && <span>Querying providers…</span>}
        </div>
      )}
    </div>
  );
}