import type { Backend } from "./ipc";
import { demoExtensions } from "./extensions";
import { parseLrc } from "./lrc";
import {
  EQ_FREQUENCIES,
  type EngineSnapshot,
  type EqSettings,
  type LibraryFolder,
  type LibraryStats,
  type Lyrics,
  type LyricLine,
  type OutputDevice,
  type Playlist,
  type TagEdit,
  type Track,
  type TrackGroup,
  type TrackQuery,
} from "./types";

/**
 * A synthetic library so the window can be inspected, clicked and screenshot in a
 * plain browser with no Rust process attached. Nothing here is real: the rail carries
 * a permanent "synthetic library" flag whenever this backend is active, and the
 * numbers are generated, never measured.
 */

interface AlbumSeed {
  artist: string;
  album: string;
  year: number;
  genre: string;
  format: string;
  bitDepth: number | null;
  sampleRate: number | null;
  bitrateKbps: number | null;
  titles: string[];
}

const ALBUM_SEEDS: AlbumSeed[] = [
  {
    artist: "Halden Roe",
    album: "Blue Interval",
    year: 2019,
    genre: "Ambient",
    format: "flac",
    bitDepth: 24,
    sampleRate: 96000,
    bitrateKbps: null,
    titles: ["Slate", "Blue Interval", "Windover", "Tin Roof", "Slow Return"],
  },
  {
    artist: "Halden Roe",
    album: "Copper Hours",
    year: 2021,
    genre: "Ambient",
    format: "flac",
    bitDepth: 24,
    sampleRate: 48000,
    bitrateKbps: null,
    titles: ["Copper Hours", "Ledger", "Palisade", "Night Shift", "Afterglow"],
  },
  {
    artist: "The Vellum Set",
    album: "Paper Radio",
    year: 2017,
    genre: "Indie",
    format: "wav",
    bitDepth: 16,
    sampleRate: 44100,
    bitrateKbps: null,
    titles: ["Paper Radio", "Second Signal", "June Cut", "White Ferry", "Out of Range"],
  },
  {
    artist: "The Vellum Set",
    album: "Long Division",
    year: 2020,
    genre: "Indie",
    format: "flac",
    bitDepth: 16,
    sampleRate: 44100,
    bitrateKbps: null,
    titles: ["Long Division", "Half Light", "Sparrow", "Concrete Garden", "Last Call"],
  },
  {
    artist: "Mara Okonjo",
    album: "Field Notes",
    year: 2022,
    genre: "Jazz",
    format: "alac",
    bitDepth: 24,
    sampleRate: 96000,
    bitrateKbps: null,
    titles: ["Field Notes", "Brasswork", "Low Tide", "Nylon", "Nightjar"],
  },
  {
    artist: "Mara Okonjo",
    album: "Quiet Machinery",
    year: 2024,
    genre: "Jazz",
    format: "flac",
    bitDepth: 24,
    sampleRate: 192000,
    bitrateKbps: null,
    titles: ["Quiet Machinery", "Flywheel", "Blue Line", "Copper Wire", "Hush"],
  },
  {
    artist: "Dunmore Union",
    album: "Live at the Foundry",
    year: 2016,
    genre: "Rock",
    format: "flac",
    bitDepth: 16,
    sampleRate: 44100,
    bitrateKbps: null,
    titles: ["Foundry", "Hammer and Tongs", "Ash", "Night Bus", "Encore"],
  },
  {
    artist: "Dunmore Union",
    album: "Second Shift",
    year: 2018,
    genre: "Rock",
    format: "mp3",
    bitDepth: null,
    sampleRate: 44100,
    bitrateKbps: 320,
    titles: ["Second Shift", "Press Gang", "Rust", "Grey Water", "Late Pay"],
  },
  {
    artist: "Isla Fenn",
    album: "Small Hours",
    year: 2023,
    genre: "Pop",
    format: "m4a",
    bitDepth: null,
    sampleRate: 44100,
    bitrateKbps: 256,
    titles: ["Small Hours", "Telephone", "Almost June", "Handwriting", "Daylight"],
  },
  {
    artist: "Kestrel Nine",
    album: "Nocturne Cycle",
    year: 2020,
    genre: "Electronic",
    format: "opus",
    bitDepth: null,
    sampleRate: 48000,
    bitrateKbps: 192,
    titles: ["Nocturne I", "Nocturne II", "Carrier", "Drift", "Return to Zero"],
  },
  {
    artist: "Kestrel Nine",
    album: "Signal Path",
    year: 2025,
    genre: "Electronic",
    format: "flac",
    bitDepth: 24,
    sampleRate: 96000,
    bitrateKbps: null,
    titles: ["Signal Path", "Relay", "Phase", "Beacon", "Shutdown"],
  },
  {
    artist: "Ruben Saez Trio",
    album: "Tres Voces",
    year: 2015,
    genre: "Latin Jazz",
    format: "flac",
    bitDepth: 16,
    sampleRate: 44100,
    bitrateKbps: null,
    titles: ["Tres Voces", "Cauce", "Madrugada", "Farol", "Vuelta"],
  },
  {
    artist: "Margit Voss",
    album: "Aeolian",
    year: 2021,
    genre: "Classical",
    format: "aiff",
    bitDepth: 24,
    sampleRate: 96000,
    bitrateKbps: null,
    titles: ["Aeolian", "Partita", "Nocturne for Left Hand", "Cradle", "Postlude"],
  },
];

