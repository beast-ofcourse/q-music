import { useEffect, useRef, useState } from "react";

import { Icon } from "./Icon";
import { backend } from "../lib/ipc";
import type { Track } from "../lib/types";
import { describe, useStore } from "../state/store";

const MENU_WIDTH = 236;
/** Tall enough to keep the last item on screen; the menu itself never scrolls. */
const MENU_MAX_HEIGHT = 360;

interface RowMenuProps {
  track: Track;
  x: number;
  y: number;
  onClose: () => void;
}

/**
 * The row's secondary actions. It exists because a manual playlist is useless without a
 * way to put a track in one, and because tag editing belongs on the row as well as in
 * the now-playing panel.
 */
export function RowMenu({ track, x, y, onClose }: RowMenuProps) {
  const { playlists, addToPlaylist, select, setView, notify } = useStore();
  const menuRef = useRef<HTMLDivElement>(null);
  const [addingTo, setAddingTo] = useState<number | null>(null);

  useEffect(() => {
    const menu = menuRef.current;
    menu?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();

    function handlePointerDown(event: PointerEvent) {
      if (menu && event.target instanceof Node && !menu.contains(event.target)) onClose();
    }

    window.addEventListener("pointerdown", handlePointerDown, true);
    return () => window.removeEventListener("pointerdown", handlePointerDown, true);
  }, [onClose]);

  // Keep the menu inside the window even when the row sits at an edge.
  const left = Math.max(8, Math.min(x, window.innerWidth - MENU_WIDTH - 8));
  const top = Math.max(8, Math.min(y, window.innerHeight - MENU_MAX_HEIGHT));

  async function addTo(playlistId: number, name: string) {
    setAddingTo(playlistId);
    try {
      await addToPlaylist(playlistId, [track.id]);
      notify(`Added “${track.title || "Untitled"}” to ${name}`);
      onClose();
    } catch (error) {
      notify(describe(error), "error");
    } finally {
      setAddingTo(null);
    }
  }

  async function reveal() {
    try {
      await backend.revealTrack(track.path);
      onClose();
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  function moveFocus(step: 1 | -1) {
    const items = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
    if (items.length === 0) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = (current + step + items.length) % items.length;
    items[next]?.focus();
  }

  return (
    <div
      ref={menuRef}
      className="row-menu"
      role="menu"
      aria-label={`Actions for ${track.title || "this track"}`}
      style={{ left, top, width: MENU_WIDTH }}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation();
          onClose();
        } else if (event.key === "ArrowDown") {
          event.preventDefault();
          moveFocus(1);
        } else if (event.key === "ArrowUp") {
          event.preventDefault();
          moveFocus(-1);
        }
      }}
    >
      <p className="row-menu-label" role="presentation">Add to playlist</p>

      {playlists.length === 0 ? (
        <p className="row-menu-note">No playlists yet</p>
      ) : (
        playlists.map((playlist) => (
          <button
            key={playlist.id}
            type="button"
            role="menuitem"
            className="row-menu-item"
            disabled={addingTo !== null}
            onClick={() => void addTo(playlist.id, playlist.name)}
          >
            <Icon name="queue" size={13} />
            <span className="row-menu-text">{playlist.name}</span>
            <span className="row-menu-count">{playlist.trackCount}</span>
          </button>
        ))
      )}

      <div className="row-menu-rule" role="separator" />

      <button
        type="button"
        role="menuitem"
        className="row-menu-item"
        onClick={() => {
          select(track.id);
          setView({ kind: "editor" });
          onClose();
        }}
      >
        <Icon name="edit" size={13} />
        Edit tags
      </button>

      <button type="button" role="menuitem" className="row-menu-item" onClick={() => void reveal()}>
        <Icon name="folder" size={13} />
        Show file
      </button>
    </div>
  );
}
