import { Icon } from "./Icon";
import { useArtwork } from "../lib/artwork";
import { techLine } from "../lib/format";
import { DownloadsPane } from "./DownloadsPane";
import { LevelPlot } from "./LevelPlot";
import { LyricsPane } from "./LyricsPane";
import { QueuePane } from "./QueuePane";
import { useStore, type NowTab } from "../state/store";

const TABS: Array<{ id: NowTab; label: string }> = [
  { id: "queue", label: "Queue" },
  { id: "lyrics", label: "Lyrics" },
  { id: "downloads", label: "Downloads" },
];

/**
 * The downloads pane subscribes to a live job feed as soon as it mounts, so it is only
 * mounted while the panel is open on that tab. Closed panels should not keep a timer or
 * a subscription alive for a list nobody is reading.
 */
function NowPane({ tab, open }: { tab: NowTab; open: boolean }) {
  if (tab === "queue") {
    return <QueuePane />;
  }
  if (tab === "lyrics") {
    return <LyricsPane />;
  }
  return open ? <DownloadsPane /> : null;
}

export function NowPanel() {
  const { currentTrack, selectedTrack, snapshot, nowTab, nowOpen, setNowTab, closeNow, setView } =
    useStore();
  const artwork = useArtwork(currentTrack?.artId ?? null);
  const tech = currentTrack ? techLine(currentTrack) : [];
  const editable = selectedTrack ?? currentTrack;

  return (
    <aside className="now" aria-label="Now playing" aria-hidden={!nowOpen} inert={!nowOpen}>
      <div className="cover">
        {artwork ? (
          <img src={artwork} alt="" />
        ) : (
          <span className="cover-empty">
            <Icon name="disc" size={22} />
            {currentTrack ? "No artwork" : "Idle"}
          </span>
        )}
        <button
          type="button"
          className="now-close"
          aria-label="Hide the now-playing panel"
          onClick={closeNow}
        >
          <Icon name="close" size={14} />
        </button>
      </div>

      <div className="now-meta">
        <h2 className="now-title">{currentTrack?.title ?? "Nothing playing"}</h2>
        <p className="now-artist">
          {currentTrack
            ? `${currentTrack.artist}${currentTrack.album ? ` · ${currentTrack.album}` : ""}`
            : "Pick a row in the library to begin"}
        </p>
        {tech.length > 0 ? (
          <p className="now-tech">
            {tech.map((part) => (
              <span key={part}>{part}</span>
            ))}
          </p>
        ) : null}
        {editable ? (
          <button
            type="button"
            className="button button-quiet now-edit"
            onClick={() => setView({ kind: "editor" })}
          >
            <Icon name="edit" size={12} />
            Edit tags
          </button>
        ) : null}
      </div>

      <LevelPlot snapshot={snapshot} />

      <div className="now-lower">
        <div className="now-tabs" role="tablist" aria-label="Now playing detail">
          {TABS.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              id={`now-tab-${tab.id}`}
              className="now-tab"
              aria-selected={nowTab === tab.id}
              aria-controls={`now-pane-${tab.id}`}
              onClick={() => setNowTab(tab.id)}
            >
              {tab.label}
            </button>
          ))}
        </div>

        <div
          className="now-pane"
          role="tabpanel"
          id={`now-pane-${nowTab}`}
          aria-labelledby={`now-tab-${nowTab}`}
        >
          <NowPane tab={nowTab} open={nowOpen} />
        </div>
      </div>
    </aside>
  );
}
