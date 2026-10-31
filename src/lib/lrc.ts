import type { LyricLine } from "./types";

/** `[mm:ss]`, `[mm:ss.xx]` and `[mm:ss.xxx]` — the three shapes players write. */
const TIMESTAMP = /^\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]\s*(.*)$/;

/**
 * Reads LRC text into lines. Text with no timestamp is kept as an unsynced line,
 * because a plain `.txt` lyric file is still lyrics and dropping it would lose the
 * listener's own file.
 */
export function parseLrc(content: string): LyricLine[] {
  const lines: LyricLine[] = [];

  for (const raw of content.split(/\r?\n/)) {
    const text = raw.trim();
    if (!text) continue;

    const match = TIMESTAMP.exec(text);
    if (!match) {
      lines.push({ timeMs: null, text });
      continue;
    }

    const minutes = Number(match[1] ?? 0);
    const seconds = Number(match[2] ?? 0);
    const fraction = Number((match[3] ?? "0").padEnd(3, "0"));
    const lyric = match[4] ?? "";
    if (lyric) {
      lines.push({ timeMs: minutes * 60_000 + seconds * 1000 + fraction, text: lyric });
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
