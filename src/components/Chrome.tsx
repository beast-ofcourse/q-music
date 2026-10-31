import { useEffect, useRef, useState } from "react";

import { Icon } from "./Icon";
import { plural } from "../lib/format";
import { useStore } from "../state/store";

/** Long enough that a fast typist fires one core query, short enough to feel live. */
const SEARCH_DEBOUNCE_MS = 220;

export function Chrome() {
  const {
    query,
    setQuery,
    tracks,
    stats,
    tracksLoading,
    view,
    nowOpen,
    openNow,
    closeNow,
  } = useStore();
  const [text, setText] = useState(query.search ?? "");
  const inputRef = useRef<HTMLInputElement>(null);

  // Typing is local until it settles; the store stays the truth for the filter so a
  // view change or Escape clears the box as well as the query.
  useEffect(() => {
    const current = query.search ?? "";
    setText((previous) => (previous === current ? previous : current));
  }, [query.search]);

  useEffect(() => {
    const target = text.trim().length === 0 ? null : text.trim();
    if (target === query.search) return;
    const timer = window.setTimeout(() => setQuery({ search: target }), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [text, query.search, setQuery]);

  const total = stats?.tracks ?? 0;
  // Only the list views load `tracks`; elsewhere the honest number is the library size,
  // not the empty result set that view happens to hold.
  const listView =
    view.kind === "library" ||
    view.kind === "favorites" ||
    view.kind === "recent" ||
    view.kind === "album" ||
    view.kind === "artist" ||
    view.kind === "playlist" ||
    view.kind === "folder";

  const readout = tracksLoading
    ? "reading…"
    : !listView
      ? `${total.toLocaleString()} ${plural(total, "track")} in the library`
      : query.search
        ? `${tracks.length.toLocaleString()} of ${total.toLocaleString()}`
        : `${tracks.length.toLocaleString()} ${plural(tracks.length, "track")}`;

  return (
    <header className="chrome">
      <div className="brand">
        <span className="brand-mark">Q-Music</span>
        <span className="brand-sub">quantum</span>
      </div>

      <div className="search">
        <Icon name="search" size={14} />
        <input
          id="library-search"
          ref={inputRef}
          type="search"
          value={text}
          placeholder="Search title, artist, album, genre"
          aria-label="Search the library"
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              setText("");
              setQuery({ search: null });
            }
          }}
        />
        {text.length > 0 ? (
          <button
            type="button"
            className="search-clear"
            aria-label="Clear the search"
            onClick={() => {
              setText("");
              setQuery({ search: null });
              inputRef.current?.focus();
            }}
          >
            <Icon name="close" size={12} />
          </button>
        ) : null}
      </div>

      <span className="count-readout" aria-live="polite">
        {readout}
      </span>

      <span className="chrome-spacer" />

      <button
        type="button"
        className="now-toggle"
        aria-label={nowOpen ? "Hide the now-playing panel" : "Show the now-playing panel"}
        aria-expanded={nowOpen}
        onClick={() => (nowOpen ? closeNow() : openNow("queue"))}
      >
        <Icon name="queue" size={16} />
      </button>
    </header>
  );
}
