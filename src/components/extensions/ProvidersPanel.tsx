import { useCallback, useEffect, useRef, useState } from "react";

import { backend } from "../../lib/ipc";
import {
  toExtensionFailure,
  type ProviderCapability,
  type ProviderGroup,
  type ProviderView,
} from "../../lib/extensions";
import { ProviderRow } from "./ProviderRow";
import { capabilityLabel, healthLabel } from "./provider-health";

function withoutKey(record: Record<string, string>, key: string): Record<string, string> {
  if (!(key in record)) {
    return record;
  }
  const next = { ...record };
  delete next[key];
  return next;
}

function swap(providers: ProviderView[], from: number, to: number): ProviderView[] {
  const moved = providers[from];
  const displaced = providers[to];
  if (moved === undefined || displaced === undefined) {
    return providers;
  }
  const next = [...providers];
  next[from] = displaced;
  next[to] = moved;
  return next;
}

function withProvider(
  groups: ProviderGroup[],
  id: string,
  update: (provider: ProviderView) => ProviderView,
): ProviderGroup[] {
  return groups.map((group) => ({
    ...group,
    providers: group.providers.map((provider) => (provider.id === id ? update(provider) : provider)),
  }));
}

export function ProvidersPanel() {
  const [groups, setGroups] = useState<ProviderGroup[] | null>(null);
  const [readFailure, setReadFailure] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [noticeById, setNoticeById] = useState<Record<string, string>>({});
  const [defaultProvider, setDefaultProvider] = useState<string | null>(
    typeof localStorage !== "undefined" ? localStorage.getItem("qmusic.defaultSearchProvider") : null,
  );
  const [announcement, setAnnouncement] = useState("");

  const rowRefs = useRef(new Map<string, HTMLLIElement>());
  const pendingFocus = useRef<string | null>(null);

  const load = useCallback(async () => {
    setReadFailure(null);
    try {
      setGroups(await backend.providers());
    } catch (error) {
      setReadFailure(toExtensionFailure(error).message);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // React keeps the row's DOM node because the key is the provider id, so focus would
  // normally survive a reorder. It does not survive the case that matters: a provider
  // moved to the top has its "up" button disabled, and a disabled button drops focus.
  // Restoring here puts focus back on the row itself.
  useEffect(() => {
    const id = pendingFocus.current;
    if (id === null) {
      return;
    }
    pendingFocus.current = null;
    rowRefs.current.get(id)?.focus();
  }, [groups]);

  function registerRow(id: string, element: HTMLLIElement | null) {
    if (element === null) {
      rowRefs.current.delete(id);
      return;
    }
    rowRefs.current.set(id, element);
  }

  async function move(capability: ProviderCapability, id: string, direction: -1 | 1) {
    if (groups === null) {
      return;
    }
    const group = groups.find((candidate) => candidate.capability === capability);
    if (!group) {
      return;
    }
    const from = group.providers.findIndex((provider) => provider.id === id);
    const to = from + direction;
    const moved = group.providers[from];
    const displaced = group.providers[to];
    if (from < 0 || moved === undefined || displaced === undefined) {
      return;
    }

    // The backend takes the complete order, not a delta: a partial list would leave the
    // providers it omits in an order nobody chose.
    const ordered = group.providers.map((provider) => provider.id);
    ordered[from] = displaced.id;
    ordered[to] = moved.id;

    const previous = groups;
    const optimistic = groups.map((candidate) =>
      candidate.capability === capability
        ? { ...candidate, providers: swap(candidate.providers, from, to) }
        : candidate,
    );

    setNoticeById((current) => withoutKey(current, id));
    setGroups(optimistic);
    setBusyId(id);
    pendingFocus.current = id;

    try {
      const fresh = await backend.setProviderPriority(capability, ordered);
      setGroups(fresh);
      setAnnouncement(
        `${moved.displayName} moved to position ${to + 1} of ${ordered.length} in the ` +
          `${capabilityLabel(capability).toLowerCase()} list.`,
      );
    } catch (error) {
      setGroups(previous);
      pendingFocus.current = id;
      setNoticeById((current) => ({
        ...current,
        [id]: `The new order was not saved. ${toExtensionFailure(error).message}`,
      }));
    } finally {
      setBusyId(null);
    }
  }

  async function retry(id: string) {
    if (groups === null) {
      return;
    }
    const provider = groups
      .flatMap((group) => group.providers)
      .find((candidate) => candidate.id === id);
    if (!provider) {
      return;
    }

    setNoticeById((current) => withoutKey(current, id));
    setBusyId(id);
    try {
      const health = await backend.retryProvider(id);
      setGroups((current) =>
        current === null
          ? current
          : withProvider(current, id, (candidate) => ({ ...candidate, health })),
      );
      setAnnouncement(`${provider.displayName} reports ${healthLabel(health).toLowerCase()}.`);
    } catch (error) {
      setNoticeById((current) => ({
        ...current,
        [id]: `The provider could not be reached. ${toExtensionFailure(error).message}`,
      }));
    } finally {
      setBusyId(null);
    }
  }

  if (groups === null) {
    return (
      <div className="ext-body">
        {readFailure === null ? (
          <p className="prov-loading" role="status">
            Reading provider state...
          </p>
        ) : (
          <div className="ext-banner" data-tone="error" role="alert">
            <p className="ext-banner-text">
              <strong className="ext-banner-label">Provider state could not be read.</strong>{" "}
              {readFailure}
            </p>
            <button type="button" className="button" onClick={() => void load()}>
              Try again
            </button>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="ext-body">
      <section className="prov-panel" aria-labelledby="prov-title" aria-busy={busyId !== null}>
        <div className="prov-intro">
          <h2 className="prov-title" id="prov-title">
            Provider order
          </h2>
          <p className="prov-copy">
            Each capability is asked in this order, top first. A provider that fails three
            calls in a row is skipped until it answers again. Health is measured for this
            session only; it is not a saved setting.
          </p>
        </div>

        <div className="prov-default">
          <label htmlFor="default-search-provider" className="prov-default-label">
            Default search provider
          </label>
          <select
            id="default-search-provider"
            className="prov-default-select"
            value={defaultProvider ?? ""}
            onChange={(event) => {
              const value = event.target.value;
              if (value === "") {
                localStorage.removeItem("qmusic.defaultSearchProvider");
                setDefaultProvider(null);
              } else {
                localStorage.setItem("qmusic.defaultSearchProvider", value);
                setDefaultProvider(value);
              }
            }}
          >
            <option value="">All providers</option>
            {groups
              ?.find((g) => g.capability === "metadata")
              ?.providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.displayName}
                </option>
              ))}
          </select>
          <p className="prov-default-hint">
            When set, search queries only this provider instead of all of them.
          </p>
        </div>

        {readFailure === null ? null : (
          <div className="ext-banner" data-tone="error" role="alert">
            <p className="ext-banner-text">
              <strong className="ext-banner-label">The order could not be re-read.</strong>{" "}
              {readFailure}
            </p>
            <button type="button" className="button" onClick={() => void load()}>
              Try again
            </button>
          </div>
        )}

        <div className="prov-groups">
          {groups.map((group) => {
            const label = capabilityLabel(group.capability);
            const titleId = `prov-group-${group.capability}`;
            return (
              <section className="prov-group" aria-labelledby={titleId} key={group.capability}>
                <div className="prov-group-head">
                  <h3 className="prov-group-title" id={titleId}>
                    {label}
                  </h3>
                  <span className="prov-group-count">
                    {group.providers.length === 1
                      ? "1 provider"
                      : `${group.providers.length} providers`}
                  </span>
                </div>

                {group.providers.length === 0 ? (
                  <p className="prov-empty">
                    No {label.toLowerCase()} provider is installed. Install one from the
                    Catalogue tab to add it to this list.
                  </p>
                ) : (
                  <ol className="prov-list" role="list">
                    {group.providers.map((provider, index) => (
                      <ProviderRow
                        key={provider.id}
                        provider={provider}
                        position={index + 1}
                        total={group.providers.length}
                        busy={busyId === provider.id}
                        notice={noticeById[provider.id]}
                        onMove={(direction) => void move(group.capability, provider.id, direction)}
                        onRetry={() => void retry(provider.id)}
                        register={(element) => registerRow(provider.id, element)}
                      />
                    ))}
                  </ol>
                )}
              </section>
            );
          })}
        </div>

        <p className="sr-only" role="status">
          {announcement}
        </p>
      </section>
    </div>
  );
}
