import { useEffect, useState } from "react";

import { backend } from "../lib/ipc";
import { useStore } from "../state/store";
import { SearchResults } from "../components/SearchResults";

const FILTERS = [
  { label: "ALL", value: "all" },
  { label: "SONGS", value: "tracks" },
  { label: "ALBUMS", value: "albums" },
  { label: "ARTISTS", value: "artists" },
  { label: "PLAYLISTS", value: "playlists" },
];

const RECENT_KEY = "qmusic.recentSearches";

function loadRecent(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
  } catch {
    return [];
  }
}

function saveRecent(list: string[]) {
  localStorage.setItem(RECENT_KEY, JSON.stringify(list.slice(0, 8)));
}

export function HomeView() {
  const { query, setQuery } = useStore();
  const [filter, setFilter] = useState("all");
  const [recentSearches, setRecentSearches] = useState<string[]>(loadRecent);
  const [home, setHome] = useState<any>(null);
  const [recent, setRecent] = useState<any[]>([]);

  useEffect(() => {
    void backend.providerHome().then(setHome).catch(() => setHome(null));
    void backend.recentTracks(12).then(setRecent).catch(() => setRecent([]));
  }, []);

  useEffect(() => {
    if (query.search && query.search.trim()) {
      const q = query.search.trim();
      setRecentSearches((prev) => {
        const next = [q, ...prev.filter((x) => x !== q)].slice(0, 8);
        saveRecent(next);
        return next;
      });
    }
  }, [query.search]);

  function submitSearch(q: string) {
    setQuery({ search: q });
  }

  return (
    <div className="home-view" style={{ padding: "24px", overflow: "auto", height: "100%" }}>
      <header style={{ marginBottom: 24 }}>
        <h1 style={{ fontSize: 28, margin: 0 }}>Home</h1>
        <p style={{ opacity: 0.7, marginTop: 4 }}>Search across your online providers.</p>
      </header>

      <div className="search" style={{ maxWidth: 720, marginBottom: 16 }}>
        <input
          id="home-search"
          type="search"
          placeholder="Search songs, albums, artists, playlists…"
          value={query.search ?? ""}
          onChange={(e) => setQuery({ search: e.target.value || null })}
          onKeyDown={(e) => {
            if (e.key === "Enter" && query.search) submitSearch(query.search);
          }}
          style={{ width: "100%", padding: "12px 16px", fontSize: 16, borderRadius: 8 }}
        />
      </div>

      <div style={{ display: "flex", gap: 8, marginBottom: 24, flexWrap: "wrap" }}>
        {FILTERS.map((f) => (
          <button
            key={f.value}
            type="button"
            onClick={() => setFilter(f.value)}
            style={{
              padding: "6px 14px",
              borderRadius: 999,
              border: "1px solid currentColor",
              background: filter === f.value ? "currentColor" : "transparent",
              color: filter === f.value ? "var(--bg, #fff)" : "inherit",
              cursor: "pointer",
            }}
          >
            {f.label}
          </button>
        ))}
      </div>

      {recentSearches.length > 0 && !query.search && (
        <section style={{ marginBottom: 24 }}>
          <h3 style={{ fontSize: 14, textTransform: "uppercase", opacity: 0.6 }}>Recently searched</h3>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginTop: 8 }}>
            {recentSearches.map((q) => (
              <button
                key={q}
                type="button"
                onClick={() => submitSearch(q)}
                style={{ padding: "4px 10px", borderRadius: 999, border: "1px solid #ccc", background: "transparent", cursor: "pointer" }}
              >
                {q}
              </button>
            ))}
          </div>
        </section>
      )}

      {home?.providers?.length > 0 && !query.search && (
        <section style={{ marginBottom: 24 }}>
          <h3 style={{ fontSize: 14, textTransform: "uppercase", opacity: 0.6 }}>For you</h3>
          {home.providers.map((p: any) => (
            <div key={p.id} style={{ marginTop: 16 }}>
              <h4 style={{ margin: "8px 0" }}>{p.name}</h4>
              {p.sections?.map((s: any, i: number) => (
                <div key={i} style={{ marginBottom: 12 }}>
                  <div style={{ fontSize: 13, opacity: 0.7, marginBottom: 6 }}>{s.title}</div>
                  <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(120px, 1fr))", gap: 12 }}>
                    {s.items?.map((it: any) => (
                      <div key={it.id} style={{ fontSize: 12 }}>
                        {it.artUrl ? <img src={it.artUrl} alt="" style={{ width: "100%", aspectRatio: "1", objectFit: "cover", borderRadius: 6 }} /> : <div style={{ width: "100%", aspectRatio: "1", background: "#eee", borderRadius: 6 }} />}
                        <div style={{ marginTop: 4, fontWeight: 600 }}>{it.title}</div>
                        <div style={{ opacity: 0.6 }}>{it.artists?.join(", ")}</div>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          ))}
        </section>
      )}

      {recent.length > 0 && !query.search && (
        <section style={{ marginBottom: 24 }}>
          <h3 style={{ fontSize: 14, textTransform: "uppercase", opacity: 0.6 }}>Recently played</h3>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(120px, 1fr))", gap: 12, marginTop: 8 }}>
            {recent.slice(0, 12).map((t: any) => (
              <div key={t.id} style={{ fontSize: 12 }}>
                {t.artId ? null : null}
                <div style={{ fontWeight: 600 }}>{t.title}</div>
                <div style={{ opacity: 0.6 }}>{t.artist}</div>
              </div>
            ))}
          </div>
        </section>
      )}

      {query.search && <SearchResults filter={filter} />}
    </div>
  );
}