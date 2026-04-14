import type { ExpandedComment } from "@/generated";

/**
 * Three visual states an inbox leaf can render in:
 *
 * - `me-directed-unacked` — the comment is either broadcast (`to: []`)
 *   or explicitly addressed to `me`, and I have not acked it yet.
 *   Rendered with a purple accent and an inline "Ack" button.
 * - `acked-by-me` — I have already acked this comment (regardless of
 *   whether it was addressed to me). Rendered dimmed with an ellipsis
 *   menu exposing only "Unack".
 * - `neutral` — not mine and not acked by me. Current default styling.
 */
export type LeafVisual = "me-directed-unacked" | "acked-by-me" | "neutral";

export interface LeafState {
  directedAtMe: boolean;
  ackedByMe: boolean;
  visual: LeafVisual;
}

/**
 * Derive the inbox-leaf visual state for a single comment. Pure and
 * hook-free so the decision table is covered by the unit tests and the
 * component can call it unconditionally at render time.
 *
 * `me` is `null`/`undefined` until the CLI identity probe resolves; in
 * that case every leaf is `neutral` and no Ack affordance renders —
 * avoids flashing a purple accent onto unrelated rows while the probe
 * is in flight.
 *
 * The `to: []` case is treated as "broadcast / directed at everyone",
 * matching the CLI's pending-for semantics: a comment without explicit
 * recipients is addressed to the document's author and any reader.
 */
export function deriveLeafState(
  comment: Pick<ExpandedComment, "to" | "ack">,
  me: string | null | undefined
): LeafState {
  if (!me) {
    return { directedAtMe: false, ackedByMe: false, visual: "neutral" };
  }
  const toList = comment.to ?? [];
  const directedAtMe = toList.length === 0 || toList.includes(me);
  const ackedByMe = (comment.ack ?? []).some((entry) => entry.author === me);
  const visual: LeafVisual = ackedByMe
    ? "acked-by-me"
    : directedAtMe
      ? "me-directed-unacked"
      : "neutral";
  return { directedAtMe, ackedByMe, visual };
}
