/** The comment thread of the active file. */

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { CommentCard } from "@/components/sidebar/CommentCard";
import { KindFilterBar } from "@/components/sidebar/KindFilterBar";
import { readToken } from "@/components/sidebar/readToken";
import { findRadixScrollViewport } from "@/components/sidebar/scrollViewport";
import type { Comment } from "@/generated";
import { useBackend } from "@/hooks/useBackend";
import { collectKinds, matchesKindFilter, pruneKindFilter } from "@/lib/kindFilter";
import { buildThreadTree, type ThreadNode } from "@/lib/threadTree";
import { parseVerifyFailure, type VerifyFailure } from "@/lib/verifyFailure";

/** Props for {@link ThreadedComments}. */
interface ThreadedCommentsProps {
  file: string;
  onReply?: (commentId: string) => void;
  onGoToLine?: (line: number) => void;
  onMutation?: () => void;
  /** Observed as a prop, not a key, so a refetch happens in place and keeps the scroll offset. */
  refreshKey?: number;
  /** Id of the comment being replied to; the reply editor renders right after its card. */
  replyTarget?: string | null;
  replyEditor?: React.ReactNode;
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
  const [kindFilter, setKindFilter] = useState<string[]>([]);
  // True only until the first fetch for a file resolves: flipping it on a refetch would collapse
  // the list to the placeholder and snap the sidebar viewport to the top.
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [me, setMe] = useState<string | null>(null);
  // A fresh object per refetch, so the layout effect still fires when the offset repeats.
  const [scrollRestore, setScrollRestore] = useState<{ top: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const scrollViewportRef = useRef<HTMLElement | null>(null);

  const findScrollViewport = useCallback((): HTMLElement | null => {
    if (scrollViewportRef.current && document.contains(scrollViewportRef.current)) {
      return scrollViewportRef.current;
    }
    const found = findRadixScrollViewport(rootRef.current);
    if (found) scrollViewportRef.current = found;
    return found;
  }, []);

  // Token of the newest fetch: a slow response that a newer fetch superseded is dropped.
  const newestFetch = useRef<string | null>(null);
  // The file the rendered list belongs to; a mismatch means the user switched files.
  const renderedFile = useRef<string | null>(null);

  const refresh = useCallback(
    async (generation: number) => {
      const token = readToken(generation, file);
      newestFetch.current = token;
      if (renderedFile.current !== file) {
        renderedFile.current = file;
        // Switching files is the one case where the placeholder comes back.
        setLoading(true);
        setComments([]);
      }
      const snapshot = findScrollViewport()?.scrollTop ?? null;
      try {
        const result = await backend.comments(file);
        if (newestFetch.current !== token) return;
        setComments(result);
        setError(null);
      } catch (err) {
        console.error("ThreadedComments.refresh failed:", err);
        if (newestFetch.current !== token) return;
        setComments([]);
        setError(errorMessage(err));
      } finally {
        if (newestFetch.current === token) {
          setLoading(false);
          if (snapshot !== null) setScrollRestore({ top: snapshot });
        }
      }
    },
    [backend, file, findScrollViewport]
  );

  useEffect(() => {
    refresh(refreshKey ?? 0);
  }, [refresh, refreshKey]);

  // Synchronous, so the user never sees the scroll jump.
  useLayoutEffect(() => {
    if (scrollRestore === null) return;
    const viewport = findScrollViewport();
    if (viewport) {
      viewport.scrollTop = scrollRestore.top;
    }
  }, [scrollRestore, findScrollViewport]);

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

  const availableKinds = useMemo(() => collectKinds(comments), [comments]);

  useEffect(() => {
    setKindFilter((prev) => pruneKindFilter(prev, availableKinds));
  }, [availableKinds]);

  // The kind filter works per comment, not per thread, as the CLI does: a matching reply stays
  // visible without its parent, floated to root by `buildThreadTree`.
  const visibleComments = useMemo(() => {
    if (kindFilter.length === 0) return comments;
    return comments.filter((c) => matchesKindFilter(c.remargin_kind ?? [], kindFilter));
  }, [comments, kindFilter]);

  const threads = useMemo(() => buildThreadTree(visibleComments), [visibleComments]);

  const handleAck = useCallback(
    async (id: string, remove: boolean) => {
      try {
        await backend.ack(file, [id], remove);
        // Staged in the sandbox so the interaction is visible in the next Submit cycle.
        try {
          await backend.sandboxAdd([file]);
        } catch {
          // Best-effort: ack succeeded, don't fail the whole operation.
        }
        await refresh(refreshKey ?? 0);
        onMutation?.();
      } catch (err) {
        console.error("ThreadedComments.ack failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh, refreshKey, onMutation]
  );

  const handleReact = useCallback(
    async (id: string, emoji: string, remove: boolean) => {
      try {
        await backend.react(file, id, emoji, remove);
        await refresh(refreshKey ?? 0);
        onMutation?.();
      } catch (err) {
        console.error("ThreadedComments.react failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh, refreshKey, onMutation]
  );

  const handleDelete = useCallback(
    async (id: string) => {
      try {
        await backend.deleteComments(file, [id]);
        await refresh(refreshKey ?? 0);
        onMutation?.();
      } catch (err) {
        console.error("ThreadedComments.delete failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh, refreshKey, onMutation]
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
        <ErrorPanel raw={error} />
      </div>
    );
  }

  if (threads.length === 0) {
    const filtered = comments.length > 0 && kindFilter.length > 0;
    return (
      <div ref={rootRef}>
        {filtered && (
          <KindFilterBar
            availableKinds={availableKinds}
            selected={kindFilter}
            onChange={setKindFilter}
          />
        )}
        <div className="px-4 py-3 text-xs text-text-faint">
          {filtered ? "No comments match the selected kinds." : "No comments in this file."}
        </div>
      </div>
    );
  }

  return (
    <div ref={rootRef}>
      <KindFilterBar
        availableKinds={availableKinds}
        selected={kindFilter}
        onChange={setKindFilter}
      />
      <div className="flex flex-col">
        {threads.map((node) => (
          <CommentThread
            key={node.comment.id}
            node={node}
            file={file}
            depth={0}
            me={me}
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
    </div>
  );
}

/**
 * Render the comment-pane error state. When the raw stderr blob carries
 * the structured `verify_failed` shape, surface a plain-English headline
 * + actionable hint, with the per-failure breakdown tucked inside a
 * disclosure. Falls back to the raw text otherwise.
 */
function ErrorPanel({ raw }: { raw: string }) {
  const parsed: VerifyFailure | null = parseVerifyFailure(raw);
  if (!parsed) {
    return (
      <>
        <div className="font-semibold mb-1">Failed to load comments</div>
        <div className="font-mono text-[10px]">{raw}</div>
      </>
    );
  }
  return (
    <>
      <div className="font-semibold mb-1">{parsed.headline}</div>
      <div className="mb-2">{parsed.hint}</div>
      <details>
        <summary className="cursor-pointer">Show full details</summary>
        <ul className="font-mono text-[10px] mt-1">
          {parsed.failures.map((row) => (
            <li key={row.id}>
              {row.id}: checksum={row.checksum_ok ? "ok" : "FAIL"} signature={row.signature}
            </li>
          ))}
        </ul>
      </details>
    </>
  );
}

/** Props for {@link CommentThread}. */
interface CommentThreadProps {
  node: ThreadNode;
  file: string;
  depth: number;
  me: string | null;
  onAck: (id: string, remove: boolean) => void;
  onDelete: (id: string) => void;
  onReply?: (id: string) => void;
  onReact: (id: string, emoji: string, remove: boolean) => void;
  onGoToLine?: (line: number) => void;
  /** Id of the comment whose card gets the inline reply editor beneath it, one level deeper. */
  replyTarget: string | null;
  replyEditor?: React.ReactNode;
}

function CommentThread({
  node,
  file,
  depth,
  me,
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
  // CommentCard's depth padding: 10px base plus 16px per level.
  const style = { paddingLeft: `${10 + depth * 16}px` };
  return (
    <div ref={ref} style={style}>
      {children}
    </div>
  );
}
