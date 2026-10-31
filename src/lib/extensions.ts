/**
 * The extension platform's window-facing contract.
 *
 * Every shape here mirrors a struct that `src-tauri/src/ext_commands.rs` builds from
 * `crates/qext`. The commands return *views* rather than the crate's own types: the
 * registry document and the store use snake_case internally, and letting that leak
 * into the interface would make this file disagree with `types.ts`.
 *
 * The demo implementation at the bottom is not a mock of the UI. It is a second
 * backend: the same contract, backed by the real published catalogue and a synthetic
 * host, so the whole surface can be walked in a plain browser with no Rust process.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { PUBLISHED_CATALOGUE } from "./extension-catalogue";
import type { LyricLine } from "./types";

/** The app version this window reports. `registry.json` entries declare a minimum. */
export const HOST_VERSION = "4.10.0";

/** One entry exactly as the registry document publishes it. */
export interface PublishedEntry {
  id: string;
  name: string;
  displayName: string | null;
  version: string;
  description: string | null;
  downloadUrl: string;
  sha256: string;
  category: string | null;
  tags: string[];
  downloads: number;
  updatedAt: string | null;
  minAppVersion: string | null;
  iconUrl: string | null;
}

/** Why the host cannot install an entry. Mirrors `qext::registry::Unsupported`. */
export type EntryStatus =
  | { state: "not_evaluated" }
  | { state: "supported" }
  | { state: "unsupported"; reason: "host_too_old"; required: string; host: string }
  | { state: "unsupported"; reason: "invalid_version"; field: string; value: string }
  | { state: "unsupported"; reason: "invalid_checksum"; value: string };

export interface CatalogueEntry extends PublishedEntry {
  status: EntryStatus;
  /**
   * `requiredRuntimeFeatures` this host does not implement. Empty until the package has
   * been inspected: the published document does not declare manifest features, so the
   * host learns them from the archive (or from a refused install).
   */
  missingFeatures: string[];
}

export interface InstalledExtension {
  id: string;
  version: string;
  displayName: string | null;
  sha256: string;
  installedAt: number;
  enabled: boolean;
  directory: string;
  dataDirectory: string;
}

/** A directory under the extensions root that cannot be served. */
export interface BrokenExtension {
  id: string;
  path: string;
  reason: string;
}

/** Everything the Extensions view draws, in one round trip. */
export interface CatalogueSnapshot {
  entries: CatalogueEntry[];
  installed: InstalledExtension[];
  broken: BrokenExtension[];
  /** Present when the last refresh failed; the entries above are then the cache. */
  refreshError: string | null;
  fetchedAt: number | null;
}

export type InstallPhase =
  | "queued"
  | "downloading"
  | "verifying"
  | "installing"
  | "done"
  | "failed";

export interface InstallProgress {
  id: string;
  phase: InstallPhase;
  percent: number;
  message: string | null;
}

export type ExtensionErrorCode =
  | "unsupported_features"
  | "host_too_old"
  | "invalid_version"
  | "invalid_checksum"
  | "checksum_mismatch"
  | "id_mismatch"
  | "already_installed"
  | "downgrade"
  | "not_installed"
  | "disabled"
  | "package_invalid"
  | "refused"
  | "network"
  | "unknown";

/** The rejection value every extension command uses. Never a bare string. */
export interface ExtensionFailure {
  code: ExtensionErrorCode;
  message: string;
  /** `requiredRuntimeFeatures` this host does not implement, when that is the reason. */
  missingFeatures: string[];
}

export function isExtensionFailure(value: unknown): value is ExtensionFailure {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ExtensionFailure).code === "string" &&
    typeof (value as ExtensionFailure).message === "string"
  );
}

/** Normalises anything a command can throw into the typed failure. */
export function toExtensionFailure(error: unknown): ExtensionFailure {
  if (isExtensionFailure(error)) {
    return {
      code: error.code,
      message: error.message,
      missingFeatures: error.missingFeatures ?? [],
    };
  }
  return {
    code: "unknown",
    message: typeof error === "string" ? error : String(error),
    missingFeatures: [],
  };
}

/** The setting types `settings[]` can declare. Anything else is a host gap, not a guess. */
export type SettingType = "text" | "string" | "number" | "boolean" | "select" | "button";

export interface SettingChoice {
  value: string;
  label: string;
}

export interface SettingDescriptor {
  key: string;
  label: string;
  type: SettingType;
  /** JSON default from the manifest. Absent for `button`. */
  defaultValue: unknown;
  description: string | null;
  options: SettingChoice[];
  /** Registered action a `button` setting invokes. */
  action: string | null;
}

export interface QualityDescriptor {
  id: string;
  label: string;
  description: string | null;
  kind: string | null;
  /** Setting overrides this quality applies on top of the saved values. */
  settings: Record<string, unknown>;
}

export interface ExtensionSettingsView {
  id: string;
  displayName: string;
  version: string;
  settings: SettingDescriptor[];
  qualities: QualityDescriptor[];
  selectedQuality: string;
  /** Effective values: manifest defaults, then saved values, then quality overrides. */
  values: Record<string, unknown>;
  /** Values the user saved, before any quality override. */
  savedValues: Record<string, unknown>;
}

