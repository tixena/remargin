import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { CommentCard } from "@/components/sidebar/CommentCard";
import { findRadixScrollViewport } from "@/components/sidebar/scrollViewport";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { Comment } from "@/generated";
import { useBackend } from "@/hooks/useBackend";

interface ThreadedCommentsProps {
  file: string;
  onReply?: (commentId: string) => void;
  onGoToLine?: (line: number) => void;
  onMutation?: () => void;
  /**
   * Monotonic counter bumped by the shell when any sidebar section should
   * refetch. We observe it as a prop (rather than being keyed off it) so
   * the component refetches in place, preserving the scroll offset of the
   * outer sidebar viewport — see rem-8w5.
   */
  refreshKey?: number;
  /**
   * ID of the comment the user is replying to, if any. When set, the
   * `replyEditor` node is rendered as a peer row immediately after the
   * matching comment's card (same visual depth as a reply) instead of at
   * the top of the thread, so the composer stays next to the comment the
   * user is actually replying to.
   */
  replyTarget?: string | null;
  /**
   * The inline reply composer to render below the targeted comment. Owned
   * by the sidebar (which also owns `replyTarget`), passed down so the
   * thread can slot it in at the right place.
   */
  replyEditor?: React.ReactNode;
}

interface ThreadNode {
  comment: Comment;
  replies: ThreadNode[];
}

