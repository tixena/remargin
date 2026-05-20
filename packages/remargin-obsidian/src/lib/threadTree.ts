import type { Comment } from "@/generated/types";

/**
 * One node in a comment thread tree: the comment itself plus its direct
 * replies, sorted oldest-first by `ts`. Roots stay in source order.
 */
export interface ThreadNode {
  comment: Comment;
  replies: ThreadNode[];
}

/**
 * Group a flat comment list into a forest of thread trees by `reply_to`.
 *
 * Roots are comments whose `reply_to` is unset OR points to an id that
 * is not in `comments` (orphan replies — they float up to root so the
 * widget still renders them). Roots stay in source order; replies are
 * sorted oldest-first by `ts` so a reader sees the conversation in the
 * order it happened. Stable on equal `ts` — first occurrence wins.
 */
export function buildThreadTree(comments: Comment[]): ThreadNode[] {
  const byId = new Map<string, ThreadNode>();
  const roots: ThreadNode[] = [];

  for (const c of comments) {
    byId.set(c.id, { comment: c, replies: [] });
  }

  for (const c of comments) {
    const node = byId.get(c.id);
    if (!node) continue;
    const parent = c.reply_to ? byId.get(c.reply_to) : undefined;
    if (parent) {
      parent.replies.push(node);
    } else {
      roots.push(node);
    }
  }

  sortRepliesAsc(roots);
  return roots;
}

function sortRepliesAsc(nodes: ThreadNode[]): void {
  for (const node of nodes) {
    node.replies.sort((a, b) => (a.comment.ts ?? "").localeCompare(b.comment.ts ?? ""));
    sortRepliesAsc(node.replies);
  }
}

/**
 * Index a flat comment list by id for O(1) parent / lookup.
 */
export function indexById(comments: Comment[]): Map<string, Comment> {
  const byId = new Map<string, Comment>();
  for (const c of comments) {
    byId.set(c.id, c);
  }
  return byId;
}

/**
 * Walk a thread node and yield every comment in its subtree, root first
 * then its replies depth-first (each reply followed by its own
 * descendants before the next sibling).
 */
export function* walkThread(node: ThreadNode): Generator<Comment> {
  yield node.comment;
  for (const reply of node.replies) {
    yield* walkThread(reply);
  }
}
