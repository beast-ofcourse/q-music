import type { ProviderCapability, ProviderHealth, ProviderView } from "../../lib/extensions";

const CAPABILITY_LABEL: Record<ProviderCapability, string> = {
  metadata: "Metadata",
  download: "Download",
  lyrics: "Lyrics",
};

const HEALTH_LABEL: Record<ProviderHealth["state"], string> = {
  healthy: "Healthy",
  unhealthy: "Unhealthy",
  unknown: "Not called yet",
};

export function capabilityLabel(capability: ProviderCapability): string {
  return CAPABILITY_LABEL[capability];
}

export function healthLabel(health: ProviderHealth): string {
  return HEALTH_LABEL[health.state];
}

/**
 * The health word alone does not say what happened, and a bare failure count does not
 * say why. A row needs both, so they are composed here rather than at the call site.
 */
export function healthDetail(health: ProviderHealth): string | null {
  if (health.state === "healthy") {
    return null;
  }

  const failures = health.consecutiveFailures;
  const streak =
    failures === 0 ? null : failures === 1 ? "1 call failed in a row." : `${failures} calls failed in a row.`;

  if (health.message === null) {
    return streak;
  }
  return streak === null ? health.message : `${health.message} ${streak}`;
}

/** The provider stores a quality id; the row shows the label a person can read. */
export function qualityLabel(provider: ProviderView): string {
  const match = provider.qualities.find((quality) => quality.id === provider.quality);
  return match?.label ?? provider.quality;
}
