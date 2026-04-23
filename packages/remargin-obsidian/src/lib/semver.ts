/**
 * Small, self-contained semver helpers for the update-check flow.
 *
 * The only release tags this plugin cares about are produced by the
 * remargin repo itself (`obsidian-vX.Y.Z` for the plugin, `vX.Y.Z` for the
 * CLI), so we do not need full semver-spec compliance — just enough to
 * compare dotted numeric triples with an optional leading `v` and an
 * optional pre-release suffix (`-rc.1`, etc.) that always sorts lower.
 *
 * Keeping this tiny module lets us avoid an extra npm dependency (important
 * for an Obsidian plugin — the esbuild bundle is shipped to users), and
 * lets the tests run without a network or any browser shim.
 */

export interface ParsedVersion {
  major: number;
  minor: number;
  patch: number;
  /** `undefined` for stable releases, otherwise the raw `-suffix` text. */
  prerelease?: string;
}

/**
 * Parse a version string into its numeric triple + optional prerelease.
 *
 * Accepts inputs like:
 *   - `0.1.6`
 *   - `v0.1.6`
 *   - `obsidian-v0.1.6`
 *   - `remargin 0.4.2` (first token-that-looks-like-a-version wins)
 *   - `0.1.6-rc.1`
 *
 * Returns `null` when no `X.Y.Z` triple can be located.
 */
export function parseVersion(raw: string): ParsedVersion | null {
  if (!raw) return null;
  const match = raw.match(/(\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.-]+)?/);
  if (!match) return null;
  const major = Number.parseInt(match[1] ?? "", 10);
  const minor = Number.parseInt(match[2] ?? "", 10);
  const patch = Number.parseInt(match[3] ?? "", 10);
  if (!Number.isFinite(major) || !Number.isFinite(minor) || !Number.isFinite(patch)) {
    return null;
  }
  return {
    major,
    minor,
    patch,
    prerelease: match[4] ? match[4].slice(1) : undefined,
  };
}

/**
 * Lexicographic compare of two parsed versions.
 *
 * Returns a negative number when `a < b`, zero when equal, positive when
 * `a > b`. A stable release always sorts above a prerelease of the same
 * `X.Y.Z`. Prerelease suffixes are compared as plain strings (good enough
 * for our `rc.N` / `beta.N` patterns; we are not implementing the full
 * semver precedence rules).
 */
export function compareVersions(a: ParsedVersion, b: ParsedVersion): number {
  if (a.major !== b.major) return a.major - b.major;
  if (a.minor !== b.minor) return a.minor - b.minor;
  if (a.patch !== b.patch) return a.patch - b.patch;
  // Stable > prerelease.
  if (a.prerelease === undefined && b.prerelease !== undefined) return 1;
  if (a.prerelease !== undefined && b.prerelease === undefined) return -1;
  if (a.prerelease === undefined && b.prerelease === undefined) return 0;
  return (a.prerelease as string).localeCompare(b.prerelease as string);
}

/**
 * `true` when `latest` strictly outranks `installed`. `false` in every other
 * case — including when either version is unparseable, so an ambiguous
 * read never surfaces a bogus "update available" Notice.
 */
export function isNewer(latestRaw: string, installedRaw: string): boolean {
  const latest = parseVersion(latestRaw);
  const installed = parseVersion(installedRaw);
  if (!latest || !installed) return false;
  return compareVersions(latest, installed) > 0;
}
