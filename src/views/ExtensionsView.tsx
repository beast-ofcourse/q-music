import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";

import { Icon } from "../components/Icon";
import { ExtensionCard } from "../components/extensions/ExtensionCard";
import { ExtensionSettingsSheet } from "../components/extensions/ExtensionSettingsSheet";
import { LocalExtensionCard } from "../components/extensions/LocalExtensionCard";
import { ProvidersPanel } from "../components/extensions/ProvidersPanel";
import type { InstalledAction } from "../components/extensions/InstalledControls";
import { SettingsLauncherProvider } from "../components/extensions/settings-launcher";
import { backend } from "../lib/ipc";
import {
  toExtensionFailure,
  type BrokenExtension,
  type CatalogueSnapshot,
  type InstallProgress,
} from "../lib/extensions";
import "../styles/extensions-catalogue.css";
import "../styles/extensions-providers.css";

/** Long enough to see a completed bar settle before the installed controls replace it. */
const DONE_VISIBLE_MS = 1400;
const SKELETON_CARDS = 6;

type ExtensionsTab = "catalogue" | "providers";

const EXTENSION_TABS: { id: ExtensionsTab; label: string }[] = [
  { id: "catalogue", label: "Catalogue" },
  { id: "providers", label: "Providers" },
];

function withoutKey<T>(record: Record<string, T>, key: string): Record<string, T> {
  if (!(key in record)) {
    return record;
  }
  const next = { ...record };
  delete next[key];
  return next;
}

