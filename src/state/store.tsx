import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  type ReactNode,
} from "react";

import { backend } from "../lib/ipc";
import {
  emptyQuery,
  EQ_FREQUENCIES,
  type EngineSnapshot,
  type EqSettings,
  type LibraryFolder,
  type LibraryStats,
  type Lyrics,
  type OutputDevice,
  type Playlist,
  type RepeatMode,
  type ScanProgress,
  type Track,
  type TrackGroup,
  type TrackQuery,
} from "../lib/types";

export type ThemeName =
  | "carbon"
  | "paper"
  | "stage"
  | "apple"
  | "glass"
  | "catppuccin-mocha"
  | "sky-catppuccin"
  | "babypink-catppuccin";

export type View =
  | { kind: "home" }
  | { kind: "library" }
  | { kind: "favorites" }
  | { kind: "recent" }
  | { kind: "albums" }
  | { kind: "artists" }
  | { kind: "album"; name: string }
  | { kind: "artist"; name: string }
  | { kind: "playlists" }
  | { kind: "playlist"; id: number }
  | { kind: "folders" }
  | { kind: "folder"; path: string }
  | { kind: "eq" }
  | { kind: "settings" }
  | { kind: "extensions" }
  | { kind: "editor" }
  | { kind: "lyrics-view" }
  | { kind: "provider-home-feed"; id: string }
  | { kind: "provider-browse-categories"; id: string }
  | { kind: "resolved"; provider: string; entityType: string; item: any };

export type NowTab = "queue" | "lyrics" | "downloads";

export interface Toast {
  message: string;
  tone: "info" | "error";
}

interface State {
  phase: "loading" | "ready" | "failed";
  failure: string | null;
  view: View;
  query: TrackQuery;
  tracks: Track[];
  tracksLoading: boolean;
  groups: TrackGroup[];
  groupsLoading: boolean;
  stats: LibraryStats | null;
  playlists: Playlist[];
  folders: LibraryFolder[];
  devices: OutputDevice[];
  snapshot: EngineSnapshot;
  selectedId: number | null;
  queue: number[];
  shuffle: boolean;
  repeat: RepeatMode;
  eq: EqSettings;
  lyrics: Lyrics | null;
  lyricsLoading: boolean;
  nowTab: NowTab;
  nowOpen: boolean;
  theme: ThemeName;
  scan: ScanProgress | null;
  toast: Toast | null;
  known: Record<number, Track>;
}

const EMPTY_SNAPSHOT: EngineSnapshot = {  status: "idle",
  path: null,
  positionMs: 0,
  durationMs: 0,
  volume: 0.85,
  speed: 1,
  device: "",
  sampleRate: 48_000,
  level: 0,
  peak: 0,
  endedPath: null,
  error: null,
};

const INITIAL: State = {
  phase: "loading",
  failure: null,
  view: { kind: "home" },
  query: emptyQuery(),
  tracks: [],
  tracksLoading: true,
  groups: [],
  groupsLoading: false,
  stats: null,
  playlists: [],
  folders: [],
  devices: [],
  snapshot: EMPTY_SNAPSHOT,
  selectedId: null,
  queue: [],
  shuffle: false,
  repeat: "off",
  eq: { enabled: false, gains: EQ_FREQUENCIES.map(() => 0), preampDb: 0 },
  lyrics: null,
  lyricsLoading: false,
  nowTab: "queue",
  // Matches the stylesheet breakpoint: below it the panel floats over the library, so
  // opening it on first paint would hide the thing the window just opened.
  nowOpen: window.matchMedia("(min-width: 1181px)").matches,
  theme: "carbon",
  scan: null,
  toast: null,
  known: {},
};

