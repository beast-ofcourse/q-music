import { AddFolderButton } from "../components/AddFolderButton";
import { backend } from "../lib/ipc";
import { formatBytes, formatDate, formatSpan, plural } from "../lib/format";
import { PLAYBACK_SPEEDS } from "../lib/playback";
import type { ThemeName } from "../state/store";
import { describe, useStore } from "../state/store";

const THEMES: Array<{ value: ThemeName; label: string }> = [
  { value: "carbon", label: "Carbon — matte black, bone ink" },
  { value: "paper", label: "Paper — warm light" },
  { value: "stage", label: "Stage — blue black" },
  { value: "apple", label: "Apple — clean light, blue accent" },
  { value: "glass", label: "Glass — frosted dark, purple accent" },
  { value: "catppuccin-mocha", label: "Catppuccin Mocha — warm dark" },
  { value: "sky-catppuccin", label: "Sky blue + Catppuccin white" },
  { value: "babypink-catppuccin", label: "Baby pink + Catppuccin white" },
];

const SHORTCUTS: Array<{ keys: string; action: string }> = [
  { keys: "Space", action: "Play or pause" },
  { keys: "← →", action: "Seek five seconds" },
  { keys: "Shift + ← →", action: "Seek thirty seconds" },
  { keys: "N / P", action: "Next or previous track" },
  { keys: "M", action: "Mute and restore" },
  { keys: "F", action: "Favourite the playing track" },
  { keys: "S", action: "Shuffle on or off" },
  { keys: "R", action: "Cycle repeat: off, queue, track" },
  { keys: "/", action: "Jump to the search field" },
  { keys: "Esc", action: "Close the now panel, or clear the search" },
];

export function SettingsView() {
  const { theme, setTheme, snapshot, devices, setSpeed, stats, notify, refresh } = useStore();

  const deviceNames = devices.map((device) => device.name);
  const activeDevice = snapshot.device || devices.find((device) => device.isDefault)?.name || "";
  const deviceOptions =
    activeDevice && !deviceNames.includes(activeDevice)
      ? [activeDevice, ...deviceNames]
      : deviceNames;

  async function changeDevice(name: string) {
    try {
      await backend.setOutputDevice(name);
      notify(`Output moved to ${name}`);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function rescan() {
    try {
      await backend.rescanLibrary();
      notify("Rescanning every indexed folder");
      await refresh();
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  return (
    <>
      <div className="view-head">
        <h1 className="view-title">Settings</h1>
      </div>

      <div className="view-body">
        <div className="panel">
          <section className="panel-section">
            <h2 className="panel-title">Appearance</h2>
            <label className="field">
              <span className="field-label">Theme</span>
              <select
                className="select"
                value={theme}
                onChange={(event) => setTheme(event.target.value as ThemeName)}
              >
                {THEMES.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
            <p className="hint">
              The theme swaps the token block only: ground, ink, one signal colour. Contrast is
              held at 4.5:1 or better in all three.
            </p>
          </section>

          <section className="panel-section">
            <h2 className="panel-title">Playback</h2>

            <div className="field-row">
              <label className="field">
                <span className="field-label">Output device</span>
                <select
                  className="select"
                  value={activeDevice}
                  disabled={deviceOptions.length === 0}
                  onChange={(event) => void changeDevice(event.target.value)}
                >
                  {deviceOptions.length === 0 ? <option value="">No device found</option> : null}
                  {deviceOptions.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              </label>

              <label className="field">
                <span className="field-label">Speed</span>
                <select
                  className="select"
                  value={snapshot.speed}
                  onChange={(event) => setSpeed(Number(event.target.value))}
                >
                  {PLAYBACK_SPEEDS.map((speed) => (
                    <option key={speed} value={speed}>
                      {speed}× {speed === 1 ? "— normal" : ""}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            <p className="hint">
              Speed resamples the tape, so pitch follows the tempo. The next track is queued in
              the engine before the current one ends, which is what keeps gaps out of an album.
            </p>
          </section>

          <section className="panel-section">
            <h2 className="panel-title">Library</h2>

            {stats ? (
              <div className="kv">
                <span>
                  {stats.tracks.toLocaleString()} {plural(stats.tracks, "track")} across{" "}
                  {stats.albums.toLocaleString()} {plural(stats.albums, "album")}
                </span>
                <b>
                  {formatSpan(stats.totalDurationMs)} · {formatBytes(stats.totalBytes)}
                </b>
              </div>
            ) : null}

            <div className="kv">
              <span>Last scan</span>
              <b>{formatDate(stats?.lastScanAt ?? null)}</b>
            </div>

            <div className="actions">
              <AddFolderButton />
              <button type="button" className="button" onClick={() => void rescan()}>
                Rescan everything
              </button>
            </div>
          </section>

          <section className="panel-section">
            <h2 className="panel-title">Keyboard</h2>
            <div className="shortcuts">
              {SHORTCUTS.map((shortcut) => (
                <div className="shortcut-row" key={shortcut.keys}>
                  <kbd>{shortcut.keys}</kbd>
                  <span>{shortcut.action}</span>
                </div>
              ))}
            </div>
          </section>

          <section className="panel-section">
            <h2 className="panel-title">About</h2>
            <div className="kv">
              <span>Q-Music</span>
              <b>{__APP_VERSION__}</b>
            </div>
            <div className="kv">
              <span>Engine</span>
              <b>qcore — Rust, Symphonia, CPAL</b>
            </div>
            <div className="kv">
              <span>Typefaces</span>
              <b>Archivo, IBM Plex Mono (SIL OFL)</b>
            </div>
            <p className="hint">
              Q for quantum: the player treats playback as one state that changes, not a chain of
              windows. Everything on screen is read from that state.
            </p>
          </section>
        </div>
      </div>
    </>
  );
}
