import { useCallback, useState } from "react";

import { Icon } from "./Icon";
import { backend } from "../lib/ipc";
import { describe, useStore } from "../state/store";

/**
 * Adds a folder through the native picker, starts the scan and refreshes the window.
 * One implementation, because the rail, the empty state and settings all offer it.
 */
export function useAddFolder() {
  const { notify, refresh } = useStore();
  const [busy, setBusy] = useState(false);

  const addFolder = useCallback(async () => {
    setBusy(true);
    try {
      const path = await backend.pickFolder();
      if (!path) return;
      await backend.scanFolder(path);
      notify(`Indexing ${path}`);
      await refresh();
    } catch (error) {
      notify(describe(error), "error");
    } finally {
      setBusy(false);
    }
  }, [notify, refresh]);

  return { addFolder, busy };
}

interface AddFolderButtonProps {
  label?: string;
  quiet?: boolean;
}

export function AddFolderButton({ label = "Add folder", quiet = false }: AddFolderButtonProps) {
  const { addFolder, busy } = useAddFolder();

  return (
    <button
      type="button"
      className={quiet ? "button button-quiet" : "button"}
      disabled={busy}
      onClick={() => void addFolder()}
    >
      <Icon name="plus" size={12} />
      {busy ? "Choosing…" : label}
    </button>
  );
}
