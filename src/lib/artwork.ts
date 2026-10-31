import { useEffect, useState } from "react";

import { backend } from "./ipc";

/** Artwork is fetched once per id and kept for the session: a 60-row table would
 *  otherwise ask the core for the same album image 60 times. */
const cache = new Map<string, Promise<string | null>>();

function load(artId: string): Promise<string | null> {
  const existing = cache.get(artId);
  if (existing) return existing;
  const request = backend.artwork(artId).catch(() => null);
  cache.set(artId, request);
  return request;
}

/** Resolves an artwork id to a data URL, or null when the file has none. */
export function useArtwork(artId: string | null): string | null {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!artId) {
      setUrl(null);
      return;
    }
    let alive = true;
    setUrl(null);
    void load(artId).then((resolved) => {
      if (alive) setUrl(resolved);
    });
    return () => {
      alive = false;
    };
  }, [artId]);

  return url;
}
