/**
 * Visual variant for the Ack badge on a comment card. The arrow indicates
 * whether the comment has been acked by one of the people it was directed
 * at (double) vs. only by an outsider (single); the tone indicates whether
 * the ack should be painted green (progress) or the default muted color.
 */
export type AckArrow = "single" | "double";
export type AckTone = "green" | "normal";

export interface AckVisual {
  arrow: AckArrow;
  tone: AckTone;
}

/**
 * Compute the `(arrow, tone)` pair for an Ack badge. Rules (first match
 * wins, top-to-bottom):
 *
 * 1. Comment directed to **no one** (`toTargets` empty) and acked by at
 *    least one person → green double arrow. The "no-one" case is treated
 *    as "everyone", so any ack fully satisfies it.
 * 2. Comment directed to a specific recipient set (`toTargets` non-empty)
 *    and at least one of those recipients is in the ack roster → green
 *    double arrow. The directed-at recipient is the one the sender is
 *    waiting on, so their ack closes the loop.
 * 3. Comment directed to a recipient set and none of those recipients
 *    acked, but someone else did → green single arrow. Someone engaged,
 *    but not the one the sender was waiting on.
 * 4. Any other case (no acks at all) → normal color, single arrow.
 *
 * `toTargets` is the effective "to:" list the card renders — for replies
 * this should include the parent-author fallback when `comment.to` is
 * empty, so the visual stays consistent with what the user sees above
 * the body.
 */
export function ackVisualFor(
  toTargets: readonly string[],
  ack: readonly string[]
): AckVisual {
  if (ack.length === 0) return { arrow: "single", tone: "normal" };
  if (toTargets.length === 0) return { arrow: "double", tone: "green" };
  const hit = toTargets.some((target) => ack.includes(target));
  if (hit) return { arrow: "double", tone: "green" };
  return { arrow: "single", tone: "green" };
}
