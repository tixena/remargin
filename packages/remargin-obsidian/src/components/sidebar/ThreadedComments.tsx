import { useCallback, useEffect, useMemo, useState } from "react";
import { CommentCard } from "@/components/sidebar/CommentCard";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { Comment } from "@/generated";
import { useBackend } from "@/hooks/useBackend";

interface ThreadedCommentsProps {
  file: string;
  onReply?: (commentId: string) => void;
  onGoToLine?: (line: number) => void;
  onMutation?: () => void;
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

export function ThreadedComments({ file, onReply, onGoToLine, onMutation }: ThreadedCommentsProps) {
  const backend = useBackend();
  const [comments, setComments] = useState<Comment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [me, setMe] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
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
  }, [backend, file]);

  useEffect(() => {
    refresh();
  }, [refresh]);

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
    return <div className="px-4 py-3 text-xs text-text-faint">Loading...</div>;
  }

  if (error) {
    return (
      <div className="px-4 py-3 text-xs text-red-400 whitespace-pre-wrap break-words">
        <div className="font-semibold mb-1">Failed to load comments</div>
        <div className="font-mono text-[10px]">{error}</div>
      </div>
    );
  }

  if (threads.length === 0) {
    return <div className="px-4 py-3 text-xs text-text-faint">No comments in this file.</div>;
  }

  return (
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
          />
        ))}
      </div>
    </ScrollArea>
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
}: CommentThreadProps) {
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
        />
      ))}
    </div>
  );
}