type Action =
  | { type: "ready" }
  | { type: "failed"; message: string }
  | { type: "view"; view: View }
  | { type: "query"; patch: Partial<TrackQuery> }
  | { type: "tracks"; tracks: Track[]; loading: boolean }
  | { type: "tracks-loading" }
  | { type: "groups"; groups: TrackGroup[]; loading: boolean }
  | { type: "stats"; stats: LibraryStats }
  | { type: "playlists"; playlists: Playlist[] }
  | { type: "folders"; folders: LibraryFolder[] }
  | { type: "devices"; devices: OutputDevice[] }
  | { type: "snapshot"; snapshot: EngineSnapshot }
  | { type: "select"; id: number | null }
  | { type: "queue"; queue: number[] }
  | { type: "shuffle"; on: boolean }
  | { type: "repeat"; mode: RepeatMode }
  | { type: "eq"; eq: EqSettings }
  | { type: "lyrics"; lyrics: Lyrics | null; loading: boolean }
  | { type: "now-tab"; tab: NowTab }
  | { type: "now-open"; open: boolean }
  | { type: "theme"; theme: ThemeName }
  | { type: "scan"; scan: ScanProgress | null }
  | { type: "toast"; toast: Toast | null }
  | { type: "track-updated"; track: Track };

function remember(known: Record<number, Track>, tracks: Track[]): Record<number, Track> {
  if (tracks.length === 0) return known;
  const next = { ...known };
  for (const track of tracks) next[track.id] = track;
  return next;
}

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "ready":
      return { ...state, phase: "ready" };
    case "failed":
      return { ...state, phase: "failed", failure: action.message, tracksLoading: false };
    case "view":
      // Sort survives a view change; the filter does not, because a stale album
      // filter would silently empty the next list the listener opens.
      return {
        ...state,
        view: action.view,
        selectedId: null,
        query: { ...emptyQuery(), sort: state.query.sort, order: state.query.order },
        tracksLoading: true,
      };
    case "query":
      return { ...state, query: { ...state.query, ...action.patch }, tracksLoading: true };
    case "tracks":
      return {
        ...state,
        tracks: action.tracks,
        tracksLoading: action.loading,
        known: remember(state.known, action.tracks),
      };
    case "tracks-loading":
      return { ...state, tracksLoading: true };
    case "groups":
      return { ...state, groups: action.groups, groupsLoading: action.loading };
    case "stats":
      return { ...state, stats: action.stats };
    case "playlists":
      return { ...state, playlists: action.playlists };
    case "folders":
      return { ...state, folders: action.folders };
    case "devices":
      return { ...state, devices: action.devices };
    case "snapshot":
      return { ...state, snapshot: action.snapshot };
    case "select":
      return { ...state, selectedId: action.id };
    case "queue":
      return { ...state, queue: action.queue };
    case "shuffle":
      return { ...state, shuffle: action.on };
    case "repeat":
      return { ...state, repeat: action.mode };
    case "eq":
      return { ...state, eq: action.eq };
    case "lyrics":
      return { ...state, lyrics: action.lyrics, lyricsLoading: action.loading };
    case "now-tab":
      return { ...state, nowTab: action.tab };
    case "now-open":
      return { ...state, nowOpen: action.open };
    case "theme":
      return { ...state, theme: action.theme };
    case "scan":
      return { ...state, scan: action.scan };
    case "toast":
      return { ...state, toast: action.toast };
    case "track-updated":
      return {
        ...state,
        known: { ...state.known, [action.track.id]: action.track },
        tracks: state.tracks.map((track) =>
          track.id === action.track.id ? action.track : track,
        ),
      };
    default:
      return state;
  }
}

/** The next track to hand the engine, following the queue and repeat rules. */
export function pickNext(
  queue: number[],
  currentId: number | null,
  shuffle: boolean,
  repeat: RepeatMode,
  random: () => number = Math.random,
): number | null {
  if (queue.length === 0) return null;
  if (currentId === null) return queue[0] ?? null;
  if (repeat === "one") return currentId;

  if (shuffle) {
    const others = queue.filter((id) => id !== currentId);
    if (others.length === 0) return repeat === "all" ? currentId : null;
    return others[Math.floor(random() * others.length)] ?? null;
  }

  const index = queue.indexOf(currentId);
  if (index < 0) return queue[0] ?? null;
  const next = queue[index + 1];
  if (next !== undefined) return next;
  return repeat === "all" ? (queue[0] ?? null) : null;
}

