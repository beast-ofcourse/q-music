import { Icon } from "../Icon";
import type {
  CatalogueEntry,
  EntryStatus,
  InstallProgress,
  InstalledExtension,
} from "../../lib/extensions";
import { InstallProgressBar } from "./InstallProgressBar";
import { InstalledControls, type InstalledAction } from "./InstalledControls";
import { compareVersions } from "./version-order";

interface ExtensionCardProps {
  entry: CatalogueEntry;
  installed: InstalledExtension | undefined;
  progress: InstallProgress | undefined;
  busyAction: InstalledAction | null;
  notice: string | undefined;
  onInstall: () => void;
  onUninstall: () => void;
  onSetEnabled: (enabled: boolean) => void;
}

/** One published package, including the reason it cannot be installed if it cannot. */
export function ExtensionCard({
  entry,
  installed,
  progress,
  busyAction,
  notice,
  onInstall,
  onUninstall,
  onSetEnabled,
}: ExtensionCardProps) {
  const name = entry.displayName ?? entry.name;
  const headingId = `ext-name-${entry.id}`;
  const standing = hostStanding(entry);
  const installedVersionNote = installed ? versionNote(installed, entry) : null;

  return (
    <article
      className="ext-card"
      aria-labelledby={headingId}
      data-installed={installed ? "true" : "false"}
    >
      <div className="ext-card-head">
        <span className="ext-badge">{categoryLabel(entry.category)}</span>
        <span className="ext-version">v{entry.version}</span>
      </div>

      <div className="ext-identity">
        <h2 className="ext-name" id={headingId}>
          {name}
        </h2>
        <p className="ext-id">{entry.id}</p>
      </div>

      <p className="ext-desc">
        {entry.description ?? "No description was published for this package."}
      </p>

      {entry.missingFeatures.length > 0 ? (
        <ul className="ext-missing" aria-label="Features this host does not implement">
          {entry.missingFeatures.map((feature) => (
            <li key={feature} className="ext-missing-line">
              Needs <code className="ext-feature">{feature}</code>, which this host does not
              implement.
            </li>
          ))}
        </ul>
      ) : null}

      <div className="ext-card-foot">
        {notice !== undefined && progress === undefined ? (
          <p className="ext-notice" role="alert">
            {notice}
          </p>
        ) : null}

        {progress ? <InstallProgressBar name={name} progress={progress} /> : null}

        {installed ? (
          <>
            {installedVersionNote ? (
              <p className="ext-version-note">{installedVersionNote}</p>
            ) : null}
            <InstalledControls
              name={name}
              installed={installed}
              busyAction={busyAction}
              onSetEnabled={onSetEnabled}
              onUninstall={onUninstall}
            />
          </>
        ) : progress ? null : (
          <StandingControl standing={standing} name={name} onInstall={onInstall} />
        )}
      </div>
    </article>
  );
}

interface StandingControlProps {
  standing: HostStanding;
  name: string;
  onInstall: () => void;
}

function StandingControl({ standing, name, onInstall }: StandingControlProps) {
  if (standing.kind === "refused") {
    return (
      <div className="ext-refusal">
        <span className="ext-refusal-label">Not installable</span>
        <p className="ext-refusal-text">{standing.reason}</p>
      </div>
    );
  }
  if (standing.kind === "unevaluated") {
    return <p className="ext-pending">This host has not evaluated the package yet.</p>;
  }
  return (
    <button
      type="button"
      className="button ext-install"
      aria-label={`Install ${name}`}
      onClick={onInstall}
    >
      <Icon name="arrow-down" size={14} />
      Install
    </button>
  );
}

type HostStanding =
  | { kind: "installable" }
  | { kind: "refused"; reason: string }
  | { kind: "unevaluated" };

function hostStanding(entry: CatalogueEntry): HostStanding {
  if (entry.missingFeatures.length > 0) {
    return { kind: "refused", reason: featureRefusal(entry.missingFeatures) };
  }
  const status = entry.status;
  if (status.state === "supported") {
    return { kind: "installable" };
  }
  if (status.state === "not_evaluated") {
    return { kind: "unevaluated" };
  }
  return { kind: "refused", reason: refusalText(status) };
}

/** The list above already names them; the footer only states the consequence. */
function featureRefusal(features: string[]): string {
  return `This host does not implement ${features.join(", ")}.`;
}

/** Every refusal is built from the fields the registry supplied, never per entry. */
function refusalText(status: Extract<EntryStatus, { state: "unsupported" }>): string {
  switch (status.reason) {
    case "host_too_old":
      return `Needs Q-Music ${status.required} or newer, this host is ${status.host}.`;
    case "invalid_version":
      return `The registry's ${readableField(status.field)} field reads "${status.value}", which is not a version.`;
    case "invalid_checksum":
      return "The registry's digest is not a usable SHA-256, so the package cannot be verified.";
  }
}

function readableField(field: string): string {
  if (field === "min_app_version") {
    return "minimum app version";
  }
  return field.replace(/_/g, " ");
}

function categoryLabel(category: string | null): string {
  if (category === null || category.trim() === "") {
    return "Uncategorised";
  }
  return category
    .split(/[_-]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

function versionNote(installed: InstalledExtension, entry: CatalogueEntry): string | null {
  if (installed.version === entry.version) {
    return null;
  }
  const order = compareVersions(installed.version, entry.version);
  if (order > 0) {
    return `The installed copy is newer: ${installed.version}, against the published ${entry.version}.`;
  }
  if (order < 0) {
    return `The installed copy is older: ${installed.version}, against the published ${entry.version}.`;
  }
  return `The installed copy is ${installed.version}; the published release is ${entry.version}.`;
}
