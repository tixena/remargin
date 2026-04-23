/**
 * Three visual states for the Ack toggle on a comment card.
 *
 * `me-acked` — the current identity is in the ack list (click removes).
 * `others-acked` — at least one ack, but the current identity is absent
 *   (click adds).
 * `unacked` — nobody has acked yet (click adds; count is hidden).
 */
export type AckState = "me-acked" | "others-acked" | "unacked";

/**
 * Classify the ack state of a comment given its ack author list and the
 * current identity. An empty or undefined `me` is treated as "not in the
 * list", so comments still render sensibly before the identity resolves.
 */
export function ackStateFor(ack: readonly string[], me?: string | null): AckState {
  if (me && ack.includes(me)) return "me-acked";
  if (ack.length > 0) return "others-acked";
  return "unacked";
}

/**
 * The comment-card ack pill has two distinct render modes:
 *
 * - `"label"` → non-interactive `AckToggle` (a `<span>`).
 * - `"button"` → interactive `AckButton` (a real `<button>`).
 */
export type AckAffordanceKind = "label" | "button";

/**
 * What to show in the comment-card's kebab menu for the ack action.
 *
 * - `"ack"` → "Ack" item that calls `onAck(id, remove=false)`.
 * - `"unack"` → "Unack" item that calls `onAck(id, remove=true)`.
 * - `"none"` → no ack-related item; the main-card button (or nothing)
 *   owns the action.
 */
export type AckKebabKind = "ack" | "unack" | "none";

/**
 * Combined decision for a comment card's ack affordance, computed from
 * the current identity, the comment's author, and its ack roster.
 */
export interface AckAffordance {
  kind: AckAffordanceKind;
  kebab: AckKebabKind;
}

/**
 * Pick the ack affordance for a comment given the viewer and the
 * comment's author / ack list. Pure function — component tests cover
 * this directly instead of SSR-rendering the full `CommentCard` (which
 * pulls in the Obsidian runtime).
 *
 * Rules (rem-lcx + rem-pmun):
 *
 * 1. Viewer wrote the comment (`comment.author === me`):
 *    - pill is always a non-interactive label.
 *    - kebab carries a single mutually-exclusive action: `Unack` when
 *      the viewer is in the ack list, `Ack` otherwise. Acking your own
 *      comment from the main card clutters the UI, so it lives in the
 *      kebab for this rare case.
 * 2. Viewer has acked (and is not the author): label on card, `Unack`
 *    in kebab.
 * 3. Viewer has not acked: interactive button on card, no ack item in
 *    the kebab (the button IS the entry point).
 */
export function ackAffordanceFor(
  commentAuthor: string,
  ack: readonly string[],
  me: string | null | undefined
): AckAffordance {
  const viewerIsAuthor = !!me && commentAuthor === me;
  const viewerAcked = ackStateFor(ack, me) === "me-acked";

  if (viewerIsAuthor) {
    return {
      kind: "label",
      kebab: viewerAcked ? "unack" : "ack",
    };
  }
  if (viewerAcked) {
    return { kind: "label", kebab: "unack" };
  }
  return { kind: "button", kebab: "none" };
}