export interface ActionOutcome {
  ok: boolean;
  message: string | null;
}

export type ProviderCapability = "metadata" | "download" | "lyrics";

export type HealthState = "healthy" | "unhealthy" | "unknown";

export interface ProviderHealth {
  state: HealthState;
  message: string | null;
  consecutiveFailures: number;
}

export interface ProviderView {
  id: string;
  displayName: string;
  capability: ProviderCapability;
  version: string;
  enabled: boolean;
  quality: string;
  qualities: QualityDescriptor[];
  health: ProviderHealth;
  /** Features the package declares that this host does not implement. */
  missingFeatures: string[];
}

export interface ProviderGroup {
  capability: ProviderCapability;
  /** Ordered: index 0 is tried first. */
  providers: ProviderView[];
}

export type JobStage =
  | "queued"
  | "resolving"
  | "downloading"
  | "post_processing"
  | "embedding"
  | "done"
  | "failed"
  | "cancelled";

export interface JobFailure {
  stage: JobStage;
  /** Stable code from `qext::adapter::DownloadFailureKind`. */
  kind: string;
  message: string;
}

export interface QueueJob {
  id: string;
  trackId: string;
  title: string;
  artist: string;
  provider: string;
  quality: string;
  stage: JobStage;
  percent: number | null;
  bytesWritten: number;
  bytesTotal: number | null;
  /** Measured by the host from successive progress samples; `null` before any. */
  speedBps: number | null;
  outputPath: string;
  failure: JobFailure | null;
  updatedAt: number;
}

/** One track a provider offered. The local library has its own `Track` shape. */
export interface ProviderTrack {
  provider: string;
  providerName: string;
  providerTrackId: string;
  title: string;
  artist: string;
  album: string;
  durationMs: number;
  artUrl: string | null;
  quality: string | null;
}

export interface OnlineSearchResponse {
  requestId: string;
  query: string;
  tracks: OnlineTrackResult[];
  albums: OnlineAlbumResult[];
  artists: OnlineArtistResult[];
  playlists: OnlinePlaylistResult[];
  providers: OnlineSearchProviderStatus[];
  elapsedMs: number;
  cancelled: boolean;
}

export interface OnlineTrackResult {
  id: string;
  provider: string;
  providerName: string;
  providerTrackId: string;
  title: string;
  artists: string[];
  album?: string | null;
  albumId?: string | null;
  albumProvider?: string | null;
  durationMs: number;
  artUrl?: string | null;
  isrc?: string | null;
  explicit: boolean;
  inLibrary?: boolean;
  itemType: string;
}

export interface OnlineAlbumResult {
  id: string;
  provider: string;
  providerName: string;
  providerAlbumId: string;
  title: string;
  artists: string[];
  year?: number | null;
  trackCount?: number | null;
  artUrl?: string | null;
  alsoOn?: string[];
  itemType: string;
}

export interface OnlineArtistResult {
  id: string;
  provider: string;
  providerName: string;
  providerArtistId: string;
  name: string;
  artUrl?: string | null;
  alsoOn?: string[];
  itemType: string;
}

export interface OnlinePlaylistResult {
  id: string;
  provider: string;
  providerName: string;
  providerPlaylistId: string;
  title: string;
  creator?: string | null;
  trackCount?: number | null;
  artUrl?: string | null;
  alsoOn?: string[];
  itemType: string;
}

export interface OnlineSearchProviderStatus {
  id: string;
  name: string;
  state: string;
  error?: string | null;
  elapsedMs: number;
  hitCounts: { tracks: number; albums: number; artists: number; playlists: number };
}
export interface ProviderSearchOutcome {
  provider: string;
  /** A quiet, provider-scoped message. Never delays the other providers. */
  failure: string | null;
}

export interface ProviderLyrics {
  provider: string;
  providerName: string;
  trackId: number;
  synced: boolean;
  lines: LyricLine[];
}

/** The extension half of `Backend`. */
export interface ExtensionSurface {
  extensionsSnapshot(): Promise<CatalogueSnapshot>;
  installExtension(id: string): Promise<InstalledExtension>;
  installExtensionFromFile(): Promise<InstalledExtension | null>;
  uninstallExtension(id: string): Promise<void>;
  setExtensionEnabled(id: string, enabled: boolean): Promise<InstalledExtension>;

  extensionSettings(id: string): Promise<ExtensionSettingsView>;
  saveExtensionSettings(
    id: string,
    values: Record<string, unknown>,
    quality: string,
  ): Promise<ExtensionSettingsView>;
  invokeExtensionAction(id: string, action: string): Promise<ActionOutcome>;

  providers(): Promise<ProviderGroup[]>;
  setProviderPriority(capability: ProviderCapability, ids: string[]): Promise<ProviderGroup[]>;
  retryProvider(id: string): Promise<ProviderHealth>;

  queueJobs(): Promise<QueueJob[]>;
  cancelJob(jobId: string): Promise<void>;
  retryJob(jobId: string): Promise<void>;
  enqueueDownload(track: ProviderTrack): Promise<QueueJob>;