interface StoreValue extends State {
  currentTrack: Track | null;
  currentId: number | null;
  selectedTrack: Track | null;
  setView: (view: View) => void;
  setQuery: (patch: Partial<TrackQuery>) => void;
  select: (id: number | null) => void;
  play: (track: Track, fromQueue?: number[]) => Promise<void>;
  togglePlay: () => Promise<void>;
  next: () => Promise<void>;
  previous: () => Promise<void>;
  stop: () => Promise<void>;
  seek: (positionMs: number) => void;
  setVolume: (volume: number) => void;
  toggleMute: () => void;
  setSpeed: (speed: number) => void;
  setShuffle: (on: boolean) => void;
  setRepeat: (mode: RepeatMode) => void;
  setEq: (eq: EqSettings) => void;
  setTheme: (theme: ThemeName) => void;
  openNow: (tab: NowTab) => void;
  closeNow: () => void;
  setNowTab: (tab: NowTab) => void;
  toggleFavorite: (track: Track) => Promise<void>;
  replaceTrack: (track: Track) => void;
  refresh: () => Promise<void>;
  addToPlaylist: (playlistId: number, trackIds: number[]) => Promise<void>;
  notify: (message: string, tone?: Toast["tone"]) => void;
  dismissToast: () => void;
  loadLyrics: (trackId: number) => Promise<void>;
}

const StoreContext = createContext<StoreValue | null>(null);

