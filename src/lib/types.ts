/** Mirror of the serde shapes in `crates/qcore/src/model.rs` and `db.rs`.
 *  Field names are exactly what the Rust side serialises: camelCase structs,
 *  kebab-case enums. Changing one here without changing the core is a bug. */

export type PlaybackStatus = "idle" | "playing" | "paused" | "error";
export type RepeatMode = "off" | "all" | "one";
export type SortOrder = "asc" | "desc";
export type SortField =
  | "title"
  | "artist"
  | "album"
  | "added"
  | "duration"
  | "year"
  | "play-count"
  | "format";

export type SmartField =
  | "artist"
  | "album"
  | "genre"
  | "title"
  | "year"
  | "format"
  | "favorite"
  | "play-count"
  | "added-days"
  | "duration-seconds";

export type SmartOp =
  | "is"
  | "is-not"
  | "contains"
  | "greater-than"
  | "less-than"
  | "in-last";

export type LyricsSource = "embedded" | "sidecar" | "manual" | "none";

export interface Track {
  id: number;
  path: string;
  title: string;
  artist: string;
  albumArtist: string;
  album: string;
  genre: string;
  year: number | null;
  trackNo: number | null;
  discNo: number | null;
  durationMs: number;
  sampleRate: number | null;
  channels: number | null;
  bitDepth: number | null;
  bitrateKbps: number | null;
  format: string;
  sizeBytes: number;
  mtime: number;
  addedAt: number;
  playCount: number;
  lastPlayedAt: number | null;
  favorite: boolean;
  artId: string | null;
  hasLyrics: boolean;
}

export interface TrackQuery {
  search: string | null;
  artist: string | null;
  album: string | null;
  genre: string | null;
  format: string | null;
  favoritesOnly: boolean;
  sort: SortField;
  order: SortOrder;
  limit: number;
  offset: number;
}

export function emptyQuery(): TrackQuery {
  return {
    search: null,
    artist: null,
    album: null,
    genre: null,
    format: null,
    favoritesOnly: false,
    sort: "artist",
    order: "asc",
    limit: 0,
    offset: 0,
  };
}

export interface SmartRule {
  field: SmartField;
  op: SmartOp;
  value: string;
}

export interface SmartRules {
  matchAny: boolean;
  rules: SmartRule[];
  limit: number;
}

export interface Playlist {
  id: number;
  name: string;
  kind: "manual" | "smart";
  rules: SmartRules | null;
  trackCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface LyricLine {
  timeMs: number | null;
  text: string;
}

export interface Lyrics {
  trackId: number;
  source: LyricsSource;
  lines: LyricLine[];
  synced: boolean;
  warning: string | null;
}

/** One album or artist row in a browse list, aggregated by the core. */
export interface TrackGroup {
  name: string;
  subtitle: string;
  trackCount: number;
  durationMs: number;
  artId: string | null;
}

export interface LibraryFolder {
  id: number;
  path: string;
  addedAt: number;
  trackCount: number;
}

export interface LibraryStats {
  tracks: number;
  albums: number;
  artists: number;
  totalDurationMs: number;
  totalBytes: number;
  lastScanAt: number | null;
  scanning: boolean;
}

export interface ScanProgress {
  scanning: boolean;
  folder: string;
  filesSeen: number;
  tracksAdded: number;
  tracksUpdated: number;
  tracksRemoved: number;
  failures: string[];
  finishedAt: number | null;
}

export interface OutputDevice {
  name: string;
  isDefault: boolean;
}

export interface EngineSnapshot {
  status: PlaybackStatus;
  path: string | null;
  positionMs: number;
  durationMs: number;
  volume: number;
  speed: number;
  device: string;
  sampleRate: number;
  level: number;
  peak: number;
  endedPath: string | null;
  error: string | null;
}

export interface EqSettings {
  enabled: boolean;
  gains: number[];
  preampDb: number;
}

export interface TagEdit {
  title?: string;
  artist?: string;
  album?: string;
  albumArtist?: string;
  genre?: string;
  year?: number;
  trackNo?: number;
  discNo?: number;
}

/** Ten bands, exactly the frequencies the Rust equaliser runs. */
export const EQ_FREQUENCIES = [
  31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000,
] as const;

export const AUDIO_EXTENSIONS = [
  "flac",
  "mp3",
  "m4a",
  "aac",
  "wav",
  "ogg",
  "opus",
  "aiff",
  "aif",
  "alac",
] as const;
