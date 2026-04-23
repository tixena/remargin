/**
 * Helpers for the sidebar's `remargin_kind` filter chips (rem-u8br).
 *
 * Keep these pure and synchronous: they run on every render of the
 * Inbox and Current-file sections to both (a) build the chip set and
 * (b) decide which comments the current filter keeps. The filter is
 * OR-semantic to match the CLI (`--kind question --kind action-item`
 * surfaces comments tagged with EITHER value).
 */

/**
 * A structural view over anything the sidebar renders — the real
 * payloads are `Comment` and `ExpandedComment`, both of which declare
 * `remargin_kind?: Array<string>` (optional on the wire so pre-field
 * comments round-trip without the key). Narrowed to just the field
 * we read so tests can pass plain object literals without fabricating
 * identities, timestamps, signatures, etc.
 */
export interface HasRemarginKind {
  remargin_kind?: string[];
}

/**
 * Collect the sorted, de-duplicated set of `remargin_kind` values
 * present in the supplied items. Used to drive the chip row in the
 * sidebar: only kinds that actually appear in the visible data get a
 * chip, matching the AC's "values present in the visible set".
 *
 * Sort is case-insensitive so `Question` and `question` would appear
 * next to each other if both existed, but the stored casing wins for
 * display. Validation on the CLI side already canonicalizes input, so
 * in practice duplicates only differ by character-level equality.
 */
export function collectKinds(items: Iterable<HasRemarginKind>): string[] {
  const seen = new Set<string>();
  for (const item of items) {
    for (const kind of item.remargin_kind ?? []) {
      if (kind.length > 0) seen.add(kind);
    }
  }
  return Array.from(seen).sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
}

/**
 * OR-match a comment's kind list against the user's filter selection.
 *
 * Semantics:
 *   - An empty filter matches every comment (the "no filter" state).
 *   - A non-empty filter matches when the comment carries at least one
 *     of the selected kinds.
 *
 * Mirrors `remargin_core::kind::matches_kind_filter` so a future
 * migration to server-side filtering is a no-op from the UI's side.
 */
export function matchesKindFilter(
  kinds: readonly string[] | undefined,
  filter: readonly string[],
): boolean {
  if (filter.length === 0) return true;
  if (!kinds) return false;
  for (const k of kinds) {
    if (filter.includes(k)) return true;
  }
  return false;
}

/**
 * Drop kinds from the filter that are no longer present in the
 * visible set. Called whenever the Inbox or Current-file data
 * refetches: if the user filters on `question`, marks every question
 * acked, and the inbox switches to Pending-only, the chip set shrinks
 * and the stale selection must shrink with it — otherwise the header
 * still claims an active filter but the chip row is empty.
 */
export function pruneKindFilter(filter: string[], available: string[]): string[] {
  if (filter.length === 0) return filter;
  const availableSet = new Set(available);
  const next = filter.filter((k) => availableSet.has(k));
  return next.length === filter.length ? filter : next;
}
