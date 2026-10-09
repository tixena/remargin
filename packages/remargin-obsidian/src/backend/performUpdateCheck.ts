/** Update-check orchestration, kept apart from `RemarginBackend` so tests can import it. */

import {
  isCacheFresh,
  type ReleasesFetcher,
  runUpdateCheck,
  type UpdateCheckState,
} from "@/lib/githubReleases";

/** Inputs to {@link performUpdateCheck}. */
export interface PerformUpdateCheckArgs {
  force: boolean;
  installedPlugin: string;
  fetcher: ReleasesFetcher;
  /** A rejection becomes `"unknown"`, which the comparator flags as `check-failed`. */
  cliVersion: () => Promise<string>;
  cache?: UpdateCheckState;
  now?: () => Date;
}

/**
 * Runs the update check: a fresh cache with `force: false` is returned unchanged; otherwise the
 * CLI is probed for its version (`"unknown"` on any error) and compared against GitHub releases
 * through `runUpdateCheck`.
 */
export async function performUpdateCheck(args: PerformUpdateCheckArgs): Promise<UpdateCheckState> {
  const now = args.now ?? (() => new Date());
  if (!args.force && isCacheFresh(args.cache, now())) {
    return args.cache as UpdateCheckState;
  }
  let installedCli = "unknown";
  try {
    installedCli = await args.cliVersion();
  } catch {
    // Keep "unknown" so the comparator marks the CLI column as
    // `check-failed`; the plugin can still report the plugin column.
  }
  return runUpdateCheck({
    installedPlugin: args.installedPlugin,
    installedCli,
    fetcher: args.fetcher,
    now,
  });
}
