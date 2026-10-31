import type { InstalledExtension } from "../../lib/extensions";
import { InstalledControls, type InstalledAction } from "./InstalledControls";

interface LocalExtensionCardProps {
  installed: InstalledExtension;
  busyAction: InstalledAction | null;
  notice: string | undefined;
  onUninstall: () => void;
  onSetEnabled: (enabled: boolean) => void;
}

/**
 * A package installed from a file has no catalogue entry, so it has no status or
 * published version to compare. It still needs the same controls as any other install.
 */
export function LocalExtensionCard({
  installed,
  busyAction,
  notice,
  onUninstall,
  onSetEnabled,
}: LocalExtensionCardProps) {
  const name = installed.displayName ?? installed.id;
  const headingId = `ext-local-name-${installed.id}`;

  return (
    <article className="ext-card" aria-labelledby={headingId} data-installed="true" data-local="true">
      <div className="ext-card-head">
        <span className="ext-badge">Local package</span>
      </div>

      <div className="ext-identity">
        <h2 className="ext-name" id={headingId}>
          {name}
        </h2>
        <p className="ext-id">{installed.id}</p>
      </div>

      <p className="ext-desc">
        Installed from a package file on this machine; it is not in the published catalogue.
      </p>

      <div className="ext-card-foot">
        {notice !== undefined ? (
          <p className="ext-notice" role="alert">
            {notice}
          </p>
        ) : null}
        <InstalledControls
          name={name}
          installed={installed}
          busyAction={busyAction}
          onSetEnabled={onSetEnabled}
          onUninstall={onUninstall}
        />
      </div>
    </article>
  );
}
