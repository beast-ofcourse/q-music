import type { Track } from "./types";

const MS_PER_SECOND = 1000;
const MS_PER_MINUTE = 60 * MS_PER_SECOND;
const MS_PER_HOUR = 60 * MS_PER_MINUTE;

/** `4:07`, or `1:02:33` once a track passes an hour. Mono digits depend on the
 *  caller setting a tabular font; the string itself carries no padding. */
export function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0:00";
  const total = Math.floor(ms / MS_PER_SECOND);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  }
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/** Human span for totals: `2 h 41 min`, `18 min`. */
export function formatSpan(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0 min";
  if (ms >= MS_PER_HOUR) {
    const hours = Math.floor(ms / MS_PER_HOUR);
    const minutes = Math.round((ms % MS_PER_HOUR) / MS_PER_MINUTE);
    return minutes > 0 ? `${hours} h ${minutes} min` : `${hours} h`;
  }
  const minutes = Math.max(1, Math.round(ms / MS_PER_MINUTE));
  return `${minutes} min`;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value >= 100 || unit === 0 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

/** `44.1 kHz`, `96 kHz`. */
export function formatRate(hz: number): string {
  if (!Number.isFinite(hz) || hz <= 0) return "—";
  const khz = hz / 1000;
  return `${Number.isInteger(khz) ? khz : khz.toFixed(1)} kHz`;
}

export function formatDate(epochSeconds: number | null): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
  });
}

/** A 0..=1 amplitude as dBFS. Silence is `−∞`, the honest reading, not `0`. */
export function formatDbfs(amplitude: number): string {
  if (!Number.isFinite(amplitude) || amplitude <= 0) return "−∞ dBFS";
  const db = 20 * Math.log10(Math.min(1, amplitude));
  if (db <= -60) return "−60.0 dBFS";
  return `${db > 0 ? "+" : "−"}${Math.abs(db).toFixed(1)} dBFS`;
}

/** The colour-block technical strip: what the file actually is. */
export function techLine(track: Track): string[] {
  const parts: string[] = [];
  if (track.format) parts.push(track.format.toUpperCase());
  if (track.bitDepth) parts.push(`${track.bitDepth}-bit`);
  if (track.sampleRate) parts.push(formatRate(track.sampleRate));
  if (!track.bitDepth && track.bitrateKbps) parts.push(`${track.bitrateKbps} kbps`);
  return parts;
}

/** Compact form used in table cells: `FLAC 24/96`. */
export function techCompact(track: Track): string {
  const label = track.format ? track.format.toUpperCase() : "—";
  if (track.bitDepth && track.sampleRate) {
    return `${label} ${track.bitDepth}/${Number((track.sampleRate / 1000).toFixed(1))}`;
  }
  if (track.bitrateKbps) return `${label} ${track.bitrateKbps}`;
  return label;
}

export function formatSpeed(speed: number): string {
  return `${Number(speed.toFixed(2))}×`;
}

/** Bytes per second as a transfer rate: `4.2 MB/s`. Unknown reads as a dash, not zero. */
export function formatTransferRate(bytesPerSecond: number | null): string {
  if (bytesPerSecond === null || !Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) {
    return "—";
  }
  return `${formatBytes(bytesPerSecond)}/s`;
}

export function plural(count: number, singular: string, pluralForm?: string): string {
  return count === 1 ? singular : (pluralForm ?? `${singular}s`);
}

/** `3 tracks · 18 min`, the meta readout under a view title. */
export function trackSummary(tracks: Track[]): string {
  const total = tracks.reduce((sum, track) => sum + track.durationMs, 0);
  return `${tracks.length} ${plural(tracks.length, "track")} · ${formatSpan(total)}`;
}

/** File name from a full path, for a track whose tags carry no title. */
export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}