export function ExtensionsView() {
  const [snapshot, setSnapshot] = useState<CatalogueSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [readFailure, setReadFailure] = useState<string | null>(null);
  const [streamFailure, setStreamFailure] = useState<string | null>(null);
  const [progressById, setProgressById] = useState<Record<string, InstallProgress>>({});
  const [noticeById, setNoticeById] = useState<Record<string, string>>({});
  const [busyById, setBusyById] = useState<Record<string, InstalledAction>>({});
  const [fileInstalling, setFileInstalling] = useState(false);
  const [fileFailure, setFileFailure] = useState<string | null>(null);
  const [settingsFor, setSettingsFor] = useState<string | null>(null);
  const [tab, setTab] = useState<ExtensionsTab>("catalogue");

  const progressTimers = useRef<Set<number>>(new Set());
  const pendingFocusId = useRef<string | null>(null);

  const openSettings = useCallback((id: string) => {
    pendingFocusId.current = id;
    setSettingsFor(id);
  }, []);

  const closeSettings = useCallback(() => {
    setSettingsFor(null);
  }, []);

  /** Roving tabindex: only the selected tab is in the tab order, arrows move between them. */
  function selectTab(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    let nextIndex: number;
    if (event.key === "ArrowRight") {
      nextIndex = (index + 1) % EXTENSION_TABS.length;
    } else if (event.key === "ArrowLeft") {
      nextIndex = (index - 1 + EXTENSION_TABS.length) % EXTENSION_TABS.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = EXTENSION_TABS.length - 1;
    } else {
      return;
    }
    event.preventDefault();
    const nextTab = EXTENSION_TABS[nextIndex];
    if (nextTab === undefined) {
      return;
    }
    setTab(nextTab.id);
    document.getElementById(`ext-tab-${nextTab.id}`)?.focus();
  }

  const readCatalogue = useCallback(async () => {
    setRefreshing(true);
    try {
      const next = await backend.extensionsSnapshot();
      setSnapshot(next);
      setReadFailure(null);
    } catch (error) {
      setReadFailure(toExtensionFailure(error).message);
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  const forgetProgress = useCallback((id: string) => {
    setProgressById((current) => withoutKey(current, id));
  }, []);

  useEffect(() => {
    void readCatalogue();
  }, [readCatalogue]);

  useEffect(() => {
    let stop: (() => void) | null = null;
    let cancelled = false;
    const timers = progressTimers.current;

    const onProgress = (progress: InstallProgress) => {
      if (progress.phase === "failed") {
        forgetProgress(progress.id);
        setNoticeById((current) => ({
          ...current,
          [progress.id]: progress.message ?? "The install failed without a message.",
        }));
        return;
      }

      setProgressById((current) => ({ ...current, [progress.id]: progress }));

      if (progress.phase !== "done") {
        return;
      }
      const timer = window.setTimeout(() => {
        timers.delete(timer);
        forgetProgress(progress.id);
      }, DONE_VISIBLE_MS);
      timers.add(timer);
    };

    const subscribe = async () => {
      try {
        const unlisten = await backend.onExtensionProgress(onProgress);
        if (cancelled) {
          unlisten();
          return;
        }
        stop = unlisten;
      } catch (error) {
        if (!cancelled) {
          setStreamFailure(toExtensionFailure(error).message);
        }
      }
    };
    void subscribe();

    return () => {
      cancelled = true;
      stop?.();
      timers.forEach((timer) => window.clearTimeout(timer));
      timers.clear();
    };
  }, [forgetProgress]);

  function clearNotice(id: string) {
    setNoticeById((current) => withoutKey(current, id));
  }

  // Closing the sheet remounts the catalogue, so the trigger that opened it is a new node.
  // Focus is restored by its data attribute once that commit has happened.
  useEffect(() => {
    if (settingsFor !== null) return;
    const id = pendingFocusId.current;
    if (id === null) return;
    pendingFocusId.current = null;
    const trigger = [
      ...document.querySelectorAll<HTMLButtonElement>("[data-settings-trigger]"),
    ].find((button) => button.dataset.settingsTrigger === id);
    trigger?.focus();
  }, [settingsFor]);

  async function install(id: string) {
    clearNotice(id);
    setProgressById((current) => ({
      ...current,
      [id]: { id, phase: "queued", percent: 0, message: null },
    }));
    try {
      await backend.installExtension(id);
      await readCatalogue();
    } catch (error) {
      forgetProgress(id);
      setNoticeById((current) => ({
        ...current,
        [id]: toExtensionFailure(error).message,
      }));
    }
  }

  async function runInstalledAction(
    id: string,
    action: InstalledAction,
    run: () => Promise<unknown>,
  ) {
    clearNotice(id);
    setBusyById((current) => ({ ...current, [id]: action }));
    try {
      await run();
      await readCatalogue();
    } catch (error) {
      setNoticeById((current) => ({
        ...current,
        [id]: toExtensionFailure(error).message,
      }));
    } finally {
      setBusyById((current) => withoutKey(current, id));
    }
  }

  async function installFromFile() {
    setFileInstalling(true);
    setFileFailure(null);
    try {
      const installed = await backend.installExtensionFromFile();
      if (installed === null) {
        return;
      }
      await readCatalogue();
    } catch (error) {
      setFileFailure(toExtensionFailure(error).message);
    } finally {
      setFileInstalling(false);
    }
  }

  const entries = snapshot?.entries ?? [];
  const installed = snapshot?.installed ?? [];
  const broken = snapshot?.broken ?? [];
  const installedById = new Map(installed.map((extension) => [extension.id, extension]));
  const publishedIds = new Set(entries.map((entry) => entry.id));
  const localPackages = installed.filter((extension) => !publishedIds.has(extension.id));
  const catalogueFailure = snapshot?.refreshError ?? readFailure;
  const summary = snapshot
    ? `${entries.length} published, ${installed.length} installed`
    : null;
  const emptyCopy =
    catalogueFailure === null
      ? {
          title: "No published packages",
          body:
            "This registry lists no published extensions. Any package already installed " +
            "is still listed here, and a package file can still be installed from this machine.",
          action: "Refresh",
        }
      : {
          title: "The catalogue could not be read",
          body: catalogueFailure,
          action: "Try again",
        };

  const localCards = localPackages.map((extension) => (
    <LocalExtensionCard
      key={extension.id}
      installed={extension}
      busyAction={busyById[extension.id] ?? null}
      notice={noticeById[extension.id]}
      onUninstall={() =>
        void runInstalledAction(extension.id, "uninstall", () =>
          backend.uninstallExtension(extension.id),
        )
      }
      onSetEnabled={(enabled) =>
        void runInstalledAction(extension.id, enabled ? "enable" : "disable", () =>
          backend.setExtensionEnabled(extension.id, enabled),
        )
      }
    />
  ));

  if (settingsFor !== null) {
    return (
      <ExtensionSettingsSheet extensionId={settingsFor} onClose={closeSettings} />
    );
  }

  return (
    <SettingsLauncherProvider launch={openSettings}>
      <div className="view-head ext-head">
        <div className="ext-head-text">
          <h1 className="view-title">Extensions</h1>
          <p className="ext-sub">
            Published provider packages. Installing one adds a metadata, download or lyrics
            provider to this host.
          </p>
        </div>
        {tab === "catalogue" ? (
          <div className="view-tools">
            {summary ? <span className="view-meta">{summary}</span> : null}
            <button
              type="button"
              className="button"
              disabled={fileInstalling}
              onClick={() => void installFromFile()}
            >
              <Icon name="plus" size={14} />
              {fileInstalling ? "Reading package..." : "Install from file..."}
            </button>
            <button
              type="button"
              className="button button-quiet"
              disabled={refreshing}
              onClick={() => void readCatalogue()}
            >
              {refreshing ? "Refreshing..." : "Refresh"}
            </button>
          </div>
        ) : null}
      </div>

      <div className="ext-tabs" role="tablist" aria-label="Extension platform">
        {EXTENSION_TABS.map((candidate, index) => (
          <button
            key={candidate.id}
            type="button"
            role="tab"
            id={`ext-tab-${candidate.id}`}
            className="ext-tab"
            aria-selected={tab === candidate.id}
            aria-controls={`ext-panel-${candidate.id}`}
            tabIndex={tab === candidate.id ? 0 : -1}
            onClick={() => setTab(candidate.id)}
            onKeyDown={(event) => selectTab(event, index)}
          >
            {candidate.label}
          </button>
        ))}
      </div>

      <div
        className="view-body"
        role="tabpanel"
        id="ext-panel-catalogue"
        aria-labelledby="ext-tab-catalogue"
        hidden={tab !== "catalogue"}
      >
        <div className="ext-body">
          {catalogueFailure !== null && entries.length > 0 ? (
            <div className="ext-banner" role="status">
              <p className="ext-banner-text">
                <strong className="ext-banner-label">Refresh failed.</strong> {catalogueFailure}{" "}
                The cards below are the last catalogue this host read.
              </p>
              <button
                type="button"
                className="button"
                disabled={refreshing}
                onClick={() => void readCatalogue()}
              >
                {refreshing ? "Trying..." : "Retry"}
              </button>
            </div>
          ) : null}

          {streamFailure !== null ? (
            <div className="ext-banner" data-tone="error" role="alert">
              <p className="ext-banner-text">
                <strong className="ext-banner-label">Install progress is unavailable.</strong>{" "}
                {streamFailure}
              </p>
            </div>
          ) : null}

          {fileFailure !== null ? (
            <div className="ext-banner" data-tone="error" role="alert">
              <p className="ext-banner-text">
                <strong className="ext-banner-label">The package was not installed.</strong>{" "}
                {fileFailure}
              </p>
              <button
                type="button"
                className="button"
                onClick={() => setFileFailure(null)}
              >
                Dismiss
              </button>
            </div>
          ) : null}

          {broken.length > 0 ? <BrokenList broken={broken} /> : null}

          {loading && snapshot === null ? (
            <>
              <div className="ext-grid" aria-hidden="true">
                {Array.from({ length: SKELETON_CARDS }, (_, index) => (
                  <div key={index} className="ext-skeleton" />
                ))}
              </div>
              <p className="sr-only" role="status">
                Reading the published catalogue.
              </p>
            </>
          ) : entries.length === 0 ? (
            <>
              <div className="ext-empty">
                <p className="empty-title">{emptyCopy.title}</p>
                <p className="empty-body">{emptyCopy.body}</p>
                <button
                  type="button"
                  className="button"
                  disabled={refreshing}
                  onClick={() => void readCatalogue()}
                >
                  {refreshing ? "Trying..." : emptyCopy.action}
                </button>
              </div>
              {localCards.length > 0 ? (
                <div className="ext-grid">{localCards}</div>
              ) : null}
            </>
          ) : (
            <div className="ext-grid">
              {entries.map((entry) => (
                <ExtensionCard
                  key={entry.id}
                  entry={entry}
                  installed={installedById.get(entry.id)}
                  progress={progressById[entry.id]}
                  busyAction={busyById[entry.id] ?? null}
                  notice={noticeById[entry.id]}
                  onInstall={() => void install(entry.id)}
                  onUninstall={() =>
                    void runInstalledAction(entry.id, "uninstall", () =>
                      backend.uninstallExtension(entry.id),
                    )
                  }
                  onSetEnabled={(enabled) =>
                    void runInstalledAction(entry.id, enabled ? "enable" : "disable", () =>
                      backend.setExtensionEnabled(entry.id, enabled),
                    )
                  }
                />
              ))}
              {localCards}
            </div>
          )}
        </div>
      </div>

      <div
        className="view-body"
        role="tabpanel"
        id="ext-panel-providers"
        aria-labelledby="ext-tab-providers"
        hidden={tab !== "providers"}
      >
        <ProvidersPanel />
      </div>
    </SettingsLauncherProvider>
  );
}

function BrokenList({ broken }: { broken: BrokenExtension[] }) {
  return (
    <section className="ext-broken" aria-labelledby="ext-broken-title">
      <h2 className="ext-section-title" id="ext-broken-title">
        Folders on disk that cannot be served
      </h2>
      <ul className="ext-broken-list">
        {broken.map((extension) => (
          <li key={extension.id} className="ext-broken-row">
            <div className="ext-broken-head">
              <span className="ext-broken-id">{extension.id}</span>
              <span className="ext-broken-path" title={extension.path}>
                {extension.path}
              </span>
            </div>
            <p className="ext-broken-reason">{extension.reason}</p>
          </li>
        ))}
      </ul>
    </section>
  );
}
