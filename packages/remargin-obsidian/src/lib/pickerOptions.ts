import type { Participant } from "@/backend";

/**
 * Filter a raw participant list down to the options a `RecipientPicker`
 * should show:
 *
 * 1. Drop revoked participants — they can't post, so they can't receive
 *    a new comment either (historical comments from revoked authors
 *    still render their display name, that's a separate concern).
 * 2. Drop participants already in the `selected` list — repeated
 *    recipients are silently deduped by the CLI (task 30) but UX-wise
 *    the picker should never let a user "re-select" an id.
 * 3. Dedup by participant id, keeping the first entry. Defensive against
 *    a future registry shape that lists the same id twice; also keeps
 *    the helper total.
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