const ROOT = "C:\\Music";
const EPOCH_NOW = Math.floor(Date.UTC(2026, 9, 20, 9, 0, 0) / 1000);

/** Deterministic 32-bit hash so the synthetic library is identical on every load. */
function hash(text: string): number {
  let value = 2_166_136_261;
  for (let index = 0; index < text.length; index += 1) {
    value ^= text.charCodeAt(index);
    value = Math.imul(value, 16_777_619);
  }
  return value >>> 0;
}

function mulberry32(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
  };
}

function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
}

const TRACKS: Track[] = [];
let nextTrackId = 1;

for (const seed of ALBUM_SEEDS) {
  seed.titles.forEach((title, position) => {
    const random = mulberry32(hash(`${seed.artist}:${seed.album}:${title}`));
    const durationMs = 132_000 + Math.floor(random() * 208_000);
    const bitrate = seed.bitrateKbps ?? (seed.bitDepth ?? 16) * 2 * (seed.sampleRate ?? 44_100) / 1000;
    const sizeBytes = Math.round((durationMs / 1000) * ((bitrate * 1000) / 8));
    const addedAt = EPOCH_NOW - Math.floor(random() * 720 * 86_400);
    const playCount = Math.floor(random() * 34);
    const trackNo = position + 1;

    TRACKS.push({
      id: nextTrackId,
      path: `${ROOT}\\${seed.artist}\\${seed.album}\\${String(trackNo).padStart(2, "0")} ${title}.${seed.format}`,
      title,
      artist: seed.artist,
      albumArtist: seed.artist,
      album: seed.album,
      genre: seed.genre,
      year: seed.year,
      trackNo,
      discNo: 1,
      durationMs,
      sampleRate: seed.sampleRate,
      channels: 2,
      bitDepth: seed.bitDepth,
      bitrateKbps: seed.bitrateKbps,
      format: seed.format,
      sizeBytes,
      mtime: addedAt + 4_000,
      addedAt,
      playCount,
      lastPlayedAt: playCount > 0 ? EPOCH_NOW - Math.floor(random() * 40 * 86_400) : null,
      favorite: random() > 0.86,
      artId: `cover-${slug(seed.album)}`,
      hasLyrics: ["Blue Interval", "Quiet Machinery", "Small Hours"].includes(title),
    });
    nextTrackId += 1;
  });
}

