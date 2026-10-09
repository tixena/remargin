/** Pure helpers behind the `useParticipants` hook, testable without a React renderer. */

import type { Participant, RemarginBackend } from "@/backend";
import type { RemarginSettings } from "@/types";

/**
 * Build a stable cache key from the subset of settings that could change
 * which registry the CLI resolves. Anything outside this list (e.g. the
 * sidebar side or view mode) has no effect on `registry show` and is
 * intentionally excluded to avoid unnecessary refetches.
 */
export function participantsCacheKey(settings: RemarginSettings): string {
  return [
    settings.workingDirectory,
    settings.remarginPath,
    settings.authorName,
    settings.identityMode,
    settings.configFilePath,
  ].join("|");
}

/**
 * Resolve a participant id to its registered `display_name`. Falls back to
 * the id itself in three cases:
 *
 * 1. `participants` is `undefined` (fetch has not resolved yet)
 * 2. The id is not present in the registry
 * 3. The entry has no display name (shouldn't happen because the CLI
 *    fills in a default, but we still cover it for safety)
 *
 * Always returns a non-empty string, so callers can drop it directly into
 * React children without null-coalescing.
 */
export function resolveDisplayNameFrom(
  participants: readonly Participant[] | undefined,
  id: string
): string {
  if (!participants) return id;
  const hit = participants.find((p) => p.name === id);
  if (!hit) return id;
  return hit.display_name || id;
}

/**
 * Run `backend.registryShow()` and swallow any error to `[]`, logging it
 * for debugging. The hook uses this so a transient CLI failure renders as
 * "no participants" instead of an unhandled rejection.
 */
export async function loadParticipants(backend: RemarginBackend): Promise<Participant[]> {
  try {
    return await backend.registryShow();
  } catch (err) {
    // The hook has no user-facing error channel, so the failure goes to the devtools console.
    console.error("useParticipants: failed to load registry:", err);
    return [];
  }
}