  searchProviders(query: string, limit: number): Promise<ProviderSearchOutcome[]>;
  onlineSearch(
    requestId: string,
    query: string,
    types: string[],
    limit: number,
    offset: number,
  ): Promise<OnlineSearchResponse>;
  onlineSearchCancel(requestId: string): Promise<void>;
  providerDetail(kind: string, provider: string, id: string): Promise<any>;
  providerHome(): Promise<any>;
  providerLyrics(trackId: number): Promise<ProviderLyrics | null>;
  handleUrl(url: string): Promise<any>;
  providerHomeFeed(id: string): Promise<any>;
  providerBrowseCategories(id: string): Promise<any>;

  onExtensionProgress(handler: (progress: InstallProgress) => void): Promise<UnlistenFn>;
  onQueue(handler: (jobs: QueueJob[]) => void): Promise<UnlistenFn>;
}

/** Names shared with `src-tauri/src/ext_commands.rs`, one place to keep them aligned. */
export const COMMANDS = {
  snapshot: "extensions_snapshot",
  install: "extensions_install",
  installFile: "extensions_install_file",
  uninstall: "extensions_uninstall",
  setEnabled: "extensions_set_enabled",
  settings: "extensions_settings",
  saveSettings: "extensions_save_settings",
  invokeAction: "extensions_invoke_action",
  providers: "providers_list",
  setPriority: "providers_set_priority",
  retry: "providers_retry",
  queueJobs: "queue_jobs",
  queueCancel: "queue_cancel",
  queueRetry: "queue_retry",
  queueEnqueue: "queue_enqueue",
  search: "search_providers",
  onlineSearch: "online_search",
  onlineSearchCancel: "online_search_cancel",
  providerDetail: "provider_detail",
  providerHome: "provider_home",
  lyrics: "provider_lyrics",
  handleUrl: "extensions_handle_url",
  providerHomeFeed: "provider_home_feed",
  providerBrowseCategories: "provider_browse_categories",
} as const;

export const EVENTS = {
  install: "extensions://progress",
  queue: "queue://jobs",
} as const;

export const tauriExtensions: ExtensionSurface = {
  extensionsSnapshot: () => invoke(COMMANDS.snapshot),
  installExtension: (id) => invoke(COMMANDS.install, { id }),
  installExtensionFromFile: () => invoke(COMMANDS.installFile),
  uninstallExtension: (id) => invoke(COMMANDS.uninstall, { id }),
  setExtensionEnabled: (id, enabled) => invoke(COMMANDS.setEnabled, { id, enabled }),

  extensionSettings: (id) => invoke(COMMANDS.settings, { id }),
  saveExtensionSettings: (id, values, quality) =>
    invoke(COMMANDS.saveSettings, { id, values, quality }),
  invokeExtensionAction: (id, action) => invoke(COMMANDS.invokeAction, { id, action }),

  providers: () => invoke(COMMANDS.providers),
  setProviderPriority: (capability, ids) =>
    invoke(COMMANDS.setPriority, { capability, ids }),
  retryProvider: (id) => invoke(COMMANDS.retry, { id }),

  queueJobs: () => invoke(COMMANDS.queueJobs),
  cancelJob: (jobId) => invoke(COMMANDS.queueCancel, { jobId }),
  retryJob: (jobId) => invoke(COMMANDS.queueRetry, { jobId }),
  enqueueDownload: (track) => invoke(COMMANDS.queueEnqueue, { track }),

  searchProviders: (query, limit) => invoke(COMMANDS.search, { query, limit }),
  onlineSearch: (requestId, query, types, limit, offset) =>
    invoke(COMMANDS.onlineSearch, { requestId, query, types, limit, offset }),
  onlineSearchCancel: (requestId) => invoke(COMMANDS.onlineSearchCancel, { requestId }),
  providerDetail: (kind, provider, id) => invoke(COMMANDS.providerDetail, { kind, provider, id }),
  providerHome: () => invoke(COMMANDS.providerHome),
  providerLyrics: (trackId) => invoke(COMMANDS.lyrics, { trackId }),
  handleUrl: (url) => invoke(COMMANDS.handleUrl, { url }),
  providerHomeFeed: (id) => invoke(COMMANDS.providerHomeFeed, { id }),
  providerBrowseCategories: (id) => invoke(COMMANDS.providerBrowseCategories, { id }),

  onExtensionProgress: (handler) =>
    listen<InstallProgress>(EVENTS.install, (event) => handler(event.payload)),
  onQueue: (handler) => listen<QueueJob[]>(EVENTS.queue, (event) => handler(event.payload)),
};

/* ------------------------------------------------------------------------------------
 * Demo backend
 *
 * Below this line nothing talks to Rust. The catalogue is the real published document
 * (see `extension-catalogue.ts`), the host is simulated, and every piece of state that
 * should survive a reload is written to `localStorage`. Everything synthetic is named
 * as synthetic so a screenshot can never be mistaken for a measured result.
 * ---------------------------------------------------------------------------------- */

const DEMO_STORE_KEY = "qmusic.demo.extensions.v1";

