import { useEffect, useRef } from "react";

import { Chrome } from "./components/Chrome";
import { NowPanel } from "./components/NowPanel";
import { Rail } from "./components/Rail";
import { Toast } from "./components/Toast";
import { Transport } from "./components/Transport";
import { REPEAT_CYCLE } from "./lib/playback";
import { useStore, type View } from "./state/store";
import { BrowseView } from "./views/BrowseView";
import { EditorView } from "./views/EditorView";
import { EqView } from "./views/EqView";
import { ExtensionsView } from "./views/ExtensionsView";
import { FoldersView } from "./views/FoldersView";
import { PlaylistsView } from "./views/PlaylistsView";
import { SettingsView } from "./views/SettingsView";
import { TrackListView } from "./views/TrackListView";
import { LyricsView } from "./views/LyricsView";

const SEEK_STEP_MS = 5_000;
const COARSE_SEEK_MS = 30_000;

/** Every view has exactly one home; the switch is exhaustive so none can be forgotten. */
function MainView({ view }: { view: View }) {
  switch (view.kind) {
    case "library":
    case "favorites":
    case "recent":
    case "album":
    case "artist":
    case "playlist":
    case "folder":
      return <TrackListView />;
    case "albums":
    case "artists":
      return <BrowseView />;
    case "playlists":
      return <PlaylistsView />;
    case "folders":
      return <FoldersView />;
    case "eq":
      return <EqView />;
    case "settings":
      return <SettingsView />;
    case "extensions":
      return <ExtensionsView />;
    case "editor":
      return <EditorView />;
    case "lyrics-view":
      return <LyricsView />;
  }
}

/** Text fields own their keys; a global shortcut must not eat a typed "n". */
function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    target.isContentEditable
  );
}

export function App() {
  const {
    phase,
    failure,
    view,
    nowOpen,
    snapshot,
    currentTrack,
    shuffle,
    repeat,
    togglePlay,
    next,
    previous,
    seek,
    toggleMute,
    toggleFavorite,
    setShuffle,
    setRepeat,
    closeNow,
    setView,
  } = useStore();

  // The listener is attached once; the ref carries the current closure so the handler
  // always sees this render's state without re-binding on every poll.
  const shortcuts = useRef<(event: KeyboardEvent) => void>(() => {});

  shortcuts.current = (event) => {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return;
    if (isTypingTarget(event.target)) return;

    const step = event.shiftKey ? COARSE_SEEK_MS : SEEK_STEP_MS;

    switch (event.key) {
      case " ":
        event.preventDefault();
        void togglePlay();
        break;
      case "ArrowRight":
        event.preventDefault();
        seek(Math.min(snapshot.durationMs, snapshot.positionMs + step));
        break;
      case "ArrowLeft":
        event.preventDefault();
        seek(Math.max(0, snapshot.positionMs - step));
        break;
      case "n":
      case "N":
        void next();
        break;
      case "p":
      case "P":
        void previous();
        break;
      case "m":
      case "M":
        toggleMute();
        break;
      case "f":
      case "F":
        if (currentTrack) void toggleFavorite(currentTrack);
        break;
      case "s":
      case "S":
        setShuffle(!shuffle);
        break;
      case "r":
      case "R":
        setRepeat(REPEAT_CYCLE[repeat]);
        break;
      case "l":
      case "L":
        if (currentTrack) setView({ kind: "lyrics-view" });
        break;
      case "/":
        event.preventDefault();
        document.getElementById("library-search")?.focus();
        break;
      case "Escape":
        if (nowOpen) closeNow();
        break;
      default:
        break;
    }
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => shortcuts.current(event);
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="app" data-now-open={nowOpen}>
      <Chrome />
      <Rail />

      <main className="main">
        {phase === "failed" ? (
          <div className="empty">
            <p className="empty-title">The library could not be opened</p>
            <p className="empty-body">{failure ?? "The core did not report a reason."}</p>
          </div>
        ) : phase === "loading" ? (
          <div className="empty">
            <p className="empty-title">Opening the library</p>
            <p className="empty-body">Reading the index and the engine state.</p>
          </div>
        ) : (
          <MainView view={view} />
        )}
      </main>

      <NowPanel />

      <Transport />
      <Toast />
    </div>
  );
}
