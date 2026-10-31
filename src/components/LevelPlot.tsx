import { useEffect, useState } from "react";

import { formatDbfs } from "../lib/format";
import type { EngineSnapshot } from "../lib/types";

/** Bars across the plot. At the 120 ms poll this is a little under six seconds. */
const BARS = 44;
const BAR_WIDTH = 3;
const BAR_GAP = 1;
const PLOT_WIDTH = BARS * (BAR_WIDTH + BAR_GAP);
const PLOT_HEIGHT = 76;
const BASELINE = 74;
const BAR_AREA = 66;

/** A well-mastered track sits near 0.2 RMS; this scale puts that around two thirds. */
const AMPLITUDE_SCALE = 3;
/** Slight curve so quiet passages are still visible without faking loudness. */
const CURVE = 0.85;

function barHeight(level: number): number {
  if (!Number.isFinite(level) || level <= 0) return 0;
  return Math.min(1, Math.pow(level * AMPLITUDE_SCALE, CURVE)) * BAR_AREA;
}

/**
 * The signature plot: the engine's measured RMS drawn as a strip of bars. It is not a
 * waveform and not decoration — the numbers come from the samples leaving the output,
 * so silence draws silence and pause lets the bars fall.
 */
export function LevelPlot({ snapshot }: { snapshot: EngineSnapshot }) {
  const [history, setHistory] = useState<number[]>(() => new Array<number>(BARS).fill(0));

  useEffect(() => {
    setHistory((previous) => [...previous.slice(1), snapshot.level]);
  }, [snapshot]);

  const peakY = BASELINE - Math.min(1, snapshot.peak * AMPLITUDE_SCALE) * BAR_AREA;

  return (
    <div className="plot">
      <svg
        className="plot-svg"
        viewBox={`0 0 ${PLOT_WIDTH} ${PLOT_HEIGHT}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={`Output level ${formatDbfs(snapshot.level)}`}
      >
        <line
          x1="0"
          y1={BASELINE + 0.5}
          x2={PLOT_WIDTH}
          y2={BASELINE + 0.5}
          stroke="currentColor"
          strokeOpacity="0.16"
          strokeWidth="1"
          vectorEffect="non-scaling-stroke"
        />

        <g shapeRendering="crispEdges">
          {history.map((level, index) => {
            const height = barHeight(level);
            if (height < 0.5) return null;
            return (
              <rect
                key={index}
                x={index * (BAR_WIDTH + BAR_GAP)}
                y={BASELINE - height}
                width={BAR_WIDTH}
                height={height}
                fill="var(--signal)"
              />
            );
          })}
        </g>

        {snapshot.peak > 0.002 ? (
          <line
            x1="0"
            y1={peakY}
            x2={PLOT_WIDTH}
            y2={peakY}
            stroke="var(--signal-text)"
            strokeOpacity="0.55"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
        ) : null}
      </svg>

      <div className="plot-foot">
        <span>RMS {formatDbfs(snapshot.level)}</span>
        <span>
          PEAK {formatDbfs(snapshot.peak)} · {Math.round(snapshot.sampleRate / 1000)} kHz
        </span>
      </div>
    </div>
  );
}
