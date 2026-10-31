import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { demoBackend } from "./demo";
import { tauriExtensions, type ExtensionSurface } from "./extensions";
import type {
  EngineSnapshot,
  EqSettings,
  LibraryFolder,
  LibraryStats,
  Lyrics,
  OutputDevice,
  Playlist,
  ScanProgress,
  SmartRules,
  TagEdit,
  Track,
  TrackGroup,
  TrackQuery,
} from "./types";
import type { ProviderLyrics, ProviderSearchOutcome } from "./extensions";

/** Column names the core accepts for `distinct_values`. */
export type DistinctColumn = "artist" | "album" | "genre" | "format" | "album_artist";

export interface ScanHandlers {
  progress: (progress: ScanProgress) => void;
  done: (progress: ScanProgress) => void;
  error: (message: string) => void;
}

/**
 * Everything the window can ask the core to do. Two implementations exist: the real
 * Tauri commands, and a synthetic library used when the UI runs in a plain browser.
 * Keeping one interface means a component can never depend on which one is loaded.
 */
export interface Backend extends ExtensionSurface {
  readonly kind: "tauri" | "demo";

  libraryStats(): Promise<LibraryStats>;
  listTracks(query: TrackQuery): Promise<Track[]>;
  countTracks(query: TrackQuery): Promise<number>;
  distinctValues(column: DistinctColumn): Promise<string[]>;
  groupTracks(column: "album" | "artist"): Promise<TrackGroup[]>;
  getTrack(trackId: number): Promise<Track | null>;
  setFavorite(trackId: number, favorite: boolean): Promise<void>;
  recentTracks(limit: number): Promise<Track[]>;
  recentlyAdded(limit: number): Promise<Track[]>;
  listFolders(): Promise<LibraryFolder[]>;
  scanFolder(path: string): Promise<void>;
  rescanLibrary(): Promise<void>;
  removeFolder(folderId: number): Promise<void>;
  artwork(artId: string): Promise<string | null>;
  updateTags(path: string, edit: TagEdit): Promise<Track>;

  lyricsFor(trackId: number): Promise<Lyrics>;
  saveLyrics(trackId: number, content: string): Promise<Lyrics>;
  importLyrics(trackId: number): Promise<Lyrics | null>;
  exportLyrics(trackId: number, suggestedName: string): Promise<string | null>;
  estimateLyrics(trackId: number): Promise<Lyrics>;

  searchProviders(query: string, limit: number): Promise<ProviderSearchOutcome[]>;
  providerLyrics(trackId: number): Promise<ProviderLyrics | null>;

  listPlaylists(): Promise<Playlist[]>;
  createPlaylist(name: string, kind: "manual" | "smart", rules?: SmartRules): Promise<number>;
  renamePlaylist(playlistId: number, name: string): Promise<void>;
  deletePlaylist(playlistId: number): Promise<void>;
  playlistTracks(playlistId: number): Promise<Track[]>;
  updateSmartRules(playlistId: number, rules: SmartRules): Promise<void>;
  addToPlaylist(playlistId: number, trackIds: number[]): Promise<void>;
  removeFromPlaylist(playlistId: number, position: number): Promise<void>;
  movePlaylistItem(playlistId: number, from: number, to: number): Promise<void>;

  getSetting(key: string): Promise<string | null>;
  setSetting(key: string, value: string): Promise<void>;

  engineSnapshot(): Promise<EngineSnapshot>;
  engineLoad(path: string, startMs: number): Promise<void>;
  enginePlay(): Promise<void>;
  enginePause(): Promise<void>;
  engineStop(): Promise<void>;
  engineSeek(positionMs: number): Promise<void>;
  engineSetVolume(volume: number): Promise<void>;
  engineSetSpeed(speed: number): Promise<void>;
  engineSetEq(eq: EqSettings): Promise<void>;
  engineSetNext(path: string | null): Promise<void>;
  outputDevices(): Promise<OutputDevice[]>;
  setOutputDevice(name: string): Promise<void>;
  reportPlay(trackId: number): Promise<void>;
  reportPlayback(playing: boolean, positionMs: number): Promise<void>;

  pickFolder(): Promise<string | null>;
  pickFiles(): Promise<string[]>;
  revealTrack(path: string): Promise<void>;

  onScan(handlers: ScanHandlers): Promise<UnlistenFn>;
  onMediaControl(handler: (action: string) => void): Promise<UnlistenFn>;
}

