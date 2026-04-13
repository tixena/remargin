import { Check, MoreHorizontal, Reply, Trash2 } from "lucide-react";
import { CommentHeader } from "@/components/sidebar/CommentHeader";
import { MarkdownContent } from "@/components/sidebar/MarkdownContent";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Comment } from "@/generated";

interface CommentCardProps {
  comment: Comment;
  file: string;
  depth: number;
  isOnline?: boolean;
  onAck: (id: string) => void;
  onDelete: (id: string) => void;
  onReply?: (id: string) => void;
  onGoToLine?: (line: number) => void;
}

/**
 * A single comment in the thread list. Owns the visual layout defined in
 * UI task 20: rich header, body, optional reply targets / reactions, and a
 * split action row (Ack + reactions on the left, Reply + More on the right).
 *
 * Ack, reactions, and `to:` chips live here only as placeholders until
 * tasks 21/22/23 replace them with their dedicated components.
 */
export function CommentCard({
  comment,
  file,
  depth,
  isOnline,
  onAck,
  onDelete,
  onReply,
  onGoToLine,
}: CommentCardProps) {
  const isPending = (comment.ack?.length ?? 0) === 0;
  const isClickable = comment.line > 0 && !!onGoToLine;

  return (
    <div
      className={`flex flex-col gap-[5px] px-2.5 py-2 border-b border-bg-border hover:bg-bg-hover ${
        depth > 0 ? "border-l-2 border-l-accent" : ""
      } ${isClickable ? "cursor-pointer" : ""}`}
      style={{ paddingLeft: `${10 + depth * 16}px` }}
      onClick={() => {
        if (isClickable) {
          onGoToLine?.(comment.line);
        }
      }}
    >
      <CommentHeader comment={comment} isOnline={isOnline} />

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

      <div className="text-sm text-text-normal leading-[1.4]">
        <MarkdownContent content={comment.content} sourcePath={file} />
      </div>

      {comment.reactions && Object.keys(comment.reactions).length > 0 && (
        <div className="flex items-center gap-1 flex-wrap">
          {Object.entries(comment.reactions).map(([emoji, authors]) => (
            <Badge key={emoji} variant="outline" className="px-1.5 py-0 text-[10px] gap-0.5">
              {emoji} {authors?.length ?? 0}
            </Badge>
          ))}
        </div>
      )}

      <div className="flex items-center justify-between gap-2 w-full">
        <div className="flex items-center gap-1.5">
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
        </div>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            className="h-5 px-1.5 text-[10px] text-text-faint hover:text-text-muted gap-[3px]"
            onClick={(e) => {
              e.stopPropagation();
              if (comment.id) onReply?.(comment.id);
            }}
          >
            <Reply className="w-3 h-3" />
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
    </div>
  );
}
