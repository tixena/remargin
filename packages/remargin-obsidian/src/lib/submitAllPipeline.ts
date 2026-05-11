import type {
  StagedGroup,
  SubmitGroupResult,
  SubmitProgress,
} from "@/components/sidebar/buildPromptGroups";

/**
 * Run the per-group Submit-all pipeline. Sequential by design (per
 * `ugk`); continue-on-failure (per `nzr`). Exposed as a pure function
 * so the orchestration is testable without React.
 *
 * The caller supplies the side-effecting functions (`runGroup`,
 * `cleanupGroup`) so unit tests can substitute mocks. `bumpRefresh` is
 * optional and fires after each successful group + once at the end so
 * the UI can re-render progressively.
 */
export async function runSubmitAll(args: {
  groups: StagedGroup[];
  runGroup: (group: StagedGroup) => Promise<void>;
  cleanupGroup: (group: StagedGroup) => Promise<void>;
  bumpRefresh?: () => void;
  progress?: SubmitProgress;
  now?: () => number;
}): Promise<SubmitGroupResult[]> {
  const now = args.now ?? (() => Date.now());
  const results: SubmitGroupResult[] = [];
  for (const group of args.groups) {
    const started = now();
    args.progress?.onGroupStart?.(group);
    try {
      await args.runGroup(group);
      try {
        await args.cleanupGroup(group);
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        const durationMs = now() - started;
        results.push({ group, ok: true, error: `cleanup failed: ${error}`, durationMs });
        args.progress?.onGroupComplete?.(group, { ok: true, error });
        args.bumpRefresh?.();
        continue;
      }
      results.push({ group, ok: true, durationMs: now() - started });
      args.progress?.onGroupComplete?.(group, { ok: true });
      args.bumpRefresh?.();
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      results.push({ group, ok: false, error, durationMs: now() - started });
      args.progress?.onGroupComplete?.(group, { ok: false, error });
    }
  }
  return results;
}