const FAVORITES = new Set<number>(
  TRACKS.filter((track) => track.favorite).map((track) => track.id),
);

/* --------------------------------------------------------------------- artwork --- */

const COVER_GROUNDS = ["#0A0B0C", "#121314", "#0C1113", "#141210"];
const COVER_INKS = ["#E23A1F", "#EDEAE3", "#B9451F", "#D9D3C7"];

/** A sleeve drawn from geometry only: ground, one block, two hairlines, a code strip. */
function coverSvg(album: string): string {
  const random = mulberry32(hash(album));
  const ground = COVER_GROUNDS[Math.floor(random() * COVER_GROUNDS.length)] ?? "#0A0B0C";
  const ink = COVER_INKS[Math.floor(random() * COVER_INKS.length)] ?? "#E23A1F";
  const variety = Math.floor(random() * 3);
  const glyph =
    variety === 0
      ? `<circle cx="160" cy="152" r="86" fill="${ink}"/>`
      : variety === 1
        ? `<rect x="42" y="96" width="236" height="112" fill="${ink}"/>`
        : `<path d="M42 214 L160 74 L278 214 Z" fill="${ink}"/>`;

  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 320" width="320" height="320">
  <rect width="320" height="320" fill="${ground}"/>
  ${glyph}
  <line x1="42" y1="252" x2="278" y2="252" stroke="#EDEAE3" stroke-opacity="0.28" stroke-width="1"/>
  <line x1="42" y1="264" x2="278" y2="264" stroke="#EDEAE3" stroke-opacity="0.14" stroke-width="1"/>
  <rect x="42" y="280" width="26" height="10" fill="${ink}"/>
  <rect x="74" y="280" width="10" height="10" fill="#EDEAE3" fill-opacity="0.7"/>
  <rect x="90" y="280" width="10" height="10" fill="#EDEAE3" fill-opacity="0.35"/>
</svg>`;
}

/* --------------------------------------------------------------------- lyrics --- */

const LYRICS: Record<string, LyricLine[]> = {
  "Blue Interval": [
    { timeMs: 12_000, text: "the room keeps time without me" },
    { timeMs: 19_500, text: "tape hiss where the window was" },
    { timeMs: 27_000, text: "I count the interval in blue" },
    { timeMs: 35_500, text: "and let the quiet do the rest" },
    { timeMs: 44_000, text: "nothing here is in a hurry" },
  ],
  "Quiet Machinery": [
    { timeMs: 8_400, text: "flywheel turning in the dark" },
    { timeMs: 16_900, text: "every gear a small decision" },
    { timeMs: 25_300, text: "keep the tolerance tight" },
    { timeMs: 34_600, text: "and the whole thing holds" },
  ],
  "Small Hours": [
    { timeMs: 6_000, text: "telephone face down" },
    { timeMs: 14_000, text: "clock in the kitchen losing" },
    { timeMs: 22_000, text: "I am not asleep, I am listening" },
  ],
};

/* -------------------------------------------------------------------- playlists --- */

interface DemoPlaylist extends Playlist {
  positions: number[];
}

const PLAYLISTS: DemoPlaylist[] = [
  {
    id: 1,
    name: "Late listening",
    kind: "manual",
    rules: null,
    trackCount: 4,
    createdAt: EPOCH_NOW - 90 * 86_400,
    updatedAt: EPOCH_NOW - 2 * 86_400,
    positions: [14, 3, 41, 27],
  },
  {
    id: 2,
    name: "Long-form",
    kind: "manual",
    rules: null,
    trackCount: 3,
    createdAt: EPOCH_NOW - 40 * 86_400,
    updatedAt: EPOCH_NOW - 5 * 86_400,
    positions: [2, 37, 60],
  },
];

const FOLDERS: LibraryFolder[] = [
  {
    id: 1,
    path: `${ROOT}\\Halden Roe`,
    addedAt: EPOCH_NOW - 400 * 86_400,
    trackCount: 10,
  },
  {
    id: 2,
    path: `${ROOT}\\Kestrel Nine`,
    addedAt: EPOCH_NOW - 210 * 86_400,
    trackCount: 10,
  },
];

const SETTINGS = new Map<string, string>();

/* ----------------------------------------------------------------------- engine --- */

interface DemoEngine {
  status: EngineSnapshot["status"];
  path: string | null;
  basePositionMs: number;
  startedAt: number;
  volume: number;
  speed: number;
  eq: EqSettings;
  device: string;
  pendingEnded: string | null;
}

const engine: DemoEngine = {
  status: "idle",
  path: null,
  basePositionMs: 0,
  startedAt: 0,
  volume: 0.86,
  speed: 1,
  eq: { enabled: false, gains: EQ_FREQUENCIES.map(() => 0), preampDb: 0 },
  device: "Synthetic output",
  pendingEnded: null,
};

function trackByPath(path: string | null): Track | undefined {
  if (!path) return undefined;
  return TRACKS.find((track) => track.path === path);
}

function currentPosition(): number {
  if (engine.status !== "playing") return engine.basePositionMs;
  const elapsed = (performance.now() - engine.startedAt) * engine.speed;
  return Math.min(engine.basePositionMs + elapsed, trackByPath(engine.path)?.durationMs ?? 0);
}

function snapshot(): EngineSnapshot {
  const track = trackByPath(engine.path);
  const duration = track?.durationMs ?? 0;
  const position = currentPosition();

  if (engine.status === "playing" && duration > 0 && position >= duration) {
    engine.status = "paused";
    engine.basePositionMs = duration;
    engine.pendingEnded = engine.path;
  }

  const ended = engine.pendingEnded;
  engine.pendingEnded = null;

  const seconds = position / 1000;
  const shape = Math.abs(Math.sin(seconds * 2.31) * Math.sin(seconds * 0.61 + 1.2));
  const level = engine.status === "playing" ? 0.14 + shape * 0.34 : 0;

  return {
    status: engine.status,
    path: engine.path,
    positionMs: Math.round(position),
    durationMs: duration,
    volume: engine.volume,
    speed: engine.speed,
    device: engine.device,
    sampleRate: track?.sampleRate ?? 48_000,
    level,
    peak: Math.min(1, level * 2.6),
    endedPath: ended,
    error: null,
  };
}

/* ------------------------------------------------------------------- querying --- */

function matches(track: Track, query: TrackQuery): boolean {
  if (query.artist && track.artist !== query.artist) return false;
  if (query.album && track.album !== query.album) return false;
  if (query.genre && track.genre !== query.genre) return false;
  if (query.format && track.format !== query.format) return false;
  if (query.favoritesOnly && !FAVORITES.has(track.id)) return false;

  const search = query.search?.trim().toLowerCase();
  if (search) {
    const haystack = `${track.title} ${track.artist} ${track.album} ${track.genre} ${track.path}`.toLowerCase();
    if (!haystack.includes(search)) return false;
  }
  return true;
}

function sortTracks(tracks: Track[], query: TrackQuery): Track[] {
  const direction = query.order === "desc" ? -1 : 1;
  const collate = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: "base" });

  const sorted = [...tracks].sort((a, b) => {
    switch (query.sort) {
      case "title":
        return direction * collate(a.title, b.title);
      case "album":
        return (
          direction *
          (collate(a.album, b.album) || (a.discNo ?? 0) - (b.discNo ?? 0) || (a.trackNo ?? 0) - (b.trackNo ?? 0))
        );
      case "added":
        return direction * (a.addedAt - b.addedAt);
      case "duration":
        return direction * (a.durationMs - b.durationMs);
      case "year":
        return direction * ((a.year ?? 0) - (b.year ?? 0));
      case "play-count":
        return direction * (a.playCount - b.playCount);
      case "format":
        return direction * collate(a.format, b.format);
      case "artist":
      default:
        return (
          direction *
          (collate(a.albumArtist, b.albumArtist) ||
            collate(a.album, b.album) ||
            (a.discNo ?? 0) - (b.discNo ?? 0) ||
            (a.trackNo ?? 0) - (b.trackNo ?? 0))
        );
    }
  });
  return sorted;
}

function selectTracks(query: TrackQuery): Track[] {
  const filtered = TRACKS.filter((track) => matches(track, query));
  const sorted = sortTracks(filtered, query);
  const offset = Math.max(0, query.offset);
  const limit = query.limit === 0 ? sorted.length : query.limit;
  return sorted.slice(offset, offset + limit);
}

function stats(): LibraryStats {
  const albums = new Set(TRACKS.map((track) => `${track.albumArtist}\u0000${track.album}`));
  const artists = new Set(TRACKS.map((track) => track.albumArtist));
  return {
    tracks: TRACKS.length,
    albums: albums.size,
    artists: artists.size,
    totalDurationMs: TRACKS.reduce((sum, track) => sum + track.durationMs, 0),
    totalBytes: TRACKS.reduce((sum, track) => sum + track.sizeBytes, 0),
    lastScanAt: EPOCH_NOW - 3 * 86_400,
    scanning: false,
  };
}

function playlistOf(playlistId: number): DemoPlaylist | undefined {
  return PLAYLISTS.find((playlist) => playlist.id === playlistId);
}

function hydrate(playlist: DemoPlaylist): Playlist {
  return {
    id: playlist.id,
    name: playlist.name,
    kind: playlist.kind,
    rules: playlist.rules,
    trackCount: playlist.positions.length,
    createdAt: playlist.createdAt,
    updatedAt: playlist.updatedAt,
  };
}

function syncPlaylistCount(playlist: DemoPlaylist): void {
  playlist.trackCount = playlist.positions.length;
  playlist.updatedAt = EPOCH_NOW;
}

function readLyrics(trackId: number): Lyrics {
  const track = TRACKS.find((candidate) => candidate.id === trackId);
  const lines = track ? LYRICS[track.title] : undefined;
  if (!track || !lines) {
    return {
      trackId,
      source: "none",
      lines: [],
      synced: false,
      warning: null,
    };
  }
  const stored = SETTINGS.get(`lyrics.${trackId}`);
  if (stored) {
    const parsed = parseLrc(stored);
    return {
      trackId,
      source: "manual",
      lines: parsed,
      synced: parsed.every((line) => line.timeMs !== null),
      warning: null,
    };
  }
  return { trackId, source: "embedded", lines, synced: true, warning: null };
}

function fail(message: string): never {
  throw new Error(message);
}

export const demoBackend: Backend = {
  kind: "demo",

  ...demoExtensions,

  async libraryStats() {
    return stats();
  },

  async listTracks(query) {
    return selectTracks(query).map((track) => ({ ...track, favorite: FAVORITES.has(track.id) }));
  },

  async countTracks(query) {
    return TRACKS.filter((track) => matches(track, query)).length;
  },

  async distinctValues(column) {
    const key = column === "album_artist" ? "albumArtist" : column;
    const values = new Set<string>();
    for (const track of TRACKS) {
      const value = track[key as keyof Track];
      if (typeof value === "string" && value) values.add(value);
    }
    return [...values].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
  },

  async groupTracks(column) {
    const groups = new Map<string, TrackGroup>();
    for (const track of TRACKS) {
      const artist = track.albumArtist || track.artist;
      const key = column === "album" ? track.album : artist;
      if (!key) continue;
      const existing = groups.get(key);
      if (existing) {
        existing.trackCount += 1;
        existing.durationMs += track.durationMs;
        continue;
      }
      groups.set(key, {
        name: key,
        subtitle: column === "album" ? artist : "",
        trackCount: 1,
        durationMs: track.durationMs,
        artId: track.artId,
      });
    }
    return [...groups.values()].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );
  },

  async getTrack(trackId) {    return TRACKS.find((track) => track.id === trackId) ?? null;
  },

  async setFavorite(trackId, favorite) {
    if (favorite) FAVORITES.add(trackId);
    else FAVORITES.delete(trackId);
  },

  async recentTracks(limit) {
    return TRACKS.filter((track) => track.lastPlayedAt !== null)
      .sort((a, b) => (b.lastPlayedAt ?? 0) - (a.lastPlayedAt ?? 0))
      .slice(0, limit);
  },

  async recentlyAdded(limit) {
    return [...TRACKS].sort((a, b) => b.addedAt - a.addedAt).slice(0, limit);
  },

  async listFolders() {
    return FOLDERS.map((folder) => ({ ...folder }));
  },

  async scanFolder(path) {
    const folder = FOLDERS.find((candidate) => candidate.path === path);
    if (!folder) {
      FOLDERS.push({ id: FOLDERS.length + 1, path, addedAt: EPOCH_NOW, trackCount: 0 });
    }
  },

  async rescanLibrary() {
    SETTINGS.set("demo.lastScan", String(EPOCH_NOW));
  },

  async removeFolder(folderId) {
    const index = FOLDERS.findIndex((folder) => folder.id === folderId);
    if (index >= 0) FOLDERS.splice(index, 1);
  },

  async artwork(artId) {
    const album = ALBUM_SEEDS.find((seed) => `cover-${slug(seed.album)}` === artId);
    if (!album) return null;
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(coverSvg(album.album))}`;
  },

  async updateTags(path, edit: TagEdit) {
    const track = trackByPath(path) ?? fail(`no such track: ${path}`);
    if (edit.title !== undefined) track.title = edit.title;
    if (edit.artist !== undefined) track.artist = edit.artist;
    if (edit.album !== undefined) track.album = edit.album;
    if (edit.albumArtist !== undefined) track.albumArtist = edit.albumArtist;
    if (edit.genre !== undefined) track.genre = edit.genre;
    if (edit.year !== undefined) track.year = edit.year;
    if (edit.trackNo !== undefined) track.trackNo = edit.trackNo;
    if (edit.discNo !== undefined) track.discNo = edit.discNo;
    return { ...track };
  },

  async lyricsFor(trackId) {
    return readLyrics(trackId);
  },

  async saveLyrics(trackId, content) {
    SETTINGS.set(`lyrics.${trackId}`, content);
    const track = TRACKS.find((candidate) => candidate.id === trackId);
    if (track) track.hasLyrics = true;
    return readLyrics(trackId);
  },

  async importLyrics() {
    return null;
  },

  async exportLyrics() {
    return null;
  },

  async estimateLyrics(trackId) {
    const lyrics = readLyrics(trackId);
    const track = TRACKS.find((candidate) => candidate.id === trackId);
    const duration = track?.durationMs ?? 60_000;
    const lines = lyrics.lines.map((line, index) => ({
      text: line.text,
      timeMs: Math.round((duration / (lyrics.lines.length + 2)) * (index + 1)),
    }));
    return { ...lyrics, lines, synced: true, warning: "timings estimated from line length — adjust them by hand" };
  },

  async listPlaylists() {
    return PLAYLISTS.map(hydrate);
  },

  async createPlaylist(name, kind) {
    const id = Math.max(0, ...PLAYLISTS.map((playlist) => playlist.id)) + 1;
    PLAYLISTS.push({
      id,
      name,
      kind,
      rules: null,
      trackCount: 0,
      createdAt: EPOCH_NOW,
      updatedAt: EPOCH_NOW,
      positions: [],
    });
    return id;
  },

  async renamePlaylist(playlistId, name) {
    const playlist = playlistOf(playlistId) ?? fail("playlist not found");
    playlist.name = name;
    playlist.updatedAt = EPOCH_NOW;
  },

  async deletePlaylist(playlistId) {
    const index = PLAYLISTS.findIndex((playlist) => playlist.id === playlistId);
    if (index >= 0) PLAYLISTS.splice(index, 1);
  },

  async playlistTracks(playlistId) {
    const playlist = playlistOf(playlistId) ?? fail("playlist not found");
    return playlist.positions
      .map((trackId) => TRACKS.find((track) => track.id === trackId))
      .filter((track): track is Track => track !== undefined);
  },

  async updateSmartRules(playlistId, rules) {
    const playlist = playlistOf(playlistId) ?? fail("playlist not found");
    playlist.rules = rules;
    playlist.kind = "smart";
  },

  async addToPlaylist(playlistId, trackIds) {
    const playlist = playlistOf(playlistId) ?? fail("playlist not found");
    const seen = new Set(playlist.positions);
    for (const trackId of trackIds) {
      if (!seen.has(trackId)) {
        playlist.positions.push(trackId);
        seen.add(trackId);
      }
    }
    syncPlaylistCount(playlist);
  },

  async removeFromPlaylist(playlistId, position) {
    const playlist = playlistOf(playlistId) ?? fail("playlist not found");
    playlist.positions.splice(position, 1);
    syncPlaylistCount(playlist);
  },

  async movePlaylistItem(playlistId, from, to) {
    const playlist = playlistOf(playlistId) ?? fail("playlist not found");
    if (from < 0 || from >= playlist.positions.length) return;
    const [moved] = playlist.positions.splice(from, 1);
    if (moved === undefined) return;
    playlist.positions.splice(Math.max(0, Math.min(to, playlist.positions.length)), 0, moved);
    syncPlaylistCount(playlist);
  },

  async getSetting(key) {
    return SETTINGS.get(key) ?? null;
  },

  async setSetting(key, value) {
    SETTINGS.set(key, value);
  },

  async engineSnapshot() {
    return snapshot();
  },

  async engineLoad(path, startMs) {
    engine.path = path;
    engine.basePositionMs = startMs;
    engine.startedAt = performance.now();
    engine.status = "playing";
  },

  async enginePlay() {
    if (!engine.path) return;
    engine.startedAt = performance.now();
    engine.basePositionMs = currentPosition();
    engine.status = "playing";
  },

  async enginePause() {
    engine.basePositionMs = currentPosition();
    engine.status = "paused";
  },

  async engineStop() {
    engine.status = "idle";
    engine.basePositionMs = 0;
    engine.path = null;
  },

  async engineSeek(positionMs) {
    engine.basePositionMs = positionMs;
    engine.startedAt = performance.now();
  },

  async engineSetVolume(volume) {
    engine.volume = volume;
  },

  async engineSetSpeed(speed) {
    engine.basePositionMs = currentPosition();
    engine.startedAt = performance.now();
    engine.speed = speed;
  },

  async engineSetEq(eq) {
    engine.eq = eq;
  },

  async engineSetNext(_path) {
    /* the demo advances only through its own end-of-track path */
  },

  async outputDevices() {
    return [
      { name: "Synthetic output", isDefault: true },
      { name: "Synthetic headphones", isDefault: false },
    ] satisfies OutputDevice[];
  },

  async setOutputDevice(name) {
    engine.device = name;
  },

  async reportPlay(trackId) {
    const track = TRACKS.find((candidate) => candidate.id === trackId);
    if (track) {
      track.playCount += 1;
      track.lastPlayedAt = EPOCH_NOW;
    }
  },

  async reportPlayback() {
    /* the OS media session only exists in the desktop app */
  },

  async pickFolder() {
    return null;
  },

  async pickFiles() {
    return [];
  },

  async revealTrack() {
    fail("Revealing a file needs the desktop app.");
  },

  async onScan() {
    return () => {};
  },

  async onMediaControl() {
    return () => {};
  },
};

/** Query helper used by the component layer for the default library view. */
