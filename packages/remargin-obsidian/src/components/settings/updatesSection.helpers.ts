import type { ComponentCheck } from "@/lib/githubReleases";

/**
 * Pure helpers used by `UpdatesSection.tsx`, factored out so they can
 * be unit-tested without instantiating React — the test runner uses
 * a strip-only TS loader that struggles with full React component
 * rendering, and every path through the UI is deterministically driven
 * by these three functions.
 */

/** Human-readable status string for the chip next to each component. */
export function statusLabel(check: ComponentCheck): string {
  if (check.status === "check-failed") return "check failed";
  if (check.status === "update-available") return "update available";
  return "up to date";
}

/**
 * Tailwind class bundle for the status chip. Matches the design doc
 * (accent-filled for update-available, red-tinted for check-failed,
 * muted for up-to-date).
 */
export function statusChipClasses(check: ComponentCheck): string {
  if (check.status === "update-available") {
    return "bg-accent text-white";
  }
  if (check.status === "check-failed") {
    return "bg-red-500/10 text-red-400 border border-red-500/40";
  }
  return "bg-bg-secondary text-text-muted border border-bg-border";
}

/**
 * Trim a long error string to its last `max` characters (default 200),
 * prefixing `…` when truncation happens. Keeps multi-line stderr tails
 * readable inside an Obsidian Notice or an inline status banner.
 */
export function tailText(text: string, max = 200): string {
  const trimmed = text.trim();
  if (trimmed.length <= max) return trimmed;
  return `…${trimmed.slice(trimmed.length - max)}`;
}

/**
 * `true` when the plugin row's Update button should be clickable. Kept
 * as a named predicate so the intent — "chip reads update-available" —
 * stays explicit everywhere it matters (test harness, button `disabled`
 * wiring, future UI variants).
 */
export function canUpdatePlugin(check: ComponentCheck | undefined): boolean {
  return check?.status === "update-available";
}

/**
 * Terminal state of the section's inline status banner after an
 * async action resolves. `null` when no banner should be shown.
 */
export type UpdateActionMessage = { ok: boolean; text: string } | null;

/**
 * Map the result of `onUpdatePlugin` to the inline-banner text the
 * section should render. Extracted so the click flow has a single
 * place to own the success + failure copy, and so the branch can
 * be unit-tested directly without booting React.
 */
export function messageForUpdate(
  result: { ok: boolean; stderr: string } | Error
): UpdateActionMessage {
  if (result instanceof Error) {
    return {
      ok: false,
      text: `Update failed: ${tailText(result.message) || "unknown error"}`,
    };
  }
  if (result.ok) {
    return {
      ok: true,
      text: "Remargin plugin updated — reload Obsidian to finish.",
    };
  }
  return {
    ok: false,
    text: `Update failed: ${tailText(result.stderr) || "unknown error"}`,
  };
}

/**
 * Map the result of `onCheckNow` to the inline banner text. The Check
 * now button always reports something so the user knows the click
 * landed — a quiet refresh looks indistinguishable from a dead button.
 */
export function messageForCheck(result: void | Error): UpdateActionMessage {
  if (result instanceof Error) {
    return {
      ok: false,
      text: result.message || "Check failed",
    };
  }
  return { ok: true, text: "Checked." };
}
