import { useRef, useState } from "react";

import { EQ_FREQUENCIES } from "../lib/types";
import { useStore } from "../state/store";

const MIN_DB = -12;
const MAX_DB = 12;
const STEP_DB = 0.5;
const ZERO_PERCENT = 50;

const PRESETS: Array<{ name: string; gains: readonly number[] }> = [
  { name: "Flat", gains: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
  { name: "Warm", gains: [3, 2.5, 1.5, 0.5, 0, 0, -0.5, -1, -1.5, -2] },
  { name: "Bright", gains: [-2, -1.5, -1, 0, 0.5, 1, 1.5, 2.5, 3, 3] },
];

function clampGain(gain: number): number {
  return Math.max(MIN_DB, Math.min(MAX_DB, Math.round(gain / STEP_DB) * STEP_DB));
}

function formatGain(gain: number): string {
  const rounded = Math.round(gain * 10) / 10;
  return `${rounded > 0 ? "+" : ""}${rounded}`;
}

function formatFrequency(hertz: number): string {
  return hertz >= 1000 ? `${hertz / 1000}k` : String(hertz);
}

function gainTopPercent(gain: number): number {
  return ((MAX_DB - gain) / (MAX_DB - MIN_DB)) * 100;
}

interface BandProps {
  frequency: number;
  gain: number;
  onChange: (gain: number) => void;
}

function Band({ frequency, gain, onChange }: BandProps) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);

  const top = gainTopPercent(gain);

  function gainAt(clientY: number): number {
    const rect = trackRef.current?.getBoundingClientRect();
    if (!rect || rect.height === 0) return gain;
    const ratio = 1 - (clientY - rect.top) / rect.height;
    return clampGain(MIN_DB + ratio * (MAX_DB - MIN_DB));
  }

  return (
    <div className="eq-band">
      <span className="eq-gain">{formatGain(gain)}</span>

      <div
        ref={trackRef}
        className="eq-slider"
        role="slider"
        tabIndex={0}
        aria-label={`${formatFrequency(frequency)} hertz gain`}
        aria-orientation="vertical"
        aria-valuemin={MIN_DB}
        aria-valuemax={MAX_DB}
        aria-valuenow={gain}
        aria-valuetext={`${formatGain(gain)} decibels`}
        data-dragging={dragging}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
          setDragging(true);
          onChange(gainAt(event.clientY));
        }}
        onPointerMove={(event) => {
          if (!dragging) return;
          onChange(gainAt(event.clientY));
        }}
        onPointerUp={(event) => {
          setDragging(false);
          onChange(gainAt(event.clientY));
        }}
        onPointerCancel={() => setDragging(false)}
        onKeyDown={(event) => {
          const step = event.shiftKey ? STEP_DB * 6 : STEP_DB;
          if (event.key === "ArrowUp") {
            event.preventDefault();
            onChange(clampGain(gain + step));
          } else if (event.key === "ArrowDown") {
            event.preventDefault();
            onChange(clampGain(gain - step));
          } else if (event.key === "Home") {
            event.preventDefault();
            onChange(MIN_DB);
          } else if (event.key === "End") {
            event.preventDefault();
            onChange(MAX_DB);
          }
        }}
      >
        <span className="eq-slider-line" />
        <span className="eq-slider-zero" />
        <span
          className="eq-slider-fill"
          style={{
            top: `${Math.min(ZERO_PERCENT, top)}%`,
            height: `${Math.abs(ZERO_PERCENT - top)}%`,
          }}
        />
        <span className="eq-slider-handle" style={{ top: `${top}%` }} />
      </div>

      <span className="eq-freq">{formatFrequency(frequency)}</span>
    </div>
  );
}

export function EqView() {
  const { eq, setEq } = useStore();

  function updateGain(index: number, gain: number) {
    setEq({
      ...eq,
      // Moving a band is an intent to hear it, so the first move switches the unit on.
      enabled: true,
      gains: eq.gains.map((value, position) => (position === index ? gain : value)),
    });
  }

  function applyPreset(gains: readonly number[]) {
    setEq({
      ...eq,
      enabled: true,
      gains: EQ_FREQUENCIES.map((_, index) => clampGain(gains[index] ?? 0)),
    });
  }

  return (
    <>
      <div className="view-head">
        <h1 className="view-title">Equaliser</h1>
        <div className="view-tools">
          <label className="toggle">
            <input
              type="checkbox"
              checked={eq.enabled}
              onChange={(event) => setEq({ ...eq, enabled: event.target.checked })}
            />
            <span>{eq.enabled ? "On" : "Off"}</span>
          </label>
        </div>
      </div>

      <div className="view-body">
        <div className="eq">
          <div className="eq-bands">
            {EQ_FREQUENCIES.map((frequency, index) => (
              <Band
                key={frequency}
                frequency={frequency}
                gain={eq.gains[index] ?? 0}
                onChange={(gain) => updateGain(index, gain)}
              />
            ))}
          </div>

          <div className="panel-section">
            <h2 className="panel-title">Preamp</h2>
            <div className="eq-preamp">
              <input
                type="range"
                className="range"
                min={MIN_DB}
                max={MAX_DB}
                step={STEP_DB}
                value={eq.preampDb}
                aria-label="Preamp gain"
                onChange={(event) => setEq({ ...eq, preampDb: Number(event.target.value) })}
              />
              <span className="volume-value">{formatGain(eq.preampDb)} dB</span>
            </div>
            <p className="hint">
              Boosting bands adds gain, so the preamp is the place to pull it back. Ten bands
              run inside the engine before the samples reach the device.
            </p>
          </div>

          <div className="panel-section">
            <h2 className="panel-title">Presets</h2>
            <div className="actions">
              {PRESETS.map((preset) => (
                <button
                  key={preset.name}
                  type="button"
                  className="button"
                  onClick={() => applyPreset(preset.gains)}
                >
                  {preset.name}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
