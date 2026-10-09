/** The option list of the recipient picker. */

import type { Participant } from "@/backend";

/**
 * Filter a raw participant list down to the options a `RecipientPicker`
 * should show:
 *
 * 1. Drop revoked participants — they can't post, so they can't receive
 *    a new comment either.
 * 2. Drop participants already in the `selected` list — the CLI dedupes
 *    repeated recipients, but the picker should never offer a selected id.
 * 3. Dedup by participant id, keeping the first entry.
 *
 * Input order is preserved so the picker reflects the registry's
 * natural ordering.
 */
export function pickerOptions(
  participants: readonly Participant[],
  selected: readonly string[]
): Participant[] {
  const selectedSet = new Set(selected);
  const seen = new Set<string>();
  const out: Participant[] = [];
  for (const p of participants) {
    if (p.status !== "active") continue;
    if (selectedSet.has(p.name)) continue;
    if (seen.has(p.name)) continue;
    seen.add(p.name);
    out.push(p);
  }
  return out;
}
