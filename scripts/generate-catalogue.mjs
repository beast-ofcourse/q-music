// Regenerates `src/lib/extension-catalogue.ts` from the qext registry fixture.
//
// The browser demo has no network, so the published catalogue is compiled in. Copying
// it mechanically is the only way to guarantee the demo never disagrees with the
// document the host would fetch.
import fs from "node:fs";
import path from "node:path";

const fixture = path.join("crates", "qext", "fixtures", "registry.json");
const target = path.join("src", "lib", "extension-catalogue.ts");

const raw = fs.readFileSync(fixture, "utf8").replace(/^\uFEFF/, "");
const doc = JSON.parse(raw);

const entries = doc.extensions.map((entry) => ({
  id: entry.id,
  name: entry.name,
  displayName: entry.display_name ?? null,
  version: entry.version,
  description: entry.description ?? null,
  downloadUrl: entry.download_url,
  sha256: entry.sha256,
  category: entry.category ?? null,
  tags: entry.tags ?? [],
  downloads: entry.downloads ?? 0,
  updatedAt: entry.updated_at ?? null,
  minAppVersion: entry.min_app_version ?? null,
  iconUrl: entry.icon_url ?? null,
}));

const header = `// The published extension catalogue, compiled in for the browser demo.
//
// Generated from \`crates/qext/fixtures/registry.json\` by
// \`node scripts/generate-catalogue.mjs\`. Ids, versions and digests are copied from
// the fixture rather than typed by hand, so a demo card can never disagree with the
// document the host would actually fetch. Regenerate after the fixture changes.

import type { PublishedEntry } from "./extensions";

export const PUBLISHED_CATALOGUE: PublishedEntry[] = `;

fs.writeFileSync(target, `${header}${JSON.stringify(entries, null, 2)};\n`, "utf8");

console.log(`wrote ${target}: ${entries.length} entries`);
