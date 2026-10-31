/** Pure transforms over the host's provider groups.
 *
 *  Reordering is a backend write, not a local sort: the list this file returns for a move
 *  is only a proposal. Whatever `setProviderPriority` answers becomes the new truth, so
 *  every displayed order and every announcement is read back from that answer. */

import type {
  ProviderCapability,
  ProviderGroup,
  ProviderHealth,
  ProviderView,
} from "../../lib/extensions";

export const CAPABILITY_ORDER: ProviderCapability[] = ["metadata", "download", "lyrics"];

const CAPABILITY_TITLES: Record<ProviderCapability, string> = {
  metadata: "Metadata",
  download: "Download",
  lyrics: "Lyrics",
};

export function capabilityTitle(capability: ProviderCapability): string {
  return CAPABILITY_TITLES[capability];
}

/** The contract says all three groups always exist; a missing one renders as empty. */
export function normaliseGroups(groups: ProviderGroup[]): ProviderGroup[] {
  return CAPABILITY_ORDER.map(
    (capability) =>
      groups.find((group) => group.capability === capability) ?? {
        capability,
        providers: [],
      },
  );
}

export function groupFor(
  groups: ProviderGroup[],
  capability: ProviderCapability,
): ProviderGroup | null {
  return groups.find((group) => group.capability === capability) ?? null;
}

/**
 * The complete id list to send for one single-step move, or null when the provider is
 * unknown or already at that end of the list.
 */
export function orderedIdsAfterMove(
  group: ProviderGroup,
  providerId: string,
  direction: -1 | 1,
): string[] | null {
  const from = group.providers.findIndex((provider) => provider.id === providerId);
  if (from === -1) {
    return null;
  }

  const to = from + direction;
  if (to < 0 || to >= group.providers.length) {
    return null;
  }

  const ids = group.providers.map((provider) => provider.id);
  const moving = ids[from];
  const displaced = ids[to];
  if (moving === undefined || displaced === undefined) {
    return null;
  }

  ids[from] = displaced;
  ids[to] = moving;
  return ids;
}

/** Composed from the returned group, so the spoken position cannot drift from the list. */
export function describeMove(
  groups: ProviderGroup[],
  capability: ProviderCapability,
  providerId: string,
): string | null {
  const group = groupFor(groups, capability);
  if (group === null) {
    return null;
  }

  const index = group.providers.findIndex((provider) => provider.id === providerId);
  const provider = group.providers[index];
  if (index === -1 || provider === undefined) {
    return null;
  }

  return `${provider.displayName} moved to position ${index + 1} of ${group.providers.length}.`;
}

/** `retryProvider` answers with one provider's health; the rest of the groups stand. */
export function withProviderHealth(
  groups: ProviderGroup[],
  providerId: string,
  health: ProviderHealth,
): ProviderGroup[] {
  return groups.map((group) => ({
    ...group,
    providers: group.providers.map((provider) =>
      provider.id === providerId ? { ...provider, health } : provider,
    ),
  }));
}

export function qualityLabel(provider: ProviderView): string {
  const descriptor = provider.qualities.find((quality) => quality.id === provider.quality);
  return descriptor?.label ?? provider.quality;
}
