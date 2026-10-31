import { useEffect, useState } from "react";

import { Icon } from "./Icon";
import { Scrubber } from "./Scrubber";
import { useArtwork } from "../lib/artwork";
import { formatSpeed } from "../lib/format";
import { nextSpeed, REPEAT_CYCLE, REPEAT_LABEL } from "../lib/playback";
import { useStore } from "../state/store";

export function Transport() {
  const {
    currentTrack,
    snapshot,
    shuffle,
    repeat,
    eq,
    togglePlay,
    next,
    previous,
    stop,
    seek,
    setVolume,
    toggleMute,
    setSpeed,
    setShuffle,
    setRepeat,
    setEq,
  } = useStore();

  const artwork = useArtwork(currentTrack?.artId ?? null);
  const [volume, setLocalVolume] = useState(snapshot.volume);

  // The engine is the truth for loudness; the slider only holds it while the echo
  // travels back, so the thumb never snaps away from the pointer.
  useEffect(() => {
    setLocalVolume(snapshot.volume);
  }, [snapshot.volume]);

  const playing = snapshot.status === "playing";
  const hasTrack = currentTrack !== null;
  const muted = volume < 0.001;
  const repeatIcon = repeat === "one" ? "repeat-one" : "repeat";

  function cycleSpeed() {
    setSpeed(nextSpeed(snapshot.speed));
  }

  return (
    <footer className="transport">
      <div className="transport-now">
        <div className="transport-thumb">
          {artwork ? <img src={artwork} alt="" /> : <Icon name="disc" size={16} />}
        </div>
        <div className="transport-id">
          <span className="transport-title">{currentTrack?.title ?? "Nothing playing"}</span>
          <span className="transport-artist">
            {currentTrack ? currentTrack.artist : "Play a row to start"}
          </span>
        </div>
      </div>

      <div className="transport-center">
        <div className="transport-buttons">
          <button
            type="button"
            className="tbtn"
            aria-pressed={shuffle}
            aria-label={shuffle ? "Shuffle on" : "Shuffle off"}
            title={shuffle ? "Shuffle on" : "Shuffle off"}
            onClick={() => setShuffle(!shuffle)}
          >
            <Icon name="shuffle" size={15} />
          </button>

          <button
            type="button"
            className="tbtn"
            aria-label="Previous track"
            title="Previous track"
            onClick={() => void previous()}
          >
            <Icon name="previous" size={15} />
          </button>

          <button
            type="button"
            className="tbtn tbtn-primary"
            aria-label={playing ? "Pause" : "Play"}
            title={playing ? "Pause (Space)" : "Play (Space)"}
            onClick={() => void togglePlay()}
          >
            <Icon name={playing ? "pause" : "play"} size={16} />
          </button>

          <button
            type="button"
            className="tbtn"
            aria-label="Next track"
            title="Next track"
            onClick={() => void next()}
          >
            <Icon name="next" size={15} />
          </button>

          <button
            type="button"
            className="tbtn"
            aria-pressed={repeat !== "off"}
            aria-label={REPEAT_LABEL[repeat]}
            title={REPEAT_LABEL[repeat]}
            onClick={() => setRepeat(REPEAT_CYCLE[repeat])}
          >
            <Icon name={repeatIcon} size={15} />
          </button>
        </div>

        <Scrubber
          positionMs={snapshot.positionMs}
          durationMs={snapshot.durationMs}
          onSeek={seek}
          disabled={!hasTrack}
        />
      </div>

      <div className="transport-tools">
        <div className="volume">
          <button
            type="button"
            className="tbtn"
            aria-label={muted ? "Unmute" : "Mute"}
            title={muted ? "Unmute (M)" : "Mute (M)"}
            onClick={toggleMute}
          >
            <Icon name={muted ? "mute" : "volume"} size={15} />
          </button>
          <input
            type="range"
            className="range"
            min={0}
            max={1}
            step={0.01}
            value={volume}
            aria-label="Volume"
            onChange={(event) => {
              const nextVolume = Number(event.target.value);
              setLocalVolume(nextVolume);
              setVolume(nextVolume);
            }}
          />
          <span className="volume-value">{Math.round(volume * 100)}</span>
        </div>

        <button
          type="button"
          className="speed-button"
          data-active={Math.abs(snapshot.speed - 1) > 0.001}
          title="Playback speed — click to step through"
          aria-label={`Playback speed ${formatSpeed(snapshot.speed)}`}
          onClick={cycleSpeed}
        >
          {formatSpeed(snapshot.speed)}
        </button>

        <button
          type="button"
          className="tbtn"
          aria-pressed={eq.enabled}
          aria-label={eq.enabled ? "Equaliser on" : "Equaliser off"}
          title={eq.enabled ? "Equaliser on" : "Equaliser off"}
          onClick={() => setEq({ ...eq, enabled: !eq.enabled })}
        >
          <Icon name="equaliser" size={15} />
        </button>

        <button
          type="button"
          className="tbtn"
          aria-label="Stop"
          title="Stop"
          disabled={!hasTrack}
          onClick={() => void stop()}
        >
          <Icon name="stop" size={14} />
        </button>
      </div>
    </footer>
  );
}
