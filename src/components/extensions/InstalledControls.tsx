import type { InstalledExtension } from "../../lib/extensions";
import { Icon } from "../Icon";
import { useSettingsLauncher } from "./settings-launcher";

export type InstalledAction = "enable" | "disable" | "uninstall";

interface InstalledControlsProps {
  name: string;
  installed: InstalledExtension;
  busyAction: InstalledAction | null;
  onSetEnabled: (enabled: boolean) => void;
  onUninstall: () => void;
}

/** The state line and the actions every installed package shares. */
export function InstalledControls({
  name,
  installed,
  busyAction,
  onSetEnabled,
  onUninstall,
}: InstalledControlsProps) {
  const launchSettings = useSettingsLauncher();
  const busy = busyAction !== null;
  const stateLabel = installed.enabled ? "Installed and enabled" : "Installed, disabled";
  const toggleLabel = installed.enabled ? "Disable" : "Enable";
  const toggleBusy = busyAction === (installed.enabled ? "disable" : "enable");

  return (
    <>
      <p className="ext-installed">
        <span
          className="ext-state-dot"
          data-enabled={installed.enabled ? "true" : "false"}
          aria-hidden="true"
        />
        <span>{stateLabel}</span>
        <span className="ext-installed-version">v{installed.version}</span>
      </p>
      <div className="ext-actions">
        {launchSettings === null ? null : (
          <button
            type="button"
            className="button"
            data-settings-trigger={installed.id}
            aria-label={`Settings for ${name}`}
            disabled={busy}
            onClick={() => launchSettings(installed.id)}
          >
            <Icon name="settings" size={14} />
            Settings
          </button>
        )}
        <button
          type="button"
          className="button"
          aria-label={`${toggleLabel} ${name}`}
          disabled={busy}
          onClick={() => onSetEnabled(!installed.enabled)}
        >
          {toggleBusy ? "Working..." : toggleLabel}
        </button>
        <button
          type="button"
          className="button button-danger"
          aria-label={`Uninstall ${name}`}
          disabled={busy}
          onClick={onUninstall}
        >
          {busyAction === "uninstall" ? "Removing..." : "Uninstall"}
        </button>
      </div>
    </>
  );
}
