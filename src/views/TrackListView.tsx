import { useEffect, useState } from "react";

import { AddFolderButton } from "../components/AddFolderButton";
import { SearchResults } from "../components/SearchResults";
import { TrackTable } from "../components/TrackTable";
import { backend } from "../lib/ipc";
import { fileName, trackSummary } from "../lib/format";
import { useStore } from "../state/store";
import "../styles/search-results.css";

const SKELETON_ROWS = 9;

interface EmptyCopy {
  title: string;
  body: string;
  withAddFolder?: boolean;
}

const EMPTY: Record<string, EmptyCopy> = {
  library: {
    title: "The library is empty",
    body: "Point Q-Music at a folder and it indexes what is inside: FLAC, ALAC, MP3, AAC, WAV, AIFF, OGG and OPUS. Nothing is copied or moved.",
    withAddFolder: true,
  },
  favorites: {
    title: "Nothing marked yet",
    body: "The heart on any row adds it here. Favourites are a filter over the same index, not a second library.",
  },
  recent: {
    title: "No play history yet",
    body: "Tracks appear here once they have been played through the engine.",
  },
};

/** Library, favourites, recents and the filtered album/artist/playlist/folder lists. */
export function TrackListView() {
  const { view, tracks, tracksLoading, query, setQuery, playlists, notify } = useStore();
  const [genres, setGenres] = useState<string[]>([]);
  const [formats, setFormats] = useState<string[]>([]);

  const filteredView = view.kind === "library" || view.kind === "favorites";

  useEffect(() => {
    if (!filteredView) return;
    let alive = true;
    void Promise.all([backend.distinctValues("genre"), backend.distinctValues("format")])
      .then(([genreValues, formatValues]) => {
        if (!alive) return;
        setGenres(genreValues);
        setFormats(formatValues);
      })
      .catch((error) => notify(`Could not read the filters: ${String(error)}`, "error"));
    return () => {
      alive = false;
    };
  }, [filteredView, notify]);

  let title = "Library";
  let subtitle: string | null = null;

  if (view.kind === "favorites") title = "Favourites";
  if (view.kind === "recent") title = "Recently played";
  if (view.kind === "album") {
    title = view.name;
    subtitle = tracks[0] ? tracks[0].albumArtist || tracks[0].artist : null;
  }
  if (view.kind === "artist") title = view.name;
  if (view.kind === "playlist") {
    title = playlists.find((playlist) => playlist.id === view.id)?.name ?? "Playlist";
  }
  if (view.kind === "folder") {
    title = fileName(view.path);
    subtitle = view.path;
  }

  const empty = EMPTY[view.kind] ?? {
    title: "Nothing here",
    body: "This view has no tracks yet.",
  };

  return (
    <>
      <div className="view-head">
        <h1 className="view-title">{title}</h1>
        {subtitle ? <p className="view-sub">{subtitle}</p> : null}

        <div className="view-tools">
          {filteredView && genres.length > 0 ? (
            <select
              className="select select-compact"
              aria-label="Filter by genre"
              value={query.genre ?? ""}
              onChange={(event) => setQuery({ genre: event.target.value || null })}
            >
              <option value="">All genres</option>
              {genres.map((genre) => (
                <option key={genre} value={genre}>
                  {genre}
                </option>
              ))}
            </select>
          ) : null}

          {filteredView && formats.length > 0 ? (
            <select
              className="select select-compact"
              aria-label="Filter by format"
              value={query.format ?? ""}
              onChange={(event) => setQuery({ format: event.target.value || null })}
            >
              <option value="">All formats</option>
              {formats.map((format) => (
                <option key={format} value={format}>
                  {format.toUpperCase()}
                </option>
              ))}
            </select>
          ) : null}

          <span className="view-meta">
            {tracksLoading ? "reading…" : trackSummary(tracks)}
          </span>
        </div>
      </div>

      <div className="view-body">
        {query.search ? (
          <SearchResults />
        ) : tracksLoading ? (
          <div aria-hidden="true">
            {Array.from({ length: SKELETON_ROWS }, (_, index) => (
              <div key={index} className="skeleton-row" />
            ))}
          </div>
        ) : tracks.length === 0 ? (
          <div className="empty">
            <p className="empty-title">{empty.title}</p>
            <p className="empty-body">{empty.body}</p>
            {empty.withAddFolder ? (
              <div className="actions">
                <AddFolderButton />
              </div>
            ) : null}
          </div>
        ) : (
          <TrackTable tracks={tracks} />
        )}
      </div>
    </>
  );
}
