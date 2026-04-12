import { Check, MoreHorizontal, Reply, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type { Comment } from "@/generated";
import { useBackend } from "@/hooks/useBackend";

interface ThreadedCommentsProps {
  file: string;
  onReply?: (commentId: string) => void;
  onGoToLine?: (line: number) => void;
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

export function ThreadedComments({ file, onReply, onGoToLine }: ThreadedCommentsProps) {
  const backend = useBackend();
  const [comments, setComments] = useState<Comment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

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

  const threads = useMemo(() => buildThreadTree(comments), [comments]);

  const handleAck = useCallback(
    async (id: string) => {
      try {
        await backend.ack(file, [id]);
        // Stage the file in the user's sandbox so the interaction is
        // visible in the next Submit-to-Claude cycle.
        try {
          await backend.sandboxAdd([file]);
        } catch {
          // Best-effort: ack succeeded, don't fail the whole operation.
        }
        await refresh();
      } catch (err) {
        console.error("ThreadedComments.ack failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh]
  );

  const handleDelete = useCallback(
    async (id: string) => {
      try {
        await backend.deleteComments(file, [id]);
        await refresh();
      } catch (err) {
        console.error("ThreadedComments.delete failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, file, refresh]
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
            depth={0}
            onAck={handleAck}
            onDelete={handleDelete}
            onReply={onReply}
            onGoToLine={onGoToLine}
          />
        ))}
      </div>
    </ScrollArea>
  );
}

interface CommentThreadProps {
  node: ThreadNode;
  depth: number;
  onAck: (id: string) => void;
  onDelete: (id: string) => void;
  onReply?: (id: string) => void;
  onGoToLine?: (line: number) => void;
}

function CommentThread({ node, depth, onAck, onDelete, onReply, onGoToLine }: CommentThreadProps) {
  const { comment } = node;
  const isPending = (comment.ack?.length ?? 0) === 0;

  const isClickable = comment.line > 0 && !!onGoToLine;

  return (
    <div>
      <div
        className={`flex flex-col gap-1 px-4 py-2 border-b border-bg-border hover:bg-bg-hover ${
          depth > 0 ? "border-l-2 border-l-accent" : ""
        } ${isClickable ? "cursor-pointer" : ""}`}
        style={{ paddingLeft: `${16 + depth * 16}px` }}
        onClick={() => {
          if (isClickable) {
            onGoToLine?.(comment.line);
          }
        }}
      >
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 min-w-0">
            <Badge
              className={`px-1 py-0 text-[9px] font-semibold ${
                comment.author_type === "agent"
                  ? "bg-purple-400 text-white"
                  : "bg-blue-400 text-white"
              }`}
            >
              {comment.author_type === "agent" ? "AI" : "H"}
            </Badge>
            {comment.id && (
              <Badge className="px-1 py-0 text-[9px] font-mono font-semibold bg-slate-500 text-white">
                {comment.id}
              </Badge>
            )}
            {comment.line > 0 && (
              <span className="text-[9px] text-text-faint font-mono">L{comment.line}</span>
            )}
            <span className="text-xs font-medium text-text-normal truncate">{comment.author}</span>
            {isPending && <span className="w-1.5 h-1.5 rounded-full bg-amber-400" />}
          </div>
          <div className="flex items-center gap-1">
            <TooltipProvider>
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="text-[10px] text-text-faint whitespace-nowrap">
                    {formatRelativeTime(comment.ts)}
                  </span>
                </TooltipTrigger>
                <TooltipContent>
                  <p className="text-xs">{formatFullTime(comment.ts)}</p>
                </TooltipContent>
              </Tooltip>
            </TooltipProvider>
          </div>
        </div>

        {comment.to && comment.to.length > 0 && (
          <div className="flex items-center gap-1 text-[10px] text-text-faint">
            <span>to:</span>
            {comment.to.map((r) => (
              <Badge key={r} variant="outline" className="px-1 py-0 text-[9px]">
                {r}
              </Badge>
            ))}
          </div>
        )}

        <p className="text-xs text-text-muted whitespace-pre-wrap break-words">{comment.content}</p>

        {comment.reactions && Object.keys(comment.reactions).length > 0 && (
          <div className="flex items-center gap-1 flex-wrap">
            {Object.entries(comment.reactions).map(([emoji, authors]) => (
              <Badge key={emoji} variant="outline" className="px-1.5 py-0 text-[10px] gap-0.5">
                {emoji} {authors?.length ?? 0}
              </Badge>
            ))}
          </div>
        )}

        <div className="flex items-center gap-1 -ml-1">
          {isPending && (
            <Button
              variant="ghost"
              size="sm"
              className="h-5 px-1.5 text-[10px] text-green-500 hover:text-green-400"
              onClick={(e) => {
                e.stopPropagation();
                if (comment.id) {
                  onGoToLine?.(comment.line);
                  onAck(comment.id);
                }
              }}
            >
              <Check className="w-3 h-3 mr-0.5" />
              Ack
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="h-5 px-1.5 text-[10px] text-text-faint hover:text-text-muted"
            onClick={(e) => {
              e.stopPropagation();
              if (comment.id) onReply?.(comment.id);
            }}
          >
            <Reply className="w-3 h-3 mr-0.5" />
            Reply
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="sm"
                className="h-5 w-5 p-0 text-text-faint hover:text-text-muted"
                onClick={(e) => e.stopPropagation()}
              >
                <MoreHorizontal className="w-3 h-3" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem
                className="text-red-400"
                onClick={(e) => {
                  e.stopPropagation();
                  if (comment.id) onDelete(comment.id);
                }}
              >
                <Trash2 className="w-3 h-3 mr-1.5" />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      {node.replies.map((reply) => (
        <CommentThread
          key={reply.comment.id}
          node={reply}
          depth={depth + 1}
          onAck={onAck}
          onDelete={onDelete}
          onReply={onReply}
          onGoToLine={onGoToLine}
        />
      ))}
    </div>
  );
}

function formatFullTime(ts?: string): string {
  if (!ts) return "";
  try {
    return new Date(ts).toLocaleString(undefined, {
      year: "numeric",
      month: "long",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  } catch {
    return ts;
  }
}

function formatRelativeTime(ts?: string): string {
  if (!ts) return "";
  try {
    const diff = Date.now() - new Date(ts).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "now";
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.floor(hours / 24);
    return `${days}d`;
  } catch {
    return "";
  }
}
