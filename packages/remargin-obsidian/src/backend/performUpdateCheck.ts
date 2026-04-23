import {
  isCacheFresh,
  type ReleasesFetcher,
  runUpdateCheck,
  type UpdateCheckState,
} from "@/lib/githubReleases";

/**
 * Standalone update-check orchestrator, factored out of `RemarginBackend`
 * so tests can import it without pulling in the class (whose
 * parameter-property constructor the test-runner's strip-only TypeScript
 * loader cannot parse — see `identityAcceptingSubcommands.ts` for the
 * same workaround pattern).
 *
 * Behavior mirrors `RemarginBackend.checkForUpdates`:
 *
 *   - Cache fresh + `force: false` -> return the cache unchanged (no fetcher
 *     call, no CLI version probe).
 *   - Otherwise -> probe the CLI for its version (`cliVersion`), falling
 *     back to `"unknown"` on any error, then run the full GitHub-releases
 *     comparison via `runUpdateCheck`.
 */
export interface PerformUpdateCheckArgs {
  force: boolean;
  installedPlugin: string;
  fetcher: ReleasesFetcher;
  /**
   * Async probe for the CLI's installed version string. Expected to return
   * something like `"remargin 0.4.2"`. Errors are swallowed and translated
   * to `"unknown"`, which the comparator flags as `check-failed`.
   */
  cliVersion: () => Promise<string>;
  cache?: UpdateCheckState;
  now?: () => Date;
}

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
