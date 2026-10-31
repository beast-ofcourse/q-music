# Q-Music — design record

What the window actually is, as built. Values here are copied from the code
(`src/styles/tokens.css`, `src/styles/app.css`), not aspirations. The direction brief
lives in `SURFACE.md`; the product decisions live in `PRODUCT.md`.

## The claim

A collection you already own, measured. Matte ground, hairline rows, one vermilion signal
spent on the current row, the level plot and the active nav mark. No cards, no grid of
cover tiles, no centred hero, no gradient, no glass. Every number is set in IBM Plex Mono,
tabular, right-aligned, so counts and durations line up down the column and read as
instrument output.

One deliberate exception: the Extensions view is a grid of cards. A catalogue is a set of
separate objects with their own version, publisher and install state, and a row cannot hold
four independent controls without lying about what a row is. Those cards are the only raised
surfaces in the window, they use the same hairline and radius as the rest, and they exist
nowhere else.

## Type

| Token | Size | Use |
| --- | --- | --- |
| `--text-xs` | 11px | rail labels, tech strip, time readouts, tags |
| `--text-sm` | 12px | secondary numerals |
| `--text-md` | 13px | body, rows, controls, field labels |
| `--text-base` | 14px | now-panel empty state |
| `--text-lg` | 17px | now-panel title, brand mark |
| `--text-xl` | 22px | view titles |
| `--text-display` | 34px | reserved, not used by the shipped views |

Archivo (variable, self-hosted `src/assets/fonts/archivo-latin-var.woff2`) carries
language; IBM Plex Mono 400/500 (`ibm-plex-mono-latin-400/500`) carries every measurement.
Both are SIL OFL and are bundled, so the window never depends on a system font's mood.
No platform sans appears in a display position.

## Space, geometry, radius, motion

- Space scale: 4, 8, 12, 16, 24, 32, 48 px.
- Header 56px, rail 208px (190px below 1180px), now column 340px, transport bar 88px.
- Track row 34px, table head 30px, grouped row (albums/artists) 48px.
- Radius: 2px and 4px only. No hard offset shadows; the only shadow is the overlay drawer
  and the row menu, both floating above the page.
- Motion: `--travel: 260ms`, `--ease-out: cubic-bezier(0.16, 1, 0.3, 1)`. One authored
  motion — the vermilion marker travelling between rows. `prefers-reduced-motion: reduce`
  collapses every animation and transition to 0.001ms.

## Colour

Three themes, each a swap of one token block. `--signal` is a **fill only** (the row
marker, the level plot, the primary button); it never draws a glyph. Text that must read
as signal uses `--signal-text`, which is tuned per theme to clear 4.5:1.

| | carbon (default) | paper | stage |
| --- | --- | --- | --- |
| `--bg` | `#0A0B0C` | `#F2EFE8` | `#080B12` |
| `--bg-raised` | `#141618` | `#E9E5DC` | `#10151F` |
| `--bg-sunken` | `#060708` | `#E2DED4` | `#05070C` |
| `--ink` | `#EDEAE3` | `#141310` | `#E4E7EE` |
| `--ink-2` | `#B9B4AC` | `#3D3A33` | `#B0B7C4` |
| `--ink-3` | `#8A867F` | `#66615A` | `#7E8797` |
| `--signal` | `#E23A1F` | `#C8320F` | `#E23A1F` |
| `--signal-text` | `#FF6E4A` | `#A8280B` | `#FF7B5E` |
| hairlines | bone 10% / 20% | ink 14% / 28% | ink 11% / 22% |

Focus is `outline: 2px solid var(--signal-text); outline-offset: 2px` everywhere, inset on
table rows and menu items so the ring is never clipped. Scrollbars, caret and selection are
themed, not left to the platform.

## Layout

`display: grid` with named areas:

```
"chrome chrome chrome"
"rail   main   now"
"transport transport transport"
```

Three states, and no others:

1. **≥1181px** — three columns, transport full width at the bottom.
2. **≤1180px** — the now column becomes a fixed overlay drawer (`top: header`, `bottom:
   transport`, `width: min(360px, 90vw)`) over the table; the rail narrows to 190px; the
   volume slider is replaced by the transport tools. The drawer is `inert` and
   `aria-hidden` while closed.
3. **Drawer closed at ≥1181px** — the now column is dropped from the grid so the table
   keeps the width.

The window minimum is 960 × 620 (`tauri.conf.json`), so those are the only two widths that
exist. At 620px tall the panel gives height up in a fixed order: cover art shrinks first,
then the lower block (tab bar plus queue or lyrics) keeps a 112px floor and its pane
scrolls. The panel clips its own overflow, so nothing can paint over the transport bar.

## Components

