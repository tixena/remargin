import type { Comment } from "@/generated/types";
import { type ThreadNode, walkThread } from "./threadTree";

/**
 * True when `comment` has `recipient` listed in `to:` AND `recipient`
 * has not acked it yet. Used to compute the "pending for me" badge and
 * the auto-expand decision.
 */
export function isPendingFor(comment: Comment, recipient: string): boolean {
  if (!comment.to.includes(recipient)) return false;
  return !comment.ack.some((a) => a.author === recipient);
}

/**
 * True when `comment` is a broadcast (no `to:` recipients) AND no one
 * has acked it yet. Broadcast pendings count as "pending for everyone"
 * for auto-expand purposes — the user spec says these auto-expand even
 * when there is no current identity.
 */
export function isPendingBroadcast(comment: Comment): boolean {
  return comment.to.length === 0 && comment.ack.length === 0;
}

/**
 * Aggregate pending stats for a thread subtree.
 *
 * `totalReplies` excludes the root itself (counts descendants). The
 * pending counts cover the entire subtree including the root, so a
 * root that is itself pending contributes to `pendingForMe` /
 * `pendingForOthers`.
 */
export interface PendingSummary {
  totalReplies: number;
  pendingForMe: number;
  pendingForOthers: number;
}

export function summarizeThread(root: ThreadNode, me: string | null): PendingSummary {
  let totalReplies = -1; // walkThread yields the root once; subtract it after the loop.
  let pendingForMe = 0;
  let pendingForOthers = 0;
  for (const c of walkThread(root)) {
    totalReplies += 1;
    const broadcast = isPendingBroadcast(c);
    const forMe = me !== null && isPendingFor(c, me);
    if (broadcast || forMe) {
      pendingForMe += 1;
    } else if (c.to.length > 0 && c.ack.length < c.to.length) {
      pendingForOthers += 1;
    }
  }
  if (totalReplies < 0) totalReplies = 0;
  return { totalReplies, pendingForMe, pendingForOthers };
}

/**
 * True when the root's subtree contains any comment that is pending for
 * the current identity OR a broadcast pending. The widget uses this to
 * seed the initial collapse state — pending threads land expanded so
 * the user sees the unanswered question without an extra click.
 */
export function shouldAutoExpand(root: ThreadNode, me: string | null): boolean {
  for (const c of walkThread(root)) {
    if (isPendingBroadcast(c)) return true;
    if (me !== null && isPendingFor(c, me)) return true;
  }
  return false;
}
