import { useCallback, useEffect, useState } from "react";

import { backend } from "../lib/ipc";
import { useStore } from "../state/store";
import "../styles/home-feed.css";

export function BrowseCategoriesView() {
  const { view, notify } = useStore();
  if (view.kind !== "provider-browse-categories") return null;
  const providerId = view.id;

  const [data, setData] = useState<any>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(
    async (force: boolean) => {
      if (force) setRefreshing(true);
      else setLoading(true);
      setError(null);
      try {
        const result = await backend.providerBrowseCategories(providerId);
        setData(result);
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

  const categories = data?.item?.categories ?? data?.categories ?? [];

  return (
    <div className="browse-categories-view">
      <div className="view-head">
        <h1 className="view-title">Browse categories</h1>
        <div className="view-tools">
          <span className="view-meta">{providerId}</span>
          <button type="button" className="button button-quiet" disabled={refreshing} onClick={() => void load(true)}>
            {refreshing ? "Refreshing..." : "Refresh"}
          </button>
        </div>
      </div>
      <div className="view-body">
        {loading && data === null ? (
          <div aria-hidden="true">
            {Array.from({ length: 6 }, (_, i) => (
              <div key={i} className="skeleton-row" />
            ))}
          </div>
        ) : error ? (
          <div className="empty">
            <p className="empty-title">Categories could not be loaded</p>
            <p className="empty-body">{error}</p>
            <button type="button" className="button" onClick={() => void load(true)}>
              Try again
            </button>
          </div>
        ) : categories.length === 0 ? (
          <div className="empty">
            <p className="empty-title">No categories</p>
            <p className="empty-body">This provider returned no browse categories.</p>
          </div>
        ) : (
          <ul className="browse-category-list" role="list">
            {categories.map((category: any, index: number) => (
              <li key={category.id ?? index} className="browse-category-row">
                <button type="button" className="browse-category-button" onClick={() => notify?.(`${category.name ?? category.id}`)}>
                  <span className="browse-category-name">{category.name ?? category.id}</span>
                  {category.id ? <span className="browse-category-id">{category.id}</span> : null}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