| File | What it owns |
| --- | --- |
| `components/Chrome.tsx` | brand, debounced search (220ms) into `TrackQuery`, honest count readout per view, panel toggle |
| `components/Rail.tsx` | nav, playlist shortcuts, folder shortcuts, foot statistics, live scan strip, synthetic-data flag |
| `components/TrackTable.tsx` | ARIA table, sortable head, travelling marker, row play target, favourite toggle, context menu |
| `components/RowMenu.tsx` | add to playlist, edit tags, show file — roving focus, Escape, focus returned to the row |
| `components/LevelPlot.tsx` | the engine's measured RMS/peak, drawn as bars; silence draws flat |
| `components/NowPanel.tsx` | cover, title/artist/tech, plot, Queue/Lyrics tabs, drawer close |
| `components/Scrubber.tsx` | pointer-captured seek; `role="slider"` with real `aria-valuenow` |
| `components/Transport.tsx` | identity, transport, volume, speed, EQ entry |
| `components/Toast.tsx` | `role="status"` message with dismiss |
| `views/*` | one view per destination; `App.tsx` switches exhaustively so none is unreachable |
| `components/extensions/*` | catalogue cards, in-card install progress, installed-state controls; no modal, no per-package branch |
| `components/extensions/ExtensionSettingsSheet.tsx` | descriptor-driven settings sheet that replaces the catalogue: pinned head and save bar, only the fields scroll, quality overrides shown disabled under a `From ...` badge, inline leave confirm |

Row states are explicit data attributes rather than class soup: `data-playing`,
`data-selected`, `data-missing`. Selected paints `--bg-raised`; playing mixes 7% of the
signal into the row and lights the play target; a row whose sample rate is unknown reads
`--ink-3` because the file is not actually playable. Loading uses skeleton rows, not a
spinner.

Settings are drawn from descriptors in declaration order, never from a package id: text,
number, boolean, select, button. A type the host does not know draws a disabled control and
names the type it could not draw, so an unsupported manifest degrades in place. A quality's
overrides are displayed, not written into the draft: saving sends the user layer plus the
chosen quality id, so picking a quality cannot launder its values into what you saved. A
field the quality sets is disabled and badged, so every value on screen has one visible
owner. The sheet replaces the catalogue instead of floating over it because the now panel is
already a drawer below 1181px, and two drawers over one body leave no honest place to read
the values.

## Keyboard

Space play/pause · ← → seek 5s · Shift+← → seek 30s · N/P next/previous · M mute · F
favourite · S shuffle · R repeat cycle · / focus search · Esc close the now panel. The
global handler ignores typing targets and any modified chord, so a letter typed into the
search field or the tag editor is never eaten.

## Data path

The frontend talks to one `Backend` interface (`src/lib/ipc.ts`). `tauriBackend` calls the
48 Tauri commands; `demoBackend` (`src/lib/demo.ts`) serves a synthetic 65-track library so
the interface can be driven in a plain browser. Demo mode is labelled in the rail and its
cover art is generated, not real sleeves. Only `ui.theme` and `ui.shuffle` are written by
the frontend; the `player.*` keys are written by the Rust side and only read back.

## Verification

Run against the built bundle in headless Chrome via `scripts/shoot.mjs` (CDP, no
dependencies) at 1440×900 and at the 960×620 minimum:

- `npx tsc --noEmit` — 0 errors; `npx vite build` — succeeds (356kB JS / 40.7kB CSS).
- 18 screenshots in `.impeccable/review/` covering library, row menu, playlist add, search,
  lyrics, playlists, folders, albums, artists, equaliser, settings (top and bottom), editor,
  the minimum window with the drawer closed and open, and both alternate themes.
- Computed-contrast audit: every text node in 7 destinations × 3 themes, compared against
  its real resolved background — 0 nodes below 4.5:1 (3:1 for large text).
- DOM measurement, not eyeballing, for the panel geometry: at 960×620 the panel is now
  476px with `scrollHeight === clientHeight`, pane 78px; at 1440×900 the cover is 339px and
  the pane 189px.
- Extension settings sheet, driven through CDP: a save round-trips and survives reload
  (`apiBase` stored as `https://roundtrip2.example/check`); keyboard order is Back → quality
  → API base → market → timeout → enrich genres → check service → refresh token → Save →
  Close; Escape closes the sheet only and returns focus to the card's Settings trigger; 0
  console messages or exceptions across open → quality → action → save → close. Contrast
  audit of the sheet, 47 nodes per theme: minimum 4.56 carbon / 5.34 paper / 4.55 stage.
- At 960×620 the sheet occupies y 56–532 with `scrollWidth - clientWidth === 0`, and the
  transport below it stays fully visible.

## Known limits

- The demo library is synthetic. So is its cover art: a generated circle, sliced to thumb
  size in list rows. Real files show real embedded artwork.
- Playback speed is tape-style resampling, so pitch moves with speed. `signalsmith-stretch`
  was rejected because the toolchain has no libclang.
- The tag editor cannot clear a numeric field; blank means "leave the current value", and
  the view says so.
- `Libre` lyric estimation spreads lines across the track length. It is a first pass, and
  the view calls it that.
- The settings sheet's loading, load-failure, unknown-type and no-settings branches exist and
  are announced, but the demo backend only serves the seeded descriptor list, so they are
  unverified in a browser.
- `button-primary` carries a white-on-signal label everywhere: 4.32:1 in carbon and stage.
  The settings sheet overrides the label to near-black locally (4.56:1); the shared button
  still carries the lower ratio in the editor and the smart-rule builder.
- The playing-row tint uses `color-mix()`, so it needs Chromium 111+ / WebKit 16.2+; on
  anything older the row simply stays untinted (graceful, not broken).