/**
 * Packages whose manifests declare runtime features this host deliberately does not
 * implement. The real `amazon` and `tidal-web` packages require `signedSession@3`; the
 * host refuses them at install with the feature named (see `plans/project-overview.md`,
 * "Scope boundary"). The demo refuses them for the same reason rather than failing a
 * checksum, which is what would happen if it pretended the package had been downloaded.
 */
const DEMO_FEATURE_REFUSALS: Record<string, string[]> = {};

interface DemoState {
  installed: InstalledExtension[];
  /** Settings the user saved, per extension. */
  saved: Record<string, Record<string, unknown>>;
  selectedQuality: Record<string, string>;
  /** Ordered provider ids per capability; ids absent from here keep their natural order. */
  priority: Partial<Record<ProviderCapability, string[]>>;
  disabled: string[];
}

function seedState(): DemoState {
  const installed: InstalledExtension[] = ["spotify-web", "apple-music", "bugs-music"].map(
    (id, index) => {
      const entry = PUBLISHED_CATALOGUE.find((candidate) => candidate.id === id);
      return {
        id,
        version: entry?.version ?? "0.0.0",
        displayName: entry?.displayName ?? null,
        sha256: entry?.sha256 ?? "",
        installedAt: 1_760_000_000 + index,
        enabled: true,
        directory: `demo/extensions/${id}`,
        dataDirectory: `demo/extensions/.data/${id}`,
      };
    },
  );

  return { installed, saved: {}, selectedQuality: {}, priority: {}, disabled: [] };
}

function loadState(): DemoState {
  if (typeof localStorage === "undefined") {
    return seedState();
  }
  try {
    const raw = localStorage.getItem(DEMO_STORE_KEY);
    if (!raw) {
      return seedState();
    }
    const parsed = JSON.parse(raw) as Partial<DemoState>;
    return { ...seedState(), ...parsed };
  } catch {
    // A demo that cannot read its own state starts from the seed rather than dying.
    return seedState();
  }
}

let state: DemoState = loadState();

function persist(): void {
  if (typeof localStorage === "undefined") {
    return;
  }
  try {
    localStorage.setItem(DEMO_STORE_KEY, JSON.stringify(state));
  } catch {
    // Private-mode browsers refuse writes; the session still works in memory.
  }
}

function compareVersions(left: string, right: string): number {
  const parse = (text: string) => {
    const core = text.split("+")[0] ?? "";
    const [numbers = "", prerelease] = core.split("-");
    return { parts: numbers.split(".").map(Number), prerelease: prerelease ?? null };
  };
  const a = parse(left);
  const b = parse(right);
  for (let index = 0; index < 3; index += 1) {
    const difference = (a.parts[index] ?? 0) - (b.parts[index] ?? 0);
    if (difference !== 0) return difference;
  }
  if (a.prerelease === b.prerelease) return 0;
  if (a.prerelease === null) return 1;
  if (b.prerelease === null) return -1;
  return a.prerelease < b.prerelease ? -1 : 1;
}

/** The same three refusals `qext::registry::evaluate` makes, in the same order. */
function evaluate(entry: PublishedEntry): EntryStatus {
  if (!/^\d+\.\d+\.\d+/.test(entry.version)) {
    return { state: "unsupported", reason: "invalid_version", field: "version", value: entry.version };
  }
  if (entry.minAppVersion !== null && !/^\d+\.\d+\.\d+/.test(entry.minAppVersion)) {
    return {
      state: "unsupported",
      reason: "invalid_version",
      field: "min_app_version",
      value: entry.minAppVersion,
    };
  }
  if (!/^[0-9a-f]{64}$/i.test(entry.sha256)) {
    return { state: "unsupported", reason: "invalid_checksum", value: entry.sha256 };
  }
  if (entry.minAppVersion !== null && compareVersions(HOST_VERSION, entry.minAppVersion) < 0) {
    return {
      state: "unsupported",
      reason: "host_too_old",
      required: entry.minAppVersion,
      host: HOST_VERSION,
    };
  }
  return { state: "supported" };
}

function catalogue(): CatalogueEntry[] {
  return PUBLISHED_CATALOGUE.map((entry) => ({
    ...entry,
    status: evaluate(entry),
    missingFeatures: DEMO_FEATURE_REFUSALS[entry.id] ?? [],
  }));
}

function failure(
  code: ExtensionErrorCode,
  message: string,
  missingFeatures: string[] = [],
): ExtensionFailure {
  return { code, message, missingFeatures };
}

const installListeners = new Set<(progress: InstallProgress) => void>();
const queueListeners = new Set<(jobs: QueueJob[]) => void>();

