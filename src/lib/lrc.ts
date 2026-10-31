import type { LyricLine } from "./types";

const METADATA_KEYS = new Set([
  "ar", "ti", "al", "au", "by", "offset", "re", "ve", "length", "tool", "encoder",
]);

/** `[mm:ss]`, `[mm:ss.xx]` and `[mm:ss.xxx]` — the three shapes players write. */
const TIMESTAMP = /^\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]\s*(.*)$/;

function isMetadataTag(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed.startsWith("[")) return false;
  const close = trimmed.indexOf("]");
  if (close === -1) return false;
  const inside = trimmed.slice(1, close);
  const colonPos = inside.indexOf(":");
  if (colonPos === -1) return false;
  const key = inside.slice(0, colonPos).trim().toLowerCase();
  return METADATA_KEYS.has(key);
}

function isTimestampLike(s: string): boolean {
  const trimmed = s.trim();
  if (!trimmed) return false;
  const parts = trimmed.split(":");
  if (parts.length === 1 && parts[0]) {
    const splitResult = parts[0].split(".");
    const whole = splitResult[0] ?? "";
    const frac = splitResult[1];
    return /^\d+$/.test(whole) && (frac === undefined || /^\d+$/.test(frac ?? ""));
  }
  if (parts.length === 2) {
    const m = parts[0] ?? "";
    const s = parts[1] ?? "";
    if (!/^\d+$/.test(m)) return false;
    const splitResult = s.split(".");
    const whole = splitResult[0] ?? "";
    const frac = splitResult[1];
    return /^\d+$/.test(whole) && (frac === undefined || /^\d+$/.test(frac));
  }
  return false;
}

function stripEnhancedTimestamps(text: string): string {
  let result = "";
  let i = 0;
  while (i < text.length) {
    if (text[i] === "<") {
      const closeIdx = text.indexOf(">", i);
      if (closeIdx !== -1) {
        const inner = text.slice(i + 1, closeIdx);
        if (isTimestampLike(inner)) {
          i = closeIdx + 1;
          continue;
        }
      }
    }
    result += text[i];
    i++;
  }
  return result;
}

function stripStrayTimestamps(text: string): string {
  let result = text;
  // Strip inline <timestamp> patterns
  let changed = true;
  while (changed) {
    changed = false;
    const start = result.indexOf("<");
    if (start === -1) break;
    const end = result.indexOf(">", start);
    if (end === -1) break;
    const inner = result.slice(start + 1, end);
    if (isTimestampLike(inner)) {
      result = result.slice(0, start) + result.slice(end + 1);
      changed = true;
    } else {
      break;
    }
  }
  // Strip version markers like v1:, v2:, etc.
  result = result.replace(/^v\d+:\s*/i, "");
  return result.trim();
}

/** Check if a line is a metadata/credit line that should not be displayed as lyrics. */
function isMetadataLine(text: string): boolean {
  const lower = text.toLowerCase().trim();
  // Skip empty lines
  if (!lower) return true;
  // Skip lines that are just URLs
  if (/^https?:\/\//.test(lower)) return true;
  // Skip common credit/metadata patterns
  if (/^(produced|written|composed|arranged|mixed|mastered|engineered|recorded|published|copyright|©|℗|p\.\s*&\s*c\.)/i.test(lower)) return true;
  // Skip lines that are just "v1:", "v2:", etc.
  if (/^v\d+:\s*$/.test(lower)) return true;
  return false;
}

/**
 * Reads LRC text into lines. Text with no timestamp is kept as an unsynced line.
 * Handles standard LRC, enhanced LRC (inline word timestamps), and metadata tags.
 */
export function parseLrc(content: string): LyricLine[] {
  const lines: LyricLine[] = [];

  for (const raw of content.split(/\r?\n/)) {
    const text = raw.trim();
    if (!text) continue;

    // Skip metadata tags
    if (isMetadataTag(text)) continue;

    const match = TIMESTAMP.exec(text);
    if (!match) {
      const cleaned = stripStrayTimestamps(stripEnhancedTimestamps(text));
      if (cleaned && !isMetadataLine(cleaned)) {
        lines.push({ timeMs: null, text: cleaned });
      }
      continue;
    }

    const minutes = Number(match[1] ?? 0);
    const seconds = Number(match[2] ?? 0);
    const fraction = Number((match[3] ?? "0").padEnd(3, "0"));
    const lyric = stripStrayTimestamps(stripEnhancedTimestamps(match[4] ?? ""));
    const timeMs = minutes * 60_000 + seconds * 1000 + fraction;

    if (lyric && !isMetadataLine(lyric)) {
      lines.push({ timeMs, text: lyric });
    }
  }

  return lines;
}

/** Writes lines back as LRC. Unsynced lines stay as bare text so the file round-trips. */
export function toLrc(lines: LyricLine[]): string {
  return lines
    .map((line) => {
      if (line.timeMs === null) return line.text;
      const total = Math.max(0, line.timeMs);
      const minutes = Math.floor(total / 60_000);
      const seconds = Math.floor((total % 60_000) / 1000);
      const hundredths = Math.floor((total % 1000) / 10);
      const stamp = `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(hundredths).padStart(2, "0")}`;
      return `[${stamp}]${line.text}`;
    })
    .join("\n");
}