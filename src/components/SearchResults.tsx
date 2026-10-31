import { useEffect, useState } from "react";

import { backend } from "../lib/ipc";
import { formatTime } from "../lib/format";
import type { ProviderSearchOutcome, ProviderTrack } from "../lib/extensions";
import { useStore } from "../state/store";

/**
 * Professional search results view.
 *
 * Local tracks render immediately; provider results append as they arrive.
 * Each provider gets its own bordered section with a header showing name,
 * status, and result count. Track rows follow the library table's visual
 * language: 40px artwork, title + artist, album (hidden on narrow), duration,
 * and hover-revealed actions.
 */
export function SearchResults() {
  const { query, tracks, tracksLoading, play, notify, openNow } = useStore();
  const [providerResults, setProviderResults] = useState<ProviderSearchOutcome[]>([]);
  const [providerLoading, setProviderLoading] = useState(false);
  const [downloadingId, setDownloadingId] = useState<string | null>(null);

  useEffect(() => {
    if (!query.search || query.search.trim() === "") {
      setProviderResults([]);
      return;
    }

    const abort = new AbortController();

    const doSearch = async () => {
      setProviderLoading(true);
      try {
        const results = await backend.searchProviders(query.search!, 10);
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

  const hasLocalResults = tracks.length > 0;
  const hasProviderResults = providerResults.some((r) => r.results.length > 0);
  const allDone = !tracksLoading && !providerLoading;

  if (!query.search) {
    return null;
  }

  if (tracksLoading && providerResults.length === 0) {
    return (
      <div className="search-results">
        <div className="search-skeleton" aria-hidden="true">
          {Array.from({ length: 6 }, (_, i) => (
            <div key={i} className="search-skeleton-row" />
          ))}
        </div>
      </div>
    );
  }

  if (!hasLocalResults && !hasProviderResults && allDone) {
    return (
      <div className="search-results">
        <div className="search-empty">
          <p className="search-empty-title">No results</p>
          <p className="search-empty-body">
            Nothing matches &ldquo;{query.search}&rdquo; in the library or from providers.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="search-results">
      {/* Local library */}
      {hasLocalResults && (
        <section className="search-section">
          <header className="search-section-head">
            <h3 className="search-section-title">Local library</h3>
            <span className="search-section-count">{tracks.length}</span>
          </header>
          <div className="search-track-list">
            {tracks.map((track) => (
              <div
                key={track.id}
                className="search-track-row"
                onClick={() => void play(track, [track.id])}
              >
                <span className="search-track-art">
                  <span className="search-track-play" aria-hidden="true">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
                      <path d="M8 5v14l11-7z" />
                    </svg>
                  </span>
                </span>
                <span className="search-track-info">
                  <span className="search-track-title">
                    {track.title || "Untitled"}
                    <span className="search-source-chip local">Local</span>
                  </span>
                  <span className="search-track-artist">
                    {track.artist || "\u2014"}
                    {track.album ? ` \u00b7 ${track.album}` : ""}
                  </span>
                </span>
                <span className="search-track-duration">
                  {formatTime(track.durationMs)}
                </span>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Provider results */}
      {providerResults.length > 0 && (
        <section className="search-section">
          <header className="search-section-head">
            <h3 className="search-section-title">Online</h3>
            <span className="search-section-count">
              {providerResults.reduce((n, r) => n + r.results.length, 0)}
            </span>
          </header>

          <div className="search-provider-list">
            {providerResults.map((outcome) => (
              <div key={outcome.provider} className="search-provider-card">
                <header className="search-provider-head">
                  <span className="search-provider-name">{outcome.providerName}</span>
                  {outcome.failure && (
                    <span className="search-provider-status error" role="status">
                      {outcome.failure}
                    </span>
                  )}
                  {!outcome.failure && (
                    <span className="search-provider-status ok">
                      {outcome.results.length} result{outcome.results.length !== 1 ? "s" : ""}
                    </span>
                  )}
                </header>

                {outcome.results.length > 0 && (
                  <div className="search-track-list">
                    {outcome.results.map((track) => (
                      <div key={track.providerTrackId} className="search-track-row">
                        <span className="search-track-art">
                          {track.artUrl ? (
                            <img src={track.artUrl} alt="" loading="lazy" />
                          ) : (
                            <span className="search-track-art-fallback" aria-hidden="true">
                              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor">
                                <path d="M12 3v10.55c-.59-.34-1.27-.55-2-.55-2.21 0-4 1.79-4 4s1.79 4 4 4 4-1.79 4-4V7h4V3h-6z" />
                              </svg>
                            </span>
                          )}
                          <span className="search-track-play" aria-hidden="true">
                            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
                              <path d="M8 5v14l11-7z" />
                            </svg>
                          </span>
                        </span>
                        <span className="search-track-info">
                          <span className="search-track-title">
                            {track.title}
                            <span className="search-source-chip">{outcome.providerName}</span>
                          </span>
                          <span className="search-track-artist">
                            {track.artist}
                            {track.album ? ` \u00b7 ${track.album}` : ""}
                          </span>
                        </span>
                        <span className="search-track-duration">
                          {formatTime(track.durationMs)}
                        </span>
                        <span className="search-track-actions">
                          <button
                            type="button"
                            className="search-action-btn"
                            onClick={() => void handleDownload(track)}
                            disabled={downloadingId === track.providerTrackId}
                            aria-label={`Download ${track.title}`}
                            title="Download"
                          >
                            {downloadingId === track.providerTrackId ? (
                              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                                <circle cx="12" cy="12" r="10" strokeDasharray="31.4" strokeDashoffset="10" />
                              </svg>
                            ) : (
                              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                                <polyline points="7 10 12 15 17 10" />
                                <line x1="12" y1="15" x2="12" y2="3" />
                              </svg>
                            )}
                          </button>
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Loading indicator */}
      {providerLoading && (
        <div className="search-status-bar" role="status" aria-live="polite">
          <span className="search-status-dot" />
          Searching providers&hellip;
        </div>
      )}
    </div>
  );
}