function emitInstall(progress: InstallProgress): void {
  installListeners.forEach((listener) => listener(progress));
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Synthetic settings every demo extension exposes. Deliberately identical for every
 * package: the view must render whatever a manifest declares, so a demo that gave each
 * package a different shape would tempt per-extension code into the UI.
 */
function demoSettingDescriptors(): SettingDescriptor[] {
  return [
    {
      key: "apiBase",
      label: "API base",
      type: "text",
      defaultValue: "https://api.example.test",
      description: "Override the endpoint the extension calls. Leave empty to use its default.",
      options: [],
      action: null,
    },
    {
      key: "market",
      label: "Market",
      type: "select",
      defaultValue: "us",
      description: "Storefront used for catalogue requests.",
      options: [
        { value: "us", label: "United States" },
        { value: "gb", label: "United Kingdom" },
        { value: "de", label: "Germany" },
        { value: "jp", label: "Japan" },
        { value: "kr", label: "South Korea" },
      ],
      action: null,
    },
    {
      key: "timeoutSeconds",
      label: "Request timeout",
      type: "number",
      defaultValue: 15,
      description: "Seconds before a request is abandoned.",
      options: [],
      action: null,
    },
    {
      key: "enrichGenres",
      label: "Enrich genres",
      type: "boolean",
      defaultValue: true,
      description: "Add genre tags from the provider catalogue where the file has none.",
      options: [],
      action: null,
    },
    {
      key: "checkService",
      label: "Check service",
      type: "button",
      defaultValue: null,
      description: "Ask the provider whether its service answers right now.",
      options: [],
      action: "checkAvailability",
    },
    {
      key: "refreshToken",
      label: "Refresh token",
      type: "button",
      defaultValue: null,
      description: "Re-run the provider's credential refresh.",
      options: [],
      action: "refreshToken",
    },
  ];
}

function demoQualities(): QualityDescriptor[] {
  return [
    {
      id: "best",
      label: "Best available",
      description: "Whatever the provider can serve at the highest tier.",
      kind: "lossless",
      settings: {},
    },
    {
      id: "high",
      label: "High (320 kbps)",
      description: "Capped so a download stays small.",
      kind: "lossy",
      settings: { enrichGenres: false },
    },
  ];
}

function defaultsOf(descriptors: SettingDescriptor[]): Record<string, unknown> {
  const values: Record<string, unknown> = {};
  descriptors.forEach((descriptor) => {
    if (descriptor.type !== "button") {
      values[descriptor.key] = descriptor.defaultValue;
    }
  });
  return values;
}

function settingsView(id: string): ExtensionSettingsView {
  const installed = state.installed.find((candidate) => candidate.id === id);
  if (!installed) {
    throw failure("not_installed", `${id} is not installed`);
  }

  const descriptors = demoSettingDescriptors();
  const qualities = demoQualities();
  const selectedQuality = state.selectedQuality[id] ?? qualities[0]?.id ?? "best";
  const saved = state.saved[id] ?? {};
  const override = qualities.find((quality) => quality.id === selectedQuality)?.settings ?? {};

  return {
    id,
    displayName: installed.displayName ?? id,
    version: installed.version,
    settings: descriptors,
    qualities,
    selectedQuality,
    savedValues: { ...defaultsOf(descriptors), ...saved },
    values: { ...defaultsOf(descriptors), ...saved, ...override },
  };
}

const demoHealth = new Map<string, ProviderHealth>([
  [
    "bugs-music",
    {
      state: "unhealthy",
      message: "The last call did not answer within 15 s.",
      consecutiveFailures: 3,
    },
  ],
]);

/** Which capability each installed demo package serves. */
function capabilitiesOf(id: string): ProviderCapability[] {
  const catalogueEntry = PUBLISHED_CATALOGUE.find((entry) => entry.id === id);
  const tags = catalogueEntry?.tags ?? [];
  const capabilities: ProviderCapability[] = ["metadata"];
  if (tags.includes("lyrics") || id === "apple-music") {
    capabilities.push("lyrics");
  }
  if (catalogueEntry?.category === "download" && !DEMO_FEATURE_REFUSALS[id]) {
    capabilities.push("download");
  }
  return capabilities;
}

function providerGroups(): ProviderGroup[] {
  const capabilities: ProviderCapability[] = ["metadata", "download", "lyrics"];

  return capabilities.map((capability) => {
    const members = state.installed.filter(
      (installed) => installed.enabled && capabilitiesOf(installed.id).includes(capability),
    );

    const ordered = [...members].sort((left, right) => {
      const priority = state.priority[capability] ?? [];
      const leftIndex = priority.indexOf(left.id);
      const rightIndex = priority.indexOf(right.id);
      const leftRank = leftIndex === -1 ? priority.length : leftIndex;
      const rightRank = rightIndex === -1 ? priority.length : rightIndex;
      return leftRank - rightRank || left.id.localeCompare(right.id);
    });

    return {
      capability,
      providers: ordered.map((installed) => ({
        id: installed.id,
        displayName: installed.displayName ?? installed.id,
        capability,
        version: installed.version,
        enabled: installed.enabled,
        quality: state.selectedQuality[installed.id] ?? "best",
        qualities: demoQualities(),
        health: demoHealth.get(installed.id) ?? {
          state: "healthy",
          message: null,
          consecutiveFailures: 0,
        },
        missingFeatures: DEMO_FEATURE_REFUSALS[installed.id] ?? [],
      })),
    };
  });
}

/**
 * The demo queue. Two jobs so a healthy row and a failed row are visible at once, and
 * one of them walks every stage so the drawer can be watched rather than described.
 * Synthetic: the real queue only carries jobs an installed download provider accepted.
 */
const KB = 1024;
let queue: QueueJob[] = [
  {
    id: "demo-job-1",
    trackId: "demo-track-1",
    title: "Night Ferry",
    artist: "Harbour Lights",
    provider: "spotify-web",
    quality: "best",
    stage: "queued",
    percent: 0,
    bytesWritten: 0,
    bytesTotal: 9_400 * KB,
    speedBps: null,
    outputPath: "demo/downloads/Harbour Lights - Night Ferry.flac",
    failure: null,
    updatedAt: Date.now(),
  },
  {
    id: "demo-job-2",
    trackId: "demo-track-2",
    title: "Salt Flat",
    artist: "Harbour Lights",
    provider: "apple-music",
    quality: "high",
    stage: "failed",
    percent: 34,
    bytesWritten: 3_100 * KB,
    bytesTotal: 9_100 * KB,
    speedBps: null,
    outputPath: "demo/downloads/Harbour Lights - Salt Flat.m4a",
    failure: {
      stage: "downloading",
      kind: "timeout",
      message: "The provider stopped answering after 3.1 MB.",
    },
    updatedAt: Date.now(),
  },
];

const STAGE_SCRIPT: JobStage[] = [
  "queued",
  "resolving",
  "downloading",
  "post_processing",
  "embedding",
  "done",
];

let queueTicker: ReturnType<typeof setInterval> | null = null;
let lastTickAt = Date.now();

function publishQueue(): void {
  queueListeners.forEach((listener) => listener(queue.map(describeJob)));
}

function describeJob(job: QueueJob): QueueJob {
  return { ...job, failure: job.failure ? { ...job.failure } : null };
}

function advanceQueue(): void {
  const now = Date.now();
  const elapsedSeconds = Math.max((now - lastTickAt) / 1000, 0.001);
  lastTickAt = now;

  const active = queue.find((job) => job.id === "demo-job-1");
  if (!active || active.stage === "done") {
    return;
  }

  const nextStage =
    STAGE_SCRIPT[STAGE_SCRIPT.indexOf(active.stage) + 1] ?? "done";
  if (active.stage === "queued" || active.stage === "resolving") {
    active.stage = nextStage;
    active.updatedAt = now;
    publishQueue();
    return;
  }

  if (active.stage === "downloading") {
    const total = active.bytesTotal ?? 0;
    const step = total * 0.09;
    const written = Math.min(active.bytesWritten + step, total);
    active.speedBps = Math.round((written - active.bytesWritten) / elapsedSeconds);
    active.bytesWritten = written;
    active.percent = total > 0 ? Math.round((written / total) * 100) : null;
    active.updatedAt = now;

    if (written >= total) {
      active.stage = "post_processing";
    }
    publishQueue();
    return;
  }

  active.stage = nextStage;
  if (active.stage === "done") {
    active.percent = 100;
  }
  active.updatedAt = now;
  publishQueue();
}

function ensureQueueTicker(): void {
  if (queueTicker !== null) {
    return;
  }
  lastTickAt = Date.now();
  queueTicker = setInterval(() => {
    advanceQueue();
    if (queueListeners.size === 0) {
      clearInterval(queueTicker as ReturnType<typeof setInterval>);
      queueTicker = null;
    }
  }, 420);
}

export const demoExtensions: ExtensionSurface = {
  async extensionsSnapshot() {
    return {
      entries: catalogue(),
      installed: state.installed.map((installed) => ({ ...installed })),
      broken: [],
      refreshError: null,
      fetchedAt: 1_760_000_000,
    };
  },

  async installExtension(id) {
    const entry = catalogue().find((candidate) => candidate.id === id);
    if (!entry) {
      throw failure("unknown", `the catalogue has no entry called ${id}`);
    }
    if (entry.status.state !== "supported") {
      throw failure("refused", `${id} is not installable on this host`);
    }
    if (DEMO_FEATURE_REFUSALS[id]) {
      const missing = DEMO_FEATURE_REFUSALS[id];
      emitInstall({ id, phase: "failed", percent: 0, message: `needs ${missing.join(", ")}` });
      throw failure(
        "unsupported_features",
        `${entry.displayName ?? id} needs ${missing.join(", ")}, which this host does not implement.`,
        missing,
      );
    }
    if (state.installed.some((installed) => installed.id === id)) {
      throw failure("already_installed", `${id} ${entry.version} is already installed`);
    }

    // Walk the phases a real install passes through, so the card animates.
    emitInstall({ id, phase: "queued", percent: 0, message: null });
    await delay(120);
    for (const percent of [12, 34, 58, 76, 91, 100]) {
      emitInstall({ id, phase: "downloading", percent, message: null });
      await delay(90);
    }
    emitInstall({ id, phase: "verifying", percent: 100, message: "checking SHA-256" });
    await delay(140);
    emitInstall({ id, phase: "installing", percent: 100, message: null });
    await delay(120);

    const installed: InstalledExtension = {
      id,
      version: entry.version,
      displayName: entry.displayName,
      sha256: entry.sha256,
      installedAt: Math.floor(Date.now() / 1000),
      enabled: true,
      directory: `demo/extensions/${id}`,
      dataDirectory: `demo/extensions/.data/${id}`,
    };
    state.installed = [...state.installed, installed];
    persist();
    emitInstall({ id, phase: "done", percent: 100, message: null });

    return { ...installed };
  },

  async installExtensionFromFile() {
    // The browser has no file dialog. This stands in for a chosen package so the flow
    // can be walked end to end; the desktop command reads a real path and validates
    // real bytes before anything is written.
    const id = "local-build";
    if (state.installed.some((installed) => installed.id === id)) {
      throw failure("already_installed", "The demo local build is already installed.");
    }

    emitInstall({ id, phase: "verifying", percent: 100, message: "checking local package" });
    await delay(320);
    emitInstall({ id, phase: "installing", percent: 100, message: null });
    await delay(180);

    const installed: InstalledExtension = {
      id,
      version: "0.0.0-demo",
      displayName: "Local build (.sflx)",
      sha256: "",
      installedAt: Math.floor(Date.now() / 1000),
      enabled: true,
      directory: `demo/extensions/${id}`,
      dataDirectory: `demo/extensions/.data/${id}`,
    };
    state.installed = [...state.installed, installed];
    persist();
    emitInstall({ id, phase: "done", percent: 100, message: null });
    return { ...installed };
  },

  async uninstallExtension(id) {
    if (!state.installed.some((installed) => installed.id === id)) {
      throw failure("not_installed", `${id} is not installed`);
    }
    state.installed = state.installed.filter((installed) => installed.id !== id);
    delete state.saved[id];
    delete state.selectedQuality[id];
    demoHealth.delete(id);
    persist();
  },

  async setExtensionEnabled(id, enabled) {
    const installed = state.installed.find((candidate) => candidate.id === id);
    if (!installed) {
      throw failure("not_installed", `${id} is not installed`);
    }
    installed.enabled = enabled;
    persist();
    return { ...installed };
  },

  async extensionSettings(id) {
    return settingsView(id);
  },

  async saveExtensionSettings(id, values, quality) {
    const view = settingsView(id);
    const declared = new Set(view.settings.map((descriptor) => descriptor.key));

    // Unknown keys are dropped rather than stored: the manifest is the schema.
    const filtered: Record<string, unknown> = {};
    Object.entries(values).forEach(([key, value]) => {
      if (declared.has(key)) {
        filtered[key] = value;
      }
    });

    state.saved[id] = filtered;
    if (view.qualities.some((candidate) => candidate.id === quality)) {
      state.selectedQuality[id] = quality;
    }
    persist();
    return settingsView(id);
  },

  async invokeExtensionAction(id, action) {
    if (!state.installed.some((installed) => installed.id === id)) {
      return { ok: false, message: `${id} is not installed` };
    }

    await delay(200);
    if (action === "checkAvailability") {
      return { ok: true, message: "The service answered in 240 ms." };
    }
    if (action === "refreshToken") {
      return {
        ok: false,
        message: "The provider rejected the stored credential. Sign in again.",
      };
    }
    return { ok: false, message: `${id} has no action called ${action}` };
  },

  async providers() {
    return providerGroups();
  },

  async setProviderPriority(capability, ids) {
    state.priority[capability] = ids;
    persist();
    return providerGroups();
  },

  async retryProvider(id) {
    await delay(150);
    const healthy: ProviderHealth = { state: "healthy", message: null, consecutiveFailures: 0 };
    demoHealth.set(id, healthy);
    return { ...healthy };
  },

  async queueJobs() {
    ensureQueueTicker();
    return queue.map(describeJob);
  },

  async cancelJob(jobId) {
    const job = queue.find((candidate) => candidate.id === jobId);
    if (!job) {
      throw failure("unknown", `no job called ${jobId}`);
    }
    // Cancelling removes the row and whatever partial bytes it had.
    queue = queue.filter((candidate) => candidate.id !== jobId);
    publishQueue();
  },

  async retryJob(jobId) {
    const job = queue.find((candidate) => candidate.id === jobId);
    if (!job) {
      throw failure("unknown", `no job called ${jobId}`);
    }
    job.stage = "queued";
    job.percent = 0;
    job.bytesWritten = 0;
    job.speedBps = null;
    job.failure = null;
    job.updatedAt = Date.now();
    publishQueue();
  },

  async enqueueDownload(track) {
    ensureQueueTicker();
    const newJob: QueueJob = {
      id: `demo-job-${Date.now()}`,
      trackId: track.providerTrackId,
      title: track.title,
      artist: track.artist,
      provider: track.provider,
      quality: track.quality ?? "best",
      stage: "queued",
      percent: 0,
      bytesWritten: 0,
      bytesTotal: 9_400 * 1024,
      speedBps: null,
      outputPath: `demo/downloads/${track.artist} - ${track.title}.flac`,
      failure: null,
      updatedAt: Date.now(),
    };
    queue = [...queue, newJob];
    publishQueue();
    return { ...newJob };
  },

  async searchProviders(query, limit) {
    const defaultId = localStorage.getItem("qmusic.defaultSearchProvider");
    let providers = providerGroups()
      .find((group) => group.capability === "metadata")
      ?.providers.filter((provider) => provider.health.state !== "unhealthy") ?? [];

    // If a default provider is set and healthy, search only that one.
    if (defaultId) {
      const defaultProvider = providers.find((p) => p.id === defaultId);
      if (defaultProvider) {
        providers = [defaultProvider];
      }
    }

    if (!providers || providers.length === 0) {
      return [];
    }

    await delay(80);

    return providers.map((provider) => {
      if (provider.id === "bugs-music") {
        return {
          provider: provider.id,
          providerName: provider.displayName,
          results: [],
          failure: "The provider did not answer in time.",
        };
      }
      const results: ProviderTrack[] = [
        {
          provider: provider.id,
          providerName: provider.displayName,
          providerTrackId: `${provider.id}-${query.toLowerCase()}-1`,
          title: `${query} (${provider.displayName} mix)`,
          artist: "Various",
          album: "Synthetic provider result",
          durationMs: 214_000,
          artUrl: null,
          quality: "best",
        },
        {
          provider: provider.id,
          providerName: provider.displayName,
          providerTrackId: `${provider.id}-${query.toLowerCase()}-2`,
          title: `${query} — live`,
          artist: "Various",
          album: "Synthetic provider result",
          durationMs: 268_000,
          artUrl: null,
          quality: "high",
        },
      ].slice(0, limit);

      return {
        provider: provider.id,
        providerName: provider.displayName,
        results,
        failure: null,
      };
    });
  },

  async onlineSearch(requestId, query, types, limit, _offset) {
    await delay(80);
    const providers = providerGroups()
      .find((group) => group.capability === "metadata")
      ?.providers.filter((p) => p.health.state !== "unhealthy") ?? [];
    const tracks = providers.flatMap((provider) => [
      {
        id: `${provider.id}:demo-1`,
        provider: provider.id,
        providerName: provider.displayName,
        providerTrackId: `demo-1`,
        title: `${query} (${provider.displayName} mix)`,
        artists: ["Various"],
        album: "Synthetic provider result",
        durationMs: 214_000,
        artUrl: null,
        explicit: false,
        inLibrary: false,
        itemType: "track",
      },
    ]).slice(0, limit);
    return {
      requestId,
      query,
      tracks: types.length === 0 || types.includes("tracks") ? tracks : [],
      albums: [],
      artists: [],
      playlists: [],
      providers: providers.map((p) => ({
        id: p.id,
        name: p.displayName,
        state: "ok",
        error: null,
        elapsedMs: 80,
        hitCounts: { tracks: tracks.length, albums: 0, artists: 0, playlists: 0 },
      })),
      elapsedMs: 80,
      cancelled: false,
    };
  },

  async onlineSearchCancel(_requestId) {},

  async providerDetail(_kind, _provider, _id) {
    return null;
  },

  async providerHome() {
    return { providers: [] };
  },

  async handleUrl(url) {
    await delay(120);
    return {
      provider: "spotify-web",
      item: {
        type: "track",
        track: {
          id: url,
          name: "Resolved from link",
          artists: ["Demo Artist"],
          duration_ms: 214000,
          images: [],
        },
      },
    };
  },

  async providerHomeFeed(id) {
    await delay(180);
    const trackItem = (title: string, i: number) => ({
      id: `${id}-track-${i}`,
      type: "track",
      title,
      artists: ["Demo Artist"],
      images: [],
      duration_ms: 180000 + i * 1000,
    });
    const albumItem = (title: string, i: number) => ({
      id: `${id}-album-${i}`,
      type: "album",
      title,
      artists: ["Demo Artist"],
      images: [],
    });
    return {
      provider: id,
      item: {
        sections: [
          {
            title: "Made for you",
            items: [trackItem("Night Ferry", 1), trackItem("Salt Flat", 2), trackItem("Blue Interval", 3)],
          },
          {
            title: "New releases",
            items: [albumItem("Blue Interval", 1), albumItem("Copper Hours", 2)],
          },
        ],
      },
    };
  },

  async providerBrowseCategories(id) {
    await delay(120);
    return {
      provider: id,
      item: {
        categories: [
          { id: "pop", name: "Pop" },
          { id: "rock", name: "Rock" },
          { id: "jazz", name: "Jazz" },
          { id: "electronic", name: "Electronic" },
          { id: "classical", name: "Classical" },
        ],
      },
    };
  },

  async providerLyrics(trackId) {
    await delay(200);
    return {
      provider: "apple-music",
      providerName: "Apple Music",
      trackId,
      synced: true,
      lines: [
        { timeMs: 12_000, text: "the ferry pulls away from the quay" },
        { timeMs: 18_400, text: "and the harbour lights go dim" },
        { timeMs: 24_900, text: "i keep the ticket in my coat" },
        { timeMs: 31_200, text: "like a promise i can hold" },
        { timeMs: 38_600, text: "night ferry, carry me home" },
      ],
    };
  },

  async onExtensionProgress(handler) {
    installListeners.add(handler);
    return () => {
      installListeners.delete(handler);
    };
  },

  async onQueue(handler) {
    queueListeners.add(handler);
    ensureQueueTicker();
    return () => {
      queueListeners.delete(handler);
    };
  },
};
