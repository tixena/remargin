import { isNewer } from "./semver";

/**
 * Raw GitHub REST API release object (subset we actually consume). See
 * https://docs.github.com/en/rest/releases/releases.
 */
export interface GithubRelease {
  tag_name: string;
  name?: string | null;
  html_url?: string;
  draft?: boolean;
  prerelease?: boolean;
  published_at?: string | null;
}

/** Repository coordinates for the remargin project's GitHub releases. */
export const REMARGIN_RELEASES_URL =
  "https://api.github.com/repos/tixena/remargin/releases";

/**
 * Component identifiers used throughout the update-check pipeline.
 *
 * - `plugin`: the Obsidian plugin itself; tags are `obsidian-vX.Y.Z`.
 * - `cli`: the remargin CLI; tags are `vX.Y.Z` (the repo's "default"
 *   tag family — anything that is not an `obsidian-` plugin tag).
 */
export type UpdateComponent = "plugin" | "cli";

/** Per-component status tracked in the cache + reported to the UI. */
export type UpdateStatus = "up-to-date" | "update-available" | "check-failed";

export interface ComponentCheck {
  status: UpdateStatus;
  installed: string;
  latest: string | null;
  tag: string | null;
  releaseUrl: string | null;
  /** Populated only when `status === "check-failed"`. */
  error?: string;
}

export interface UpdateCheckState {
  plugin: ComponentCheck;
  cli: ComponentCheck;
  /** ISO timestamp of when this snapshot was produced. */
  lastCheckedAt: string;
}

/**
 * Minimal fetch abstraction used by the update-check pipeline. The plugin
 * plugs in Obsidian's `requestUrl` (which bypasses the browser CORS policy
 * and is desktop-safe); tests inject a pure in-memory stub so they never
 * hit the network.
 *
 * `ok` + `status` mirror the familiar `fetch` shape, and `body` is the raw
 * JSON payload text — callers run it through `JSON.parse` themselves.
 */
export interface ReleasesFetcher {
  (url: string): Promise<{ ok: boolean; status: number; body: string }>;
}

/**
 * Tag-convention hook. `obsidian-v*` tags track the plugin; every other
 * tag (including the canonical `vX.Y.Z`) tracks the CLI. This matches the
 * repo's historical release practice — verified against git tags before
 * implementing.
 */
export function classifyTag(tag: string): UpdateComponent | null {
  if (!tag) return null;
  if (tag.startsWith("obsidian-v")) return "plugin";
  if (/^v\d/.test(tag)) return "cli";
  return null;
}

/**
 * Split a raw release list into the latest plugin release and the latest
 * CLI release.
 *
 * - Drafts and prereleases are skipped.
 * - When `published_at` is set, the most recently published release wins.
 *   When it is missing, the first matching release in the list wins — which
 *   matches GitHub's default descending ordering.
 */
export function splitReleases(releases: GithubRelease[]): {
  plugin: GithubRelease | null;
  cli: GithubRelease | null;
} {
  const pickLatest = (
    current: GithubRelease | null,
    candidate: GithubRelease
  ): GithubRelease => {
    if (!current) return candidate;
    const currentAt = current.published_at ? Date.parse(current.published_at) : Number.NaN;
    const candidateAt = candidate.published_at
      ? Date.parse(candidate.published_at)
      : Number.NaN;
    if (Number.isFinite(currentAt) && Number.isFinite(candidateAt)) {
      return candidateAt > currentAt ? candidate : current;
    }
    // Missing timestamps: trust the input order (GitHub returns newest first).
    return current;
  };

  let plugin: GithubRelease | null = null;
  let cli: GithubRelease | null = null;
  for (const release of releases) {
    if (release.draft || release.prerelease) continue;
    const kind = classifyTag(release.tag_name);
    if (kind === "plugin") plugin = pickLatest(plugin, release);
    else if (kind === "cli") cli = pickLatest(cli, release);
  }
  return { plugin, cli };
}

/**
 * Build an initial per-component record for the `check-failed` path. Keeps
 * the shape stable regardless of which branch of `runUpdateCheck` emitted
 * it, so UI code can render both outcomes with the same renderer.
 */
function failed(installed: string, error: string): ComponentCheck {
  return {
    status: "check-failed",
    installed,
    latest: null,
    tag: null,
    releaseUrl: null,
    error,
  };
}

/**
 * Map an `installed` + `release` pair to a `ComponentCheck`. A missing
 * release means the repo has no tag of that family yet — report as
 * `up-to-date` with `latest: null` so the UI does not falsely claim an
 * update. An unparseable installed version is reported as `check-failed`
 * so the user can still spot the misconfiguration.
 */
