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
