import { useCallback, useEffect, useRef, useState } from "react";

import { backend } from "../lib/ipc";
import { useStore } from "../state/store";
import "../styles/home-feed.css";

const CACHE_TTL_MS = 5 * 60 * 1000;

interface CacheEntry {
  ts: number;
  data: any;
}

function cacheKey(id: string): string {
  return `qmusic.homefeed.${id}`;
}

function readCache(id: string): CacheEntry | null {
  try {
    const raw = localStorage.getItem(cacheKey(id));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CacheEntry;
    if (typeof parsed?.ts !== "number" || parsed.data === undefined) return null;
    return parsed;
  } catch {
    return null;
  }
}

function writeCache(id: string, data: any): void {
  try {
    localStorage.setItem(cacheKey(id), JSON.stringify({ ts: Date.now(), data }));
  } catch {
    // Private mode or quota: in-memory only.
  }
}

export function HomeFeedView() {
  const { view, setView } = useStore();
  if (view.kind !== "provider-home-feed") return null;
  const providerId = view.id;

  const [data, setData] = useState<any>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [pullDistance, setPullDistance] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const touchStartY = useRef<number | null>(null);

  const load = useCallback(
    async (force: boolean) => {
      if (force) setRefreshing(true);
      else setLoading(true);
      setError(null);
      try {
        if (!force) {
          const cached = readCache(providerId);
          if (cached && Date.now() - cached.ts < CACHE_TTL_MS) {
            setData(cached.data);
            setLoading(false);
            return;
          }
        }
        const result = await backend.providerHomeFeed(providerId);
        setData(result);
        writeCache(providerId, result);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setLoading(false);
        setRefreshing(false);
      }
    },
    [providerId],
  );

  useEffect(() => {
    void load(false);
  }, [load]);

  function handleTouchStart(event: React.TouchEvent) {
    const container = containerRef.current;
    if (container && container.scrollTop <= 0) {
      touchStartY.current = event.touches[0]?.clientY ?? null;
    } else {
      touchStartY.current = null;
    }
  }

  function handleTouchMove(event: React.TouchEvent) {
    if (touchStartY.current === null) return;
    const currentY = event.touches[0]?.clientY ?? touchStartY.current;
    const distance = Math.max(0, currentY - touchStartY.current);
    setPullDistance(Math.min(distance, 120));
  }

  function handleTouchEnd() {
    if (pullDistance > 60) {
      void load(true);
    }
    setPullDistance(0);
    touchStartY.current = null;
  }

  function openItem(item: any) {
    const entityType = item?.type ?? item?.itemType ?? "unknown";
    setView({
      kind: "resolved",
      provider: data?.provider ?? providerId,
      entityType,
      item,
    });
  }

  const sections = data?.item?.sections ?? data?.sections ?? [];

  return (
    <div
      className="home-feed-view"
      ref={containerRef}
      onTouchStart={handleTouchStart}
      onTouchMove={handleTouchMove}
      onTouchEnd={handleTouchEnd}
    >
      <div className="view-head">
        <h1 className="view-title">Home feed</h1>
        <div className="view-tools">
          <span className="view-meta">{providerId}</span>
          <button type="button" className="button button-quiet" disabled={refreshing} onClick={() => void load(true)}>
            {refreshing ? "Refreshing..." : "Refresh"}
          </button>
        </div>
      </div>

      {pullDistance > 0 && (
        <div className="home-feed-pull" style={{ height: pullDistance }}>
          {pullDistance > 60 ? "Release to refresh" : "Pull to refresh"}
        </div>
      )}

      <div className="view-body">
        {loading && data === null ? (
          <div aria-hidden="true">
            {Array.from({ length: 4 }, (_, i) => (
              <div key={i} className="skeleton-row" />
            ))}
          </div>
        ) : error ? (
          <div className="empty">
            <p className="empty-title">The feed could not be loaded</p>
            <p className="empty-body">{error}</p>
            <button type="button" className="button" onClick={() => void load(true)}>
              Try again
            </button>
          </div>
        ) : sections.length === 0 ? (
          <div className="empty">
            <p className="empty-title">No sections</p>
            <p className="empty-body">This provider returned an empty feed.</p>
          </div>
        ) : (
          sections.map((section: any, index: number) => (
            <section key={index} className="home-feed-section">
              <h2 className="home-feed-section-title">{section.title ?? `Section ${index + 1}`}</h2>
              <div className="home-feed-row">
                {(section.items ?? []).map((item: any, itemIndex: number) => {
                  const art =
                    Array.isArray(item.images) && item.images.length > 0 ? item.images[0] : item.artUrl ?? null;
                  const title = item.title ?? item.name ?? "Untitled";
                  const subtitle = Array.isArray(item.artists)
                    ? item.artists.join(", ")
                    : item.artist ?? item.artists ?? "";
                  return (
                    <button
                      type="button"
                      key={item.id ?? itemIndex}
                      className="home-feed-card"
                      onClick={() => openItem(item)}
                    >
                      <span className="home-feed-art">
                        {art ? <img src={art} alt="" loading="lazy" /> : <span className="home-feed-art-fallback" aria-hidden="true" />}
                      </span>
                      <span className="home-feed-title">{title}</span>
                      <span className="home-feed-subtitle">{subtitle}</span>
                    </button>
                  );
                })}
              </div>
            </section>
          ))
        )}
      </div>
    </div>
  );
}
