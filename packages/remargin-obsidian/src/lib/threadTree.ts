import type { Comment } from "@/generated/types";

/**
 * One node in a comment thread tree: the comment itself plus its direct
 * replies (each a thread node, recursively). Replies appear in the
 * order they showed up in the source list — `buildThreadTree` does
 * not re-sort.
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
 * widget still renders them).
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

  return roots;
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