function buildThreadTree(comments: Comment[]): ThreadNode[] {
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

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

export function ThreadedComments({
  file,
  onReply,
  onGoToLine,
  onMutation,
  refreshKey,
  replyTarget,
  replyEditor,
}: ThreadedCommentsProps) {
  const backend = useBackend();
  const [comments, setComments] = useState<Comment[]>([]);
  // `loading` is true only until the very first fetch for a given file
  // resolves. Subsequent refetches (from refreshKey bumps, reactions,
  // acks, or reply submits) do NOT flip this back to true — that would
  // collapse the rendered list to a "Loading..." placeholder and the
  // outer sidebar viewport would snap to scrollTop=0 (rem-8w5).
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [me, setMe] = useState<string | null>(null);
  // Track the scrolling viewport so we can snapshot scrollTop before a
  // refetch replaces the comment list and restore it afterwards. The
  // actual scroller is the outer `SidebarShell` ScrollArea — we walk up
  // to find its Radix viewport element on first render.
  const rootRef = useRef<HTMLDivElement>(null);
  const scrollViewportRef = useRef<HTMLElement | null>(null);
  const pendingScrollTopRef = useRef<number | null>(null);

  const findScrollViewport = useCallback((): HTMLElement | null => {
    if (scrollViewportRef.current && document.contains(scrollViewportRef.current)) {
      return scrollViewportRef.current;
    }
    const found = findRadixScrollViewport(rootRef.current);
    if (found) scrollViewportRef.current = found;
    return found;
  }, []);

  const refresh = useCallback(async () => {
    // Snapshot the current scroll offset so `useLayoutEffect` can
    // restore it after the new comment list commits. Only matters for
    // in-place refetches — on first mount there's nothing to preserve.
    const viewport = findScrollViewport();
    if (viewport) {
      pendingScrollTopRef.current = viewport.scrollTop;
    }
    try {
      const result = await backend.comments(file);
      setComments(result);
      setError(null);
    } catch (err) {
      console.error("ThreadedComments.refresh failed:", err);
      setComments([]);
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [backend, file, findScrollViewport]);

  // Reset `loading` when the user switches to a different file — that's
  // the one case where we do want the placeholder, because the previous
  // file's list is no longer meaningful. `file` is a trigger-only dep.
  // biome-ignore lint/correctness/useExhaustiveDependencies: file is a trigger, not a read
  useEffect(() => {
    setLoading(true);
    setComments([]);
  }, [file]);

  // `refreshKey` is a trigger-only dep — bumping it must rerun refresh
  // even though we don't read its value inside the effect.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshKey is a trigger
  useEffect(() => {
    refresh();
  }, [refresh, refreshKey]);

  // Restore the viewport's scrollTop synchronously after a refetch
  // commits, so the user doesn't see the scroll jump. Pairs with the
  // snapshot taken at the top of `refresh()`. `comments` is a trigger-
  // only dep — its identity change is our "new data committed" signal.
  // biome-ignore lint/correctness/useExhaustiveDependencies: comments is a trigger
  useLayoutEffect(() => {
    const target = pendingScrollTopRef.current;
    if (target === null) return;
    pendingScrollTopRef.current = null;
    const viewport = findScrollViewport();
    if (viewport) {
      viewport.scrollTop = target;
    }
  }, [comments, findScrollViewport]);

  // Resolve the current identity once per mount so reaction pills can
  // distinguish "mine" from others' without threading it in from the shell.
  useEffect(() => {
    let cancelled = false;
    backend
      .identity()
      .then((info) => {
        if (!cancelled) setMe(info.identity ?? null);
      })
      .catch((err) => {
        console.error("ThreadedComments.identity failed:", err);
      });
    return () => {
      cancelled = true;
    };
  }, [backend]);

  const threads = useMemo(() => buildThreadTree(comments), [comments]);

  const handleAck = useCallback(
    async (id: string, remove: boolean) => {
      try {
        await backend.ack(file, [id], remove);
        // Stage the file in the user's sandbox so the interaction is
        // visible in the next Submit-to-Claude cycle.
        try {
          await backend.sandboxAdd([file]);
        } catch {
          // Best-effort: ack succeeded, don't fail the whole operation.
        }
        await refresh();
        onMutation?.();
      } catch (err) {
        console.error("ThreadedComments.ack failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh, onMutation]
  );

  const handleReact = useCallback(
    async (id: string, emoji: string, remove: boolean) => {
      try {
        await backend.react(file, id, emoji, remove);
        await refresh();
        onMutation?.();
      } catch (err) {
        console.error("ThreadedComments.react failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh, onMutation]
  );

  const handleDelete = useCallback(
    async (id: string) => {
      try {
        await backend.deleteComments(file, [id]);
        await refresh();
        onMutation?.();
      } catch (err) {
        console.error("ThreadedComments.delete failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh, onMutation]
  );

  if (loading) {
    return (
      <div ref={rootRef} className="px-4 py-3 text-xs text-text-faint">
        Loading...
      </div>
    );
  }

  if (error) {
    return (
      <div ref={rootRef} className="px-4 py-3 text-xs text-red-400 whitespace-pre-wrap break-words">
        <div className="font-semibold mb-1">Failed to load comments</div>
        <div className="font-mono text-[10px]">{error}</div>
      </div>
    );
  }

  if (threads.length === 0) {
    return (
      <div ref={rootRef} className="px-4 py-3 text-xs text-text-faint">
        No comments in this file.
      </div>
    );
  }

  return (
    <div ref={rootRef}>
      <ScrollArea className="flex-1">
        <div className="flex flex-col">
          {threads.map((node) => (
            <CommentThread
              key={node.comment.id}
              node={node}
              file={file}
              depth={0}
              me={me}
              parentAuthor={undefined}
              onAck={handleAck}
              onDelete={handleDelete}
              onReply={onReply}
              onReact={handleReact}
              onGoToLine={onGoToLine}
              replyTarget={replyTarget ?? null}
              replyEditor={replyEditor}
            />
          ))}
        </div>
      </ScrollArea>
    </div>
  );
}

interface CommentThreadProps {
  node: ThreadNode;
  file: string;
  depth: number;
  me: string | null;
  /** Author of this node's parent comment, for the implicit "to:" chip. */
  parentAuthor?: string;
  onAck: (id: string, remove: boolean) => void;
  onDelete: (id: string) => void;
  onReply?: (id: string) => void;
  onReact: (id: string, emoji: string, remove: boolean) => void;
  onGoToLine?: (line: number) => void;
  /**
   * ID of the comment whose card should have the inline reply editor
   * rendered directly beneath it (nested one level deeper, matching the
   * depth a real reply would render at). Compared against this node's id
   * during traversal — only one match fires.
   */
  replyTarget: string | null;
  replyEditor?: React.ReactNode;
}

function CommentThread({
  node,
  file,
  depth,
  me,
  parentAuthor,
  onAck,
  onDelete,
  onReply,
  onReact,
  onGoToLine,
  replyTarget,
  replyEditor,
}: CommentThreadProps) {
  const isReplyHere = replyTarget === node.comment.id && !!replyEditor;
  return (
    <div>
      <CommentCard
        comment={node.comment}
        file={file}
        depth={depth}
        isOnline={false}
        me={me}
        parentAuthor={parentAuthor}
        onAck={onAck}
        onDelete={onDelete}
        onReply={onReply}
        onReact={onReact}
        onGoToLine={onGoToLine}
      />
      {isReplyHere && <InlineReplySlot depth={depth + 1}>{replyEditor}</InlineReplySlot>}
      {node.replies.map((reply) => (
        <CommentThread
          key={reply.comment.id}
          node={reply}
          file={file}
          depth={depth + 1}
          me={me}
          parentAuthor={node.comment.author}
          onAck={onAck}
          onDelete={onDelete}
          onReply={onReply}
          onReact={onReact}
          onGoToLine={onGoToLine}
          replyTarget={replyTarget}
          replyEditor={replyEditor}
        />
      ))}
    </div>
  );
}

/**
 * Wrapper that scrolls the inline reply editor into view on mount so the
 * user does not lose it on a long thread. Depth controls the left inset
 * so the composer visually nests under the comment being replied to.
 */
function InlineReplySlot({ depth, children }: { depth: number; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, []);
  // Match CommentCard's depth-based left padding (10px base + 16px per
  // level) so the composer aligns with comment cards at the same depth.
  const style = { paddingLeft: `${10 + depth * 16}px` };
  return (
    <div ref={ref} style={style}>
      {children}
    </div>
  );
}
