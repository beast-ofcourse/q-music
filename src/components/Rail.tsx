import { Icon, type IconName } from "./Icon";
import { useAddFolder } from "./AddFolderButton";
import { backend } from "../lib/ipc";
import { formatBytes, formatSpan, plural } from "../lib/format";
import { useStore, type View } from "../state/store";

interface NavItem {
  label: string;
  icon: IconName;
  view: View;
  count?: string;
}

function folderName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function Rail() {
  const { view, stats, playlists, folders, setView, scan } = useStore();
  const { addFolder, busy: addingFolder } = useAddFolder();

  const collection: NavItem[] = [
    { label: "Home", icon: "home", view: { kind: "home" } },
    { label: "Library", icon: "library", view: { kind: "library" }, count: stats?.tracks.toLocaleString() },
    { label: "Recently played", icon: "clock", view: { kind: "recent" } },
    { label: "Favourites", icon: "heart", view: { kind: "favorites" } },
    { label: "Albums", icon: "disc", view: { kind: "albums" }, count: stats?.albums.toLocaleString() },
    { label: "Artists", icon: "artist", view: { kind: "artists" }, count: stats?.artists.toLocaleString() },
  ];

  const tools: NavItem[] = [
    { label: "Extensions", icon: "puzzle", view: { kind: "extensions" } },
    { label: "Equaliser", icon: "equaliser", view: { kind: "eq" } },
    { label: "Settings", icon: "settings", view: { kind: "settings" } },
  ];

  const isCurrent = (candidate: View) =>
    candidate.kind === view.kind &&
    ("id" in candidate ? candidate.id === (view as { id?: number }).id : true) &&
    ("name" in candidate ? candidate.name === (view as { name?: string }).name : true) &&
    ("path" in candidate ? candidate.path === (view as { path?: string }).path : true);

  return (
    <nav className="rail" aria-label="Library sections">
      <div className="rail-nav">
        <div className="rail-group">
          {collection.map((item) => (
            <button
              key={item.label}
              type="button"
              className="rail-item"
              aria-current={isCurrent(item.view) ? "page" : undefined}
              onClick={() => setView(item.view)}
              title={item.label}
            >
              <Icon name={item.icon} size={16} />
              <span className="label">{item.label}</span>
              {item.count ? <span className="count">{item.count}</span> : null}
            </button>
          ))}
        </div>

        <div className="rail-group">
          <button
            type="button"
            className="rail-label"
            aria-current={view.kind === "playlists" ? "page" : undefined}
            onClick={() => setView({ kind: "playlists" })}
          >
            Playlists
          </button>
          {playlists.length === 0 ? (
            <button
              type="button"
              className="rail-item"
              onClick={() => setView({ kind: "playlists" })}
            >
              <Icon name="plus" size={16} />
              <span className="label">Create the first</span>
            </button>
          ) : (
            playlists.map((playlist) => (
              <button
                key={playlist.id}
                type="button"
                className="rail-item"
                aria-current={isCurrent({ kind: "playlist", id: playlist.id }) ? "page" : undefined}
                onClick={() => setView({ kind: "playlist", id: playlist.id })}
                title={playlist.name}
              >
                <Icon name="queue" size={16} />
                <span className="label">{playlist.name}</span>
                <span className="count">{playlist.trackCount}</span>
              </button>
            ))
          )}
        </div>

        <div className="rail-group">
          <button
            type="button"
            className="rail-label"
            aria-current={view.kind === "folders" ? "page" : undefined}
            onClick={() => setView({ kind: "folders" })}
          >
            Folders
          </button>
          {folders.map((folder) => (
            <button
              key={folder.id}
              type="button"
              className="rail-item"
              aria-current={isCurrent({ kind: "folder", path: folder.path }) ? "page" : undefined}
              onClick={() => setView({ kind: "folder", path: folder.path })}
              title={folder.path}
            >
              <Icon name="folder" size={16} />
              <span className="label">{folderName(folder.path)}</span>
              <span className="count">{folder.trackCount}</span>
            </button>
          ))}
          <button
            type="button"
            className="rail-item"
            disabled={addingFolder}
            onClick={() => void addFolder()}
          >
            <Icon name="plus" size={16} />
            <span className="label">{addingFolder ? "Choosing…" : "Add folder"}</span>
          </button>
        </div>

        <div className="rail-group">
          {tools.map((item) => (
            <button
              key={item.label}
              type="button"
              className="rail-item"
              aria-current={isCurrent(item.view) ? "page" : undefined}
              onClick={() => setView(item.view)}
              title={item.label}
            >
              <Icon name={item.icon} size={16} />
              <span className="label">{item.label}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="rail-foot">
        {scan ? (
          <div className="scan-strip">
            <span>{scan.scanning ? "Indexing" : "Idle"}</span>
            <span className="scan-bar">
              <span />
            </span>
            <span>{scan.filesSeen}</span>
          </div>
        ) : null}

        {stats ? (
          <>
            <div className="rail-stat">
              <span>tracks</span>
              <b>{stats.tracks.toLocaleString()}</b>
            </div>
            <div className="rail-stat">
              <span>listening time</span>
              <b>{formatSpan(stats.totalDurationMs)}</b>
            </div>
            <div className="rail-stat">
              <span>on disk</span>
              <b>{formatBytes(stats.totalBytes)}</b>
            </div>
            <p className="rail-stat" style={{ display: "block", lineHeight: 1.4 }}>
              {stats.albums.toLocaleString()} {plural(stats.albums, "album")},{" "}
              {stats.artists.toLocaleString()} {plural(stats.artists, "artist")}
            </p>
          </>
        ) : null}

        {backend.kind === "demo" ? (
          <p className="synthetic-flag">
            Synthetic library — generated for interface review, not your files.
          </p>
        ) : null}
      </div>
    </nav>
  );
}