export const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const tauriBackend: Backend = {
  kind: "tauri",

  ...tauriExtensions,

  libraryStats: () => invoke("library_stats"),
  listTracks: (query) => invoke("list_tracks", { query }),
  countTracks: (query) => invoke("count_tracks", { query }),
  distinctValues: (column) => invoke("distinct_values", { column }),
  groupTracks: (column) => invoke("group_tracks", { column }),
  getTrack: (trackId) => invoke("get_track", { trackId }),
  setFavorite: (trackId, favorite) => invoke("set_favorite", { trackId, favorite }),
  recentTracks: (limit) => invoke("recent_tracks", { limit }),
  recentlyAdded: (limit) => invoke("recently_added", { limit }),
  listFolders: () => invoke("list_folders"),
  scanFolder: (path) => invoke("scan_folder", { path }),
  rescanLibrary: () => invoke("rescan_library"),
  removeFolder: (folderId) => invoke("remove_folder", { folderId }),
  artwork: (artId) => invoke("artwork", { artId }),
  updateTags: (path, edit) => invoke("update_tags", { path, edit }),

  lyricsFor: (trackId) => invoke("lyrics_for", { trackId }),
  saveLyrics: (trackId, content) => invoke("save_lyrics", { trackId, content }),
  importLyrics: (trackId) => invoke("import_lyrics", { trackId }),
  exportLyrics: (trackId, suggestedName) =>
    invoke("export_lyrics", { trackId, suggestedName }),
  estimateLyrics: (trackId) => invoke("estimate_lyrics", { trackId }),

  searchProviders: (query, limit) => invoke("search_providers", { query, limit }),
  providerLyrics: (trackId) => invoke("provider_lyrics", { trackId }),

  listPlaylists: () => invoke("list_playlists"),
  createPlaylist: (name, kind, rules) => invoke("create_playlist", { name, kind, rules }),
  renamePlaylist: (playlistId, name) => invoke("rename_playlist", { playlistId, name }),
  deletePlaylist: (playlistId) => invoke("delete_playlist", { playlistId }),
  playlistTracks: (playlistId) => invoke("playlist_tracks", { playlistId }),
  updateSmartRules: (playlistId, rules) =>
    invoke("update_smart_rules", { playlistId, rules }),
  addToPlaylist: (playlistId, trackIds) =>
    invoke("add_to_playlist", { playlistId, trackIds }),
  removeFromPlaylist: (playlistId, position) =>
    invoke("remove_from_playlist", { playlistId, position }),
  movePlaylistItem: (playlistId, from, to) =>
    invoke("move_playlist_item", { playlistId, from, to }),

  getSetting: (key) => invoke("get_setting", { key }),
  setSetting: (key, value) => invoke("set_setting", { key, value }),

  engineSnapshot: () => invoke("engine_snapshot"),
  engineLoad: (path, startMs) => invoke("engine_load", { path, startMs }),
  enginePlay: () => invoke("engine_play"),
  enginePause: () => invoke("engine_pause"),
  engineStop: () => invoke("engine_stop"),
  engineSeek: (positionMs) => invoke("engine_seek", { positionMs }),
  engineSetVolume: (volume) => invoke("engine_set_volume", { volume }),
  engineSetSpeed: (speed) => invoke("engine_set_speed", { speed }),
  engineSetEq: (eq) => invoke("engine_set_eq", { eq }),
  engineSetNext: (path) => invoke("engine_set_next", { path }),
  outputDevices: () => invoke("output_devices"),
  setOutputDevice: (name) => invoke("set_output_device", { name }),
  reportPlay: (trackId) => invoke("report_play", { trackId }),
  reportPlayback: (playing, positionMs) =>
    invoke("report_playback", { playing, positionMs }),

  pickFolder: () => invoke("pick_folder"),
  pickFiles: () => invoke("pick_files"),
  revealTrack: (path) => invoke("reveal_track", { path }),

  async onScan({ progress, done, error }) {
    const unlisten = await Promise.all([
      listen<ScanProgress>("library://scan", (event) => progress(event.payload)),
      listen<ScanProgress>("library://scan-done", (event) => done(event.payload)),
      listen<ScanProgress>("library://scan-complete", () =>
        done({ ...emptyScan(), scanning: false }),
      ),
      listen<string>("library://scan-error", (event) => error(event.payload)),
    ]);
    return () => unlisten.forEach((stop) => stop());
  },

  async onMediaControl(handler) {
    return listen<string>("media://control", (event) => handler(event.payload));
  },
};

function emptyScan(): ScanProgress {
  return {
    scanning: true,
    folder: "",
    filesSeen: 0,
    tracksAdded: 0,
    tracksUpdated: 0,
    tracksRemoved: 0,
    failures: [],
    finishedAt: null,
  };
}

/** The core when the window is a real Tauri app, the synthetic library otherwise. */
export const backend: Backend = IS_TAURI ? tauriBackend : demoBackend;
