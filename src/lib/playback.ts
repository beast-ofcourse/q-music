import type { RepeatMode } from "./types";

/** The engine clamps speed to 0.5–2.0; these are the steps the interface offers. */
export const PLAYBACK_SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2] as const;

/** Next speed after the current one, wrapping back to the slowest. */
export function nextSpeed(current: number): number {
  const index = PLAYBACK_SPEEDS.findIndex((value) => value > current + 0.001);
  return index === -1 ? (PLAYBACK_SPEEDS[0] ?? 1) : (PLAYBACK_SPEEDS[index] ?? 1);
}

/** One cycle for the repeat button and the R key, so they cannot disagree. */
export const REPEAT_CYCLE: Record<RepeatMode, RepeatMode> = {
  off: "all",
  all: "one",
  one: "off",
};

export const REPEAT_LABEL: Record<RepeatMode, string> = {
  off: "Repeat off",
  all: "Repeat the queue",
  one: "Repeat this track",
};
