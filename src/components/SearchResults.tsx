import { useEffect, useRef, useState } from "react";

import { backend } from "../lib/ipc";
import { formatTime } from "../lib/format";
import type { OnlineSearchResponse, OnlineTrackResult, OnlineAlbumResult, OnlineArtistResult, OnlinePlaylistResult } from "../lib/extensions";
import { useStore } from "../state/store";

export function SearchResults({ filter = "all" }: { filter?: string }) {
  const { query } = useStore();
  const [data, setData] = useState<OnlineSearchResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestIdRef = useRef(0);
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!query.search || query.search.trim() === "") {
      setData(null);
      setLoading(false);
      setError(null);
      return;
    }
    const q = query.search.trim();
    const thisId = ++requestIdRef.current;
    const rid = `req-${thisId}`;
    const controller = new AbortController();
    abortRef.current = controller;
    setLoading(true);
    setError(null);

    window.setTimeout(async () => {
      const types =
        filter === "all" ? ["tracks", "albums", "artists", "playlists"] : [filter];
      try {
        const res = await backend.onlineSearch(rid, q, types, 10, 0);
        if (controller.signal.aborted || thisId !== requestIdRef.current) return;
        setData(res);
      } catch (e) {
        if (controller.signal.aborted || thisId !== requestIdRef.current) return;
        setError(e instanceof Error ? e.message : String(e));
        setData(null);
      } finally {
        if (!controller.signal.aborted && thisId === requestIdRef.current) {
          setLoading(false);
        }
      }
    }, 220);

    return () => {
      controller.abort();
      void backend.onlineSearchCancel(rid);
    };
  }, [query.search, filter]);

  const showTracks = filter === "all" || filter === "tracks";
  const showAlbums = filter === "all" || filter === "albums";
  const showArtists = filter === "all" || filter === "artists";
  const showPlaylists = filter === "all" || filter === "playlists";

  const hasAny =
    (showTracks && (data?.tracks.length ?? 0) > 0) ||
    (showAlbums && (data?.albums.length ?? 0) > 0) ||
    (showArtists && (data?.artists.length ?? 0) > 0) ||
    (showPlaylists && (data?.playlists.length ?? 0) > 0);

  if (!query.search) return null;

  if (loading && !data) {
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

  if (error) {
    return (
      <div className="search-results">
        <div className="search-empty">
          <p className="search-empty-title">Search failed</p>
          <p className="search-empty-body">{error}</p>
        </div>
      </div>
    );
  }

  if (!hasAny && !loading) {
    return (
      <div className="search-results">
        <div className="search-empty">
          <p className="search-empty-title">No results</p>
          <p className="search-empty-body">
            Nothing matches &ldquo;{query.search}&rdquo;.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="search-results">
      {showTracks && data && data.tracks.length > 0 && (
        <section className="search-section">
          <header className="search-section-head">
            <h3 className="search-section-title">Songs</h3>
            <span className="search-section-count">{data.tracks.length}</span>
          </header>
          <div className="search-track-list">
            {data.tracks.map((t) => (
              <TrackRow key={t.id} track={t} />
            ))}
          </div>
        </section>
      )}
      {showAlbums && data && data.albums.length > 0 && (
        <section className="search-section">
          <header className="search-section-head">
            <h3 className="search-section-title">Albums</h3>
            <span className="search-section-count">{data.albums.length}</span>
          </header>
          <div className="search-track-list">
            {data.albums.map((a) => (
              <AlbumRow key={a.id} album={a} />
            ))}
          </div>
        </section>
      )}
      {showArtists && data && data.artists.length > 0 && (
        <section className="search-section">
          <header className="search-section-head">
            <h3 className="search-section-title">Artists</h3>
            <span className="search-section-count">{data.artists.length}</span>
          </header>
          <div className="search-track-list">
            {data.artists.map((a) => (
              <ArtistRow key={a.id} artist={a} />
            ))}
          </div>
        </section>
      )}
      {showPlaylists && data && data.playlists.length > 0 && (
        <section className="search-section">
          <header className="search-section-head">
            <h3 className="search-section-title">Playlists</h3>
            <span className="search-section-count">{data.playlists.length}</span>
          </header>
          <div className="search-track-list">
            {data.playlists.map((p) => (
              <PlaylistRow key={p.id} playlist={p} />
            ))}
          </div>
        </section>
      )}
      {loading && (
        <div className="search-status-bar" role="status" aria-live="polite">
          <span className="search-status-dot" />
          Searching providers&hellip;
        </div>
      )}
    </div>
  );
}

function TrackRow({ track }: { track: OnlineTrackResult }) {
  const { notify, openNow } = useStore();
  const [downloading, setDownloading] = useState(false);
  async function handleDownloadClick() {
    setDownloading(true);
    try {
      await backend.enqueueDownload({
        provider: track.provider,
        providerName: track.providerName,
        providerTrackId: track.providerTrackId,
        title: track.title,
        artist: track.artists.join(", "),
        album: track.album ?? "",
        durationMs: track.durationMs,
        artUrl: track.artUrl ?? null,
        quality: "best",
      } as any);
      notify(`"${track.title}" added to the download queue`);
      openNow("downloads");
    } catch (e) {
      notify(`Download failed: ${e instanceof Error ? e.message : String(e)}`, "error");
    } finally {
      setDownloading(false);
    }
  }
  return (
    <div className="search-track-row">
      <span className="search-track-art">
        {track.artUrl ? <img src={track.artUrl} alt="" loading="lazy" /> : <span className="search-track-art-fallback" aria-hidden="true" />}
      </span>
      <span className="search-track-info">
        <span className="search-track-title">
          {track.title}
          <span className="search-source-chip">{track.providerName}</span>
          {track.inLibrary && <span className="search-source-chip local">In library</span>}
        </span>
        <span className="search-track-artist">
          {track.artists.join(", ")}
          {track.album ? ` · ${track.album}` : ""}
        </span>
      </span>
      <span className="search-track-duration">{formatTime(track.durationMs)}</span>
      <span className="search-track-actions">
        <button type="button" className="search-action-btn" onClick={() => void handleDownloadClick()} disabled={downloading} aria-label={`Download ${track.title}`}>
          {downloading ? "…" : "↓"}
        </button>
      </span>
    </div>
  );
}

function AlbumRow({ album }: { album: OnlineAlbumResult }) {
  return (
    <div className="search-track-row">
      <span className="search-track-art">
        {album.artUrl ? <img src={album.artUrl} alt="" loading="lazy" /> : <span className="search-track-art-fallback" aria-hidden="true" />}
      </span>
      <span className="search-track-info">
        <span className="search-track-title">
          {album.title}
          <span className="search-source-chip">{album.providerName}</span>
        </span>
        <span className="search-track-artist">
          {album.artists.join(", ")}
          {album.year ? ` · ${album.year}` : ""}
          {album.trackCount ? ` · ${album.trackCount} tracks` : ""}
        </span>
      </span>
    </div>
  );
}

function ArtistRow({ artist }: { artist: OnlineArtistResult }) {
  return (
    <div className="search-track-row">
      <span className="search-track-art">
        {artist.artUrl ? <img src={artist.artUrl} alt="" loading="lazy" /> : <span className="search-track-art-fallback" aria-hidden="true" />}
      </span>
      <span className="search-track-info">
        <span className="search-track-title">
          {artist.name}
          <span className="search-source-chip">{artist.providerName}</span>
        </span>
      </span>
    </div>
  );
}

function PlaylistRow({ playlist }: { playlist: OnlinePlaylistResult }) {
  return (
    <div className="search-track-row">
      <span className="search-track-art">
        {playlist.artUrl ? <img src={playlist.artUrl} alt="" loading="lazy" /> : <span className="search-track-art-fallback" aria-hidden="true" />}
      </span>
      <span className="search-track-info">
        <span className="search-track-title">
          {playlist.title}
          <span className="search-source-chip">{playlist.providerName}</span>
        </span>
        <span className="search-track-artist">
          {playlist.creator ?? "Unknown creator"}
          {playlist.trackCount ? ` · ${playlist.trackCount} tracks` : ""}
        </span>
      </span>
    </div>
  );
}