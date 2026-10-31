import { useState } from "react";

import { Icon } from "../components/Icon";
import { SmartRuleBuilder } from "../components/SmartRuleBuilder";
import { backend } from "../lib/ipc";
import { formatDate } from "../lib/format";
import type { Playlist, SmartRules } from "../lib/types";
import { describe, useStore } from "../state/store";

export function PlaylistsView() {
  const { playlists, refresh, notify, setView } = useStore();
  const [name, setName] = useState("");
  const [smart, setSmart] = useState(false);
  const [editing, setEditing] = useState<Playlist | null>(null);

  async function afterWrite(playlistId?: number) {
    await refresh();
    if (playlistId !== undefined) setView({ kind: "playlist", id: playlistId });
  }

  async function createManual() {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      const id = await backend.createPlaylist(trimmed, "manual");
      setName("");
      notify(`Created “${trimmed}”`);
      await afterWrite(id);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function createSmart(rules: SmartRules) {
    const trimmed = name.trim();
    if (!trimmed) {
      notify("Name the playlist before saving its rules.", "error");
      return;
    }
    try {
      const id = await backend.createPlaylist(trimmed, "smart", rules);
      setName("");
      setSmart(false);
      notify(`Created “${trimmed}”`);
      await afterWrite(id);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function saveRules(playlist: Playlist, rules: SmartRules) {
    try {
      await backend.updateSmartRules(playlist.id, rules);
      setEditing(null);
      notify(`Updated the rules for “${playlist.name}”`);
      await afterWrite(playlist.id);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function remove(playlist: Playlist) {
    try {
      await backend.deletePlaylist(playlist.id);
      notify(`Deleted “${playlist.name}” — the tracks are untouched`);
      if (editing?.id === playlist.id) setEditing(null);
      await afterWrite();
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  return (
    <>
      <div className="view-head">
        <h1 className="view-title">Playlists</h1>
        <div className="view-tools">
          <span className="view-meta">
            {playlists.length} {playlists.length === 1 ? "playlist" : "playlists"}
          </span>
        </div>
      </div>

      <div className="view-body">
        <div className="panel">
          <section className="panel-section">
            <h2 className="panel-title">New playlist</h2>

            <div className="field-row">
              <label className="field">
                <span className="field-label">Name</span>
                <input
                  className="input"
                  value={name}
                  placeholder="Late listening"
                  onChange={(event) => setName(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && !smart) void createManual();
                  }}
                />
              </label>

              <label className="field">
                <span className="field-label">Kind</span>
                <select
                  className="select"
                  value={smart ? "smart" : "manual"}
                  onChange={(event) => setSmart(event.target.value === "smart")}
                >
                  <option value="manual">Manual — you add the tracks</option>
                  <option value="smart">Smart — rules pick the tracks</option>
                </select>
              </label>
            </div>

            {smart ? (
              <SmartRuleBuilder submitLabel="Create smart playlist" onSubmit={createSmart} />
            ) : (
              <div className="actions">
                <button
                  type="button"
                  className="button button-primary"
                  disabled={name.trim().length === 0}
                  onClick={() => void createManual()}
                >
                  Create playlist
                </button>
                <span className="hint">
                  Add tracks from the library with the row menu once it exists.
                </span>
              </div>
            )}
          </section>

          {editing ? (
            <section className="panel-section">
              <h2 className="panel-title">Rules for “{editing.name}”</h2>
              <SmartRuleBuilder
                key={editing.id}
                initial={editing.rules}
                submitLabel="Save rules"
                onSubmit={(rules) => void saveRules(editing, rules)}
                onCancel={() => setEditing(null)}
              />
            </section>
          ) : null}

          <section className="panel-section">
            <h2 className="panel-title">Saved playlists</h2>

            {playlists.length === 0 ? (
              <p className="hint">
                A manual playlist is an ordered list you build yourself. A smart playlist is a
                saved query over the index, so it keeps growing as the library does.
              </p>
            ) : (
              <div role="list" aria-label="Playlists">
                {playlists.map((playlist) => (
                  <div className="group-row group-row--manage" role="listitem" key={playlist.id}>
                    <button
                      type="button"
                      className="group-open"
                      onClick={() => setView({ kind: "playlist", id: playlist.id })}
                    >
                      <span className="group-name">{playlist.name}</span>
                      <span className="group-sub">
                        {playlist.kind === "smart" ? "Smart" : "Manual"} · updated{" "}
                        {formatDate(playlist.updatedAt)}
                      </span>
                    </button>

                    <span className="cell-num">{playlist.trackCount}</span>

                    <span className="manage-slot">
                      {playlist.kind === "smart" ? (
                        <button
                          type="button"
                          className="tbtn"
                          aria-label={`Edit the rules for ${playlist.name}`}
                          title="Edit rules"
                          onClick={() => setEditing(playlist)}
                        >
                          <Icon name="edit" size={13} />
                        </button>
                      ) : null}
                    </span>

                    <button
                      type="button"
                      className="tbtn"
                      aria-label={`Delete ${playlist.name}`}
                      title="Delete playlist"
                      onClick={() => void remove(playlist)}
                    >
                      <Icon name="trash" size={13} />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      </div>
    </>
  );
}
