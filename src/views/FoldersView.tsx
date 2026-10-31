import { AddFolderButton, useAddFolder } from "../components/AddFolderButton";
import { Icon } from "../components/Icon";
import { backend } from "../lib/ipc";
import { formatBytes, formatDate, formatSpan, plural } from "../lib/format";
import type { LibraryFolder } from "../lib/types";
import { describe, useStore } from "../state/store";

export function FoldersView() {
  const { folders, stats, refresh, notify, setView } = useStore();
  const { busy } = useAddFolder();

  async function rescan(folder: LibraryFolder) {
    try {
      await backend.scanFolder(folder.path);
      notify(`Rescanning ${folder.path}`);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function remove(folder: LibraryFolder) {
    try {
      await backend.removeFolder(folder.id);
      notify(`Removed ${folder.path} from the index`);
      await refresh();
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  return (
    <>
      <div className="view-head">
        <h1 className="view-title">Folders</h1>
        <div className="view-tools">
          <span className="view-meta">
            {folders.length} {plural(folders.length, "folder")}
          </span>
          <AddFolderButton label="Add folder" />
        </div>
      </div>

      <div className="view-body">
        <div className="panel">
          <section className="panel-section">
            <h2 className="panel-title">Indexed folders</h2>

            {folders.length === 0 ? (
              <div className="empty">
                <p className="empty-title">No folders indexed</p>
                <p className="empty-body">
                  Q-Music reads the files where they already are. Adding a folder indexes its
                  audio files; it never moves or renames anything.
                </p>
                <AddFolderButton />
              </div>
            ) : (
              <div role="list" aria-label="Indexed folders">
                {folders.map((folder) => (
                  <div className="group-row group-row--manage" role="listitem" key={folder.id}>
                    <button
                      type="button"
                      className="group-open"
                      onClick={() => setView({ kind: "folder", path: folder.path })}
                    >
                      <span className="group-name">{folder.path}</span>
                      <span className="group-sub">
                        {folder.trackCount} {plural(folder.trackCount, "track")} · added{" "}
                        {formatDate(folder.addedAt)}
                      </span>
                    </button>

                    <span className="manage-slot">
                      <button
                        type="button"
                        className="tbtn"
                        aria-label={`Rescan ${folder.path}`}
                        title="Rescan this folder"
                        disabled={busy}
                        onClick={() => void rescan(folder)}
                      >
                        <Icon name="clock" size={13} />
                      </button>
                    </span>

                    <button
                      type="button"
                      className="tbtn"
                      aria-label={`Remove ${folder.path} from the index`}
                      title="Remove from the index"
                      onClick={() => void remove(folder)}
                    >
                      <Icon name="trash" size={13} />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </section>

          {stats ? (
            <section className="panel-section">
              <h2 className="panel-title">Storage</h2>
              <div className="kv">
                <span>Tracks</span>
                <b>{stats.tracks.toLocaleString()}</b>
              </div>
              <div className="kv">
                <span>Listening time</span>
                <b>{formatSpan(stats.totalDurationMs)}</b>
              </div>
              <div className="kv">
                <span>Audio on disk</span>
                <b>{formatBytes(stats.totalBytes)}</b>
              </div>
              <div className="kv">
                <span>Last scan</span>
                <b>{formatDate(stats.lastScanAt)}</b>
              </div>
              <p className="hint">
                The index lives in a single SQLite file in the app data directory. Removing a
                folder drops its rows and its cached artwork, never the audio.
              </p>
            </section>
          ) : null}
        </div>
      </div>
    </>
  );
}