export function compareComponent(
  installed: string,
  release: GithubRelease | null
): ComponentCheck {
  if (!installed || installed === "unknown") {
    return failed(installed, "installed version unknown");
  }
  if (!release) {
    return {
      status: "up-to-date",
      installed,
      latest: null,
      tag: null,
      releaseUrl: null,
    };
  }
  const latestStatus: UpdateStatus = isNewer(release.tag_name, installed)
    ? "update-available"
    : "up-to-date";
  return {
    status: latestStatus,
    installed,
    latest: release.name?.trim() || release.tag_name,
    tag: release.tag_name,
    releaseUrl: release.html_url ?? null,
  };
}

/**
 * Arguments to `runUpdateCheck`. Kept as an options object so the plugin
 * can add future knobs (alternate repo, custom fetcher, injected clock)
 * without breaking the call site.
 */
export interface RunUpdateCheckArgs {
  installedPlugin: string;
  installedCli: string;
  fetcher: ReleasesFetcher;
  now?: () => Date;
  url?: string;
}

/**
 * Execute a full update check: fetch releases, classify tags, compare
 * versions, and assemble an `UpdateCheckState` snapshot.
 *
 * Any error from the fetcher, non-2xx HTTP response, or malformed payload
 * is folded into per-component `check-failed` statuses rather than thrown
 * — the plugin is supposed to fail silently on the network path, so the
 * error string is kept for logging but never surfaces as a user-facing
 * Notice.
 */
export async function runUpdateCheck(args: RunUpdateCheckArgs): Promise<UpdateCheckState> {
  const { installedPlugin, installedCli, fetcher } = args;
  const now = args.now ?? (() => new Date());
  const url = args.url ?? REMARGIN_RELEASES_URL;

  let releases: GithubRelease[] = [];
  let error: string | null = null;
  try {
    const response = await fetcher(url);
    if (!response.ok) {
      error = `github releases: HTTP ${response.status}`;
    } else {
      const parsed = JSON.parse(response.body) as unknown;
      if (Array.isArray(parsed)) {
        releases = parsed as GithubRelease[];
      } else {
        error = "github releases: payload is not a JSON array";
      }
    }
  } catch (err) {
    error = err instanceof Error ? err.message : "github releases: fetch failed";
  }

  const timestamp = now().toISOString();
  if (error) {
    return {
      plugin: failed(installedPlugin, error),
      cli: failed(installedCli, error),
      lastCheckedAt: timestamp,
    };
  }
  const { plugin, cli } = splitReleases(releases);
  return {
    plugin: compareComponent(installedPlugin, plugin),
    cli: compareComponent(installedCli, cli),
    lastCheckedAt: timestamp,
  };
}

/** Cache TTL for successful checks. Failed checks retry on every load. */
export const UPDATE_CACHE_TTL_MS = 24 * 60 * 60 * 1000;

/**
 * `true` when the cached snapshot is still fresh enough to skip a fetch.
 *
 * A missing cache, a failed-check cache, or a timestamp older than 24h
 * all return `false`. Force-refresh is handled by the caller — this
 * function is a pure predicate.
 */
export function isCacheFresh(
  state: UpdateCheckState | null | undefined,
  now: Date = new Date(),
  ttlMs: number = UPDATE_CACHE_TTL_MS
): boolean {
  if (!state) return false;
  // Never reuse a failed snapshot: the next load should retry.
  if (state.plugin.status === "check-failed" || state.cli.status === "check-failed") {
    return false;
  }
  const then = Date.parse(state.lastCheckedAt);
  if (!Number.isFinite(then)) return false;
  return now.getTime() - then < ttlMs;
}

/**
 * `true` when the `after` snapshot has at least one component that moved
 * from `up-to-date` (or is freshly observed) to `update-available`
 * compared to `before`.
 *
 * Used by the plugin to decide whether to fire the startup Notice: we
 * only want to nag the user once per newly published release, not on
 * every reload while the update sits on the shelf.
 */
export function detectNewUpdates(
  before: UpdateCheckState | null | undefined,
  after: UpdateCheckState
): UpdateComponent[] {
  const changed: UpdateComponent[] = [];
  for (const component of ["plugin", "cli"] as const) {
    const nextCheck = after[component];
    if (nextCheck.status !== "update-available") continue;
    const prevCheck = before?.[component];
    // Fire when:
    //   - we've never observed an update before, or
    //   - the previous snapshot wasn't flagging this component, or
    //   - the latest tag advanced (a new release landed).
    if (
      !prevCheck ||
      prevCheck.status !== "update-available" ||
      prevCheck.tag !== nextCheck.tag
    ) {
      changed.push(component);
    }
  }
  return changed;
}