const POLL_MS = 120;
const TOAST_MS = 6000;

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, INITIAL);
  const stateRef = useRef(state);
  stateRef.current = state;

  /** Last path the engine told us was audible, so a play is recorded exactly once. */
  const reportedPath = useRef<string | null>(null);
  /** Last path handed to the engine as "next", so the queue is queued once. */
  const nextQueued = useRef<string | null>(null);
  /** Last playback state sent to the OS session, so the poll does not spam it. */
  const mediaSent = useRef<string>("");
  /** Volume to restore when the listener unmutes. */
  const lastVolume = useRef(0.85);

  const notify = useCallback((message: string, tone: Toast["tone"] = "info") => {
    dispatch({ type: "toast", toast: { message, tone } });
  }, []);

  /* ------------------------------------------------------------- startup --- */

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      try {
        const [stats, playlists, folders, devices, theme, volume, speed, eq] =
          await Promise.all([
            backend.libraryStats(),
            backend.listPlaylists(),
            backend.listFolders(),
            backend.outputDevices(),
            backend.getSetting("ui.theme"),
            backend.getSetting("player.volume"),
            backend.getSetting("player.speed"),
            backend.getSetting("player.eq"),
          ]);
        if (cancelled) return;

        dispatch({ type: "stats", stats });
        dispatch({ type: "playlists", playlists });
        dispatch({ type: "folders", folders });
        dispatch({ type: "devices", devices });
        const validThemes = ["carbon", "paper", "stage", "apple", "glass", "catppuccin-mocha", "sky-catppuccin", "babypink-catppuccin"];
        if (theme !== null && validThemes.includes(theme)) {
          dispatch({ type: "theme", theme: theme as ThemeName });
        }
        const restoredEq = parseJson<EqSettings>(eq);
        if (restoredEq) dispatch({ type: "eq", eq: normalizeEq(restoredEq) });

        // The shell owns writing `player.*` on exit; the window only reads it back so
        // the controls show what the engine actually restored.
        const restoredVolume = parseJson<number>(volume);
        const restoredSpeed = parseJson<number>(speed);
        if (typeof restoredVolume === "number") {
          dispatch({
            type: "snapshot",
            snapshot: { ...EMPTY_SNAPSHOT, volume: restoredVolume },
          });
        }
        if (typeof restoredSpeed === "number") {
          dispatch({
            type: "snapshot",
            snapshot: { ...EMPTY_SNAPSHOT, volume: restoredVolume ?? EMPTY_SNAPSHOT.volume, speed: restoredSpeed },
          });
        }
        dispatch({ type: "ready" });
      } catch (error) {
        if (!cancelled) {
          dispatch({ type: "failed", message: describe(error) });
        }
      }
    }

    void boot();
    return () => {
      cancelled = true;
    };
  }, []);

  /* --------------------------------------------------------- track loading --- */

  const loadTracks = useCallback(
    async (view: View, query: TrackQuery) => {
      try {
        if (view.kind === "recent") {
          const tracks = await backend.recentTracks(80);
          dispatch({ type: "tracks", tracks, loading: false });
          return;
        }
        if (view.kind === "playlist") {
          const tracks = await backend.playlistTracks(view.id);
          dispatch({ type: "tracks", tracks, loading: false });
          return;
        }
        if (view.kind === "album") {
          const tracks = await backend.listTracks({
            ...query,
            album: view.name,
            sort: "artist",
            limit: 0,
          });
          dispatch({ type: "tracks", tracks, loading: false });
          return;
        }
        if (view.kind === "artist") {
          const tracks = await backend.listTracks({
            ...query,
            artist: view.name,
            sort: "album",
            limit: 0,
          });
          dispatch({ type: "tracks", tracks, loading: false });
          return;
        }
        if (view.kind === "folder") {
          const tracks = await backend.listTracks({ ...query, search: view.path, limit: 0 });
          dispatch({ type: "tracks", tracks, loading: false });
          return;
        }
        if (
          view.kind === "albums" ||
          view.kind === "artists" ||
          view.kind === "playlists" ||
          view.kind === "folders" ||
          view.kind === "eq" ||
          view.kind === "settings" ||
          view.kind === "editor" ||
          view.kind === "provider-home-feed" ||
          view.kind === "provider-browse-categories" ||
          view.kind === "resolved"
        ) {
          dispatch({ type: "tracks", tracks: [], loading: false });
          return;
        }

        const tracks = await backend.listTracks({
          ...query,
          favoritesOnly: view.kind === "favorites",
          limit: 0,
        });
        dispatch({ type: "tracks", tracks, loading: false });
      } catch (error) {
        dispatch({ type: "tracks", tracks: [], loading: false });
        notify(describe(error), "error");
      }
    },
    [notify],
  );

  useEffect(() => {
    if (state.phase !== "ready") return;
    void loadTracks(state.view, state.query);
  }, [state.phase, state.view, state.query, loadTracks]);

  useEffect(() => {
    if (state.phase !== "ready") return;
    if (state.view.kind !== "albums" && state.view.kind !== "artists") return;
    const column = state.view.kind === "albums" ? "album" : "artist";
    let alive = true;
    dispatch({ type: "groups", groups: [], loading: true });
    backend
      .groupTracks(column)
      .then((groups) => {
        if (alive) dispatch({ type: "groups", groups, loading: false });
      })
      .catch((error) => {
        if (alive) dispatch({ type: "groups", groups: [], loading: false });
        notify(describe(error), "error");
      });
    return () => {
      alive = false;
    };
  }, [state.phase, state.view, notify]);

  /* ---------------------------------------------------------------- theme --- */

  useEffect(() => {
    document.documentElement.dataset.theme = state.theme;
  }, [state.theme]);

  /* --------------------------------------------------------------- lyrics --- */

  const loadLyricsFor = useCallback(async (trackId: number) => {
    dispatch({ type: "lyrics", lyrics: null, loading: true });
    try {
      const lyrics = await backend.lyricsFor(trackId);
      dispatch({ type: "lyrics", lyrics, loading: false });
    } catch {
      dispatch({ type: "lyrics", lyrics: null, loading: false });
    }
  }, []);

  /* --------------------------------------------------------------- actions --- */

  const playTrack = useCallback(
    async (track: Track, fromQueue?: number[]) => {
      const queue = fromQueue && fromQueue.length > 0 ? fromQueue : [track.id];
      dispatch({ type: "queue", queue });
      try {
        await backend.engineLoad(track.path, 0);
        await backend.enginePlay();
        await backend.reportPlay(track.id);
        reportedPath.current = track.path;
        void loadLyricsFor(track.id);
      } catch (error) {
        notify(describe(error), "error");
      }
    },
    [notify, loadLyricsFor],
  );

  const skip = useCallback(
    async (direction: 1 | -1) => {
      const { queue, known, shuffle, repeat, snapshot } = stateRef.current;
      if (queue.length === 0) return;
      const currentId = findIdByPath(known, snapshot.path);

      let targetId: number | null;
      if (direction === 1) {
        targetId = pickNext(queue, currentId, shuffle, repeat);
      } else {
        const index = currentId === null ? 0 : queue.indexOf(currentId);
        targetId = queue[Math.max(0, index - 1)] ?? queue[0] ?? null;
      }

      if (targetId === null) {
        await backend.engineStop();
        return;
      }
      const track = known[targetId];
      if (!track) return;
      try {
        await backend.engineLoad(track.path, 0);
        await backend.enginePlay();
        await backend.reportPlay(track.id);
        reportedPath.current = track.path;
        void loadLyricsFor(track.id);
      } catch (error) {
        notify(describe(error), "error");
      }
    },
    [loadLyricsFor, notify],
  );

  const skipNext = useCallback(() => skip(1), [skip]);
  const skipPrevious = useCallback(() => skip(-1), [skip]);

  const stop = useCallback(async () => {
    try {
      await backend.engineStop();
    } catch (error) {
      notify(describe(error), "error");
    }
    reportedPath.current = null;
    nextQueued.current = null;
    dispatch({ type: "queue", queue: [] });
  }, [notify]);

  /* ----------------------------------------------------------------- poll --- */

  const applySnapshot = useCallback(
    async (snapshot: EngineSnapshot) => {
      dispatch({ type: "snapshot", snapshot });

      if (snapshot.error) notify(snapshot.error, "error");

      // A new file is audible: record the play once, then look up the queue rules and
      // hand the engine the following file so playback never waits between tracks.
      if (snapshot.path && snapshot.path !== reportedPath.current) {
        reportedPath.current = snapshot.path;
        const { known } = stateRef.current;
        const trackId = findIdByPath(known, snapshot.path);
        if (trackId !== null) {
          void backend.reportPlay(trackId).catch(() => {});
          if (known[trackId]?.hasLyrics) void loadLyricsFor(trackId);
        }
      }

      const { queue, shuffle, repeat, known } = stateRef.current;
      const currentId = findIdByPath(known, snapshot.path);
      const nextId = pickNext(queue, currentId, shuffle, repeat);
      const nextPath = nextId === null ? null : (known[nextId]?.path ?? null);
      if (nextPath !== nextQueued.current) {
        nextQueued.current = nextPath;
        void backend.engineSetNext(nextPath).catch(() => {});
      }

      const mediaKey = `${snapshot.status}:${Math.floor(snapshot.positionMs / 2000)}`;
      if (mediaKey !== mediaSent.current) {
        mediaSent.current = mediaKey;
        void backend
          .reportPlayback(snapshot.status === "playing", snapshot.positionMs)
          .catch(() => {});
      }
    },
    [notify, loadLyricsFor],
  );

  useEffect(() => {
    if (state.phase !== "ready") return;
    let alive = true;
    const tick = async () => {
      try {
        const snapshot = await backend.engineSnapshot();
        if (alive) await applySnapshot(snapshot);
      } catch {
        /* a poll that fails once is not worth shouting at the listener */
      }
    };
    void tick();
    const timer = window.setInterval(tick, POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [state.phase, applySnapshot]);

  /* --------------------------------------------------------- scan + media --- */

  useEffect(() => {
    if (state.phase !== "ready") return;
    let stopListening: (() => void) | null = null;
    void backend
      .onScan({
        progress: (progress) => dispatch({ type: "scan", scan: progress }),
        done: (progress) => {
          dispatch({ type: "scan", scan: null });
          notify(
            `Scan finished: ${progress.tracksAdded} added, ${progress.tracksUpdated} updated`,
          );
          void backend.libraryStats().then((stats) => dispatch({ type: "stats", stats }));
          void backend.listFolders().then((folders) => dispatch({ type: "folders", folders }));
          void loadTracks(stateRef.current.view, stateRef.current.query);
        },
        error: (message) => {
          dispatch({ type: "scan", scan: null });
          notify(message, "error");
        },
      })
      .then((unlisten) => {
        stopListening = unlisten;
      })
      .catch(() => {});
    return () => {
      stopListening?.();
    };
  }, [state.phase, notify, loadTracks]);

  useEffect(() => {
    if (state.phase !== "ready") return;
    let stopListening: (() => void) | null = null;

    const handle = async (action: string) => {
      if (action === "play") await backend.enginePlay();
      else if (action === "pause") await backend.enginePause();
      else if (action === "toggle") {
        if (stateRef.current.snapshot.status === "playing") await backend.enginePause();
        else await backend.enginePlay();
      } else if (action === "next") await skipNext();
      else if (action === "previous") await skipPrevious();
      else if (action === "stop") await stop();
    };

    void backend
      .onMediaControl((action) => {
        void handle(action).catch((error) => notify(describe(error), "error"));
      })
      .then((unlisten) => {
        stopListening = unlisten;
      })
      .catch(() => {});

    return () => {
      stopListening?.();
    };
  }, [state.phase, notify, skipNext, skipPrevious, stop]);

  /* ----------------------------------------------------------------- toast --- */

  useEffect(() => {
    if (!state.toast) return;
    const timer = window.setTimeout(() => dispatch({ type: "toast", toast: null }), TOAST_MS);
    return () => window.clearTimeout(timer);
  }, [state.toast]);

  /* ----------------------------------------------------------------- value --- */

  // Stable dispatchers: the poll re-renders the provider every 120 ms, and a fresh
  // function identity would restart every effect that lists one of these as a dependency
  // — the debounced search would never survive long enough to fire.
  const setView = useCallback((view: View) => dispatch({ type: "view", view }), []);
  const setQuery = useCallback(
    (patch: Partial<TrackQuery>) => dispatch({ type: "query", patch }),
    [],
  );
  const select = useCallback((id: number | null) => dispatch({ type: "select", id }), []);

  const value = useMemo<StoreValue>(() => {
    const currentId = findIdByPath(state.known, state.snapshot.path);
    const currentTrack = currentId === null ? null : (state.known[currentId] ?? null);
    const selectedTrack =
      state.selectedId === null ? null : (state.known[state.selectedId] ?? null);

    return {
      ...state,
      currentId,
      currentTrack,
      selectedTrack,
      setView,
      setQuery,
      select,
      play: playTrack,
      togglePlay: async () => {
        try {
          if (state.snapshot.status === "playing") await backend.enginePause();
          else if (state.snapshot.status === "paused") await backend.enginePlay();
          else if (currentTrack) await playTrack(currentTrack, state.queue);
        } catch (error) {
          notify(describe(error), "error");
        }
      },
      next: skipNext,
      previous: skipPrevious,
      stop,
      seek: (positionMs) => {
        void backend.engineSeek(positionMs).catch((error) => notify(describe(error), "error"));
      },
      setVolume: (volume) => {
        void backend.engineSetVolume(volume).catch(() => {});
      },
      toggleMute: () => {
        const current = stateRef.current.snapshot.volume;
        if (current > 0.001) {
          lastVolume.current = current;
          void backend.engineSetVolume(0).catch(() => {});
        } else {
          void backend.engineSetVolume(lastVolume.current).catch(() => {});
        }
      },
      setSpeed: (speed) => {
        void backend.engineSetSpeed(speed).catch(() => {});
      },
      setShuffle: (on) => {
        dispatch({ type: "shuffle", on });
        void backend.setSetting("ui.shuffle", JSON.stringify(on)).catch(() => {});
      },
      setRepeat: (mode) => dispatch({ type: "repeat", mode }),
      setEq: (eq) => {
        dispatch({ type: "eq", eq });
        void backend.engineSetEq(eq).catch(() => {});
      },
      setTheme: (theme) => {
        dispatch({ type: "theme", theme });
        void backend.setSetting("ui.theme", theme).catch(() => {});
      },
      openNow: (tab) => {
        dispatch({ type: "now-tab", tab });
        dispatch({ type: "now-open", open: true });
      },
      closeNow: () => dispatch({ type: "now-open", open: false }),
      setNowTab: (tab) => dispatch({ type: "now-tab", tab }),
      toggleFavorite: async (track) => {
        const next = !track.favorite;
        try {
          await backend.setFavorite(track.id, next);
          dispatch({ type: "track-updated", track: { ...track, favorite: next } });
        } catch (error) {
          notify(describe(error), "error");
        }
      },
      replaceTrack: (track) => dispatch({ type: "track-updated", track }),
      refresh: async () => {
        dispatch({ type: "tracks-loading" });
        try {
          const [stats, folders, playlists] = await Promise.all([
            backend.libraryStats(),
            backend.listFolders(),
            backend.listPlaylists(),
          ]);
          dispatch({ type: "stats", stats });
          dispatch({ type: "folders", folders });
          dispatch({ type: "playlists", playlists });
        } catch (error) {
          notify(describe(error), "error");
        }
        await loadTracks(stateRef.current.view, stateRef.current.query);
      },
      addToPlaylist: async (playlistId, trackIds) => {
        try {
          await backend.addToPlaylist(playlistId, trackIds);
          const playlists = await backend.listPlaylists();
          dispatch({ type: "playlists", playlists });
          dispatch({
            type: "toast",
            toast: { message: `Added ${trackIds.length} to the playlist`, tone: "info" },
          });
        } catch (error) {
          notify(describe(error), "error");
        }
      },
      notify,
      dismissToast: () => dispatch({ type: "toast", toast: null }),
      loadLyrics: loadLyricsFor,
    };
  }, [
    state,
    playTrack,
    skipNext,
    skipPrevious,
    stop,
    notify,
    loadLyricsFor,
    loadTracks,
    setView,
    setQuery,
    select,
  ]);

  return <StoreContext.Provider value={value}>{children}</StoreContext.Provider>;
}

export function useStore(): StoreValue {
  const context = useContext(StoreContext);
  if (!context) throw new Error("useStore must be used inside StoreProvider");
  return context;
}

function findIdByPath(known: Record<number, Track>, path: string | null): number | null {
  if (!path) return null;
  for (const track of Object.values(known)) {
    if (track.path === path) return track.id;
  }
  return null;
}

export function normalizeEq(eq: EqSettings): EqSettings {
  return {
    enabled: eq.enabled,
    gains: EQ_FREQUENCIES.map((_, index) =>
      Math.max(-12, Math.min(12, eq.gains[index] ?? 0)),
    ),
    preampDb: Math.max(-12, Math.min(12, eq.preampDb ?? 0)),
  };
}

function parseJson<T>(value: string | null): T | null {
  if (!value) return null;
  try {
    return JSON.parse(value) as T;
  } catch {
    return null;
  }
}

export function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Something went wrong.";
}
