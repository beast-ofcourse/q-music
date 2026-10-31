/**
 * Ordering for the two versions a card can show at once: the catalogue's and the
 * installed copy's. The contract publishes no comparator, so this reads only the
 * numeric release part; build metadata and prerelease tags are deliberately not
 * interpreted. A caller that gets 0 must treat the versions as the same release.
 */
export function compareVersions(left: string, right: string): number {
  const leftParts = releaseNumbers(left);
  const rightParts = releaseNumbers(right);

  for (let index = 0; index < 3; index += 1) {
    const difference = (leftParts[index] ?? 0) - (rightParts[index] ?? 0);
    if (difference !== 0) {
      return difference;
    }
  }
  return 0;
}

function releaseNumbers(version: string): number[] {
  const release = version.split(/[-+]/, 1)[0] ?? "";
  return release.split(".").map((part) => {
    const value = Number.parseInt(part, 10);
    return Number.isFinite(value) ? value : 0;
  });
}
