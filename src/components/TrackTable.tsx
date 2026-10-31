import { useState } from "react";

import { Icon } from "./Icon";
import { RowMenu } from "./RowMenu";
import { formatTime, techCompact } from "../lib/format";
import type { SortField, Track } from "../lib/types";
import { useStore } from "../state/store";

interface Column {
  field: SortField;
  label: string;
  /** Grid/hiding class shared by the header cell and the row cell. */
  className?: string;
}

const COLUMNS: Column[] = [
  { field: "title", label: "Title" },
  { field: "artist", label: "Artist" },
  { field: "album", label: "Album", className: "col-album" },
  { field: "format", label: "Format", className: "col-tech" },
  { field: "duration", label: "Time", className: "col-time" },
];

interface TrackTableProps {
  tracks: Track[];
}

interface OpenMenu {
  track: Track;
  x: number;
  y: number;
}

/**
 * The library table. Sorting lives in the header, selection paints the travelling
 * vermilion bar, and the play button in each row is the primary action.
 */
export function TrackTable({ tracks }: TrackTableProps) {
  const { query, setQuery, currentId, selectedId, select, play, toggleFavorite } = useStore();
  const [menu, setMenu] = useState<OpenMenu | null>(null);

  function closeMenu() {
    const openedFor = menu?.track.id ?? null;
    setMenu(null);
    // The row is what the listener was on; hand the focus back to it.
    if (openedFor !== null) {
      window.requestAnimationFrame(() => {
        document
          .querySelector<HTMLElement>(`.track-row[data-track-id="${openedFor}"]`)
          ?.focus();
      });
    }
  }

  const playingIndex = currentId === null ? -1 : tracks.findIndex((t) => t.id === currentId);
  const selectedIndex = selectedId === null ? -1 : tracks.findIndex((t) => t.id === selectedId);
  const markerIndex = playingIndex >= 0 ? playingIndex : selectedIndex;

  const ids = tracks.map((track) => track.id);

  function sortBy(field: SortField) {
    if (query.sort === field) {
      setQuery({ order: query.order === "asc" ? "desc" : "asc" });
      return;
    }
    setQuery({ sort: field, order: "asc" });
  }

  return (
    <div role="table" aria-label="Tracks" aria-rowcount={tracks.length}>
      <div className="table-head" role="row">
        <span role="columnheader">
          <span className="sr-only">Play</span>
        </span>
        {COLUMNS.map((column) => {
          const sorted = query.sort === column.field;
          return (
            <span
              key={column.field}
              role="columnheader"
              aria-sort={sorted ? (query.order === "asc" ? "ascending" : "descending") : "none"}
              className={[column.className, column.field === "duration" ? "col-num" : ""]
                .filter(Boolean)
                .join(" ")}
            >
              <button
                type="button"
                className={sorted ? "is-sorted" : undefined}
                onClick={() => sortBy(column.field)}
              >
                {column.label}
                {sorted ? (
                  <Icon name={query.order === "asc" ? "arrow-up" : "arrow-down"} size={10} />
                ) : null}
              </button>
            </span>
          );
        })}
      </div>

      <div className="table-body" role="rowgroup">
        <span
          className="row-marker"
          style={{
            // The travel distance is the row height the stylesheet already owns.
            transform: `translateY(calc(var(--row-h) * ${Math.max(0, markerIndex)}))`,
          }}
          aria-hidden="true"
          data-visible={markerIndex >= 0}
        />

        {tracks.map((track, index) => {
          const playing = track.id === currentId;
          const audio = track.sampleRate ?? 0;
          return (
            <div
              key={track.id}
              role="row"
              tabIndex={0}
              className="track-row"
              data-track-id={track.id}
              data-playing={playing}
              data-selected={track.id === selectedId}
              data-missing={audio === 0}
              onClick={() => select(track.id)}
              onDoubleClick={() => void play(track, ids)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void play(track, ids);
                }
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                select(track.id);
                const rect = event.currentTarget.getBoundingClientRect();
                // Keyboard-invoked menus report 0,0; anchor them to the row instead.
                const keyboard = event.clientX === 0 && event.clientY === 0;
                setMenu({
                  track,
                  x: keyboard ? rect.left + 28 : event.clientX,
                  y: keyboard ? rect.bottom : event.clientY,
                });
              }}
            >
              <span role="cell">
                <button
                  type="button"
                  className="row-play"
                  data-playing={playing}
                  aria-label={`Play ${track.title}`}
                  title={playing ? "Playing" : "Play"}
                  onClick={(event) => {
                    event.stopPropagation();
                    void play(track, ids);
                  }}
                >
                  <Icon name={playing ? "pause" : "play"} size={14} />
                </button>
              </span>

              <span role="cell" className="cell-title">
                {track.title || "Untitled"}
                {track.favorite ? <Icon name="heart" size={11} className="fav-mark" /> : null}
              </span>

              <span role="cell" className="cell-text">
                {track.artist || "—"}
              </span>

              <span role="cell" className="cell-text col-album">
                {track.album || "—"}
              </span>

              <span role="cell" className="cell-tech col-tech">
                {techCompact(track)}
              </span>

              <span role="cell" className="cell-num col-time">
                <span>{formatTime(track.durationMs)}</span>
                <button
                  type="button"
                  className="row-fav"
                  aria-pressed={track.favorite}
                  aria-label={
                    track.favorite ? `Remove ${track.title} from favourites` : `Favourite ${track.title}`
                  }
                  onClick={(event) => {
                    event.stopPropagation();
                    void toggleFavorite(track);
                  }}
                >
                  <Icon
                    name="heart"
                    size={12}
                    className={track.favorite ? "fav-mark" : undefined}
                  />
                </button>
              </span>

              <span className="sr-only">
                Row {index + 1} of {tracks.length}
              </span>
            </div>
          );
        })}
      </div>

      {menu ? (
        <RowMenu track={menu.track} x={menu.x} y={menu.y} onClose={closeMenu} />
      ) : null}
    </div>
  );
}
