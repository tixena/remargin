import { MoreHorizontal, Reply, Trash2 } from "lucide-react";
import { AckToggle } from "@/components/sidebar/AckToggle";
import { CommentHeader } from "@/components/sidebar/CommentHeader";
import { EmojiPicker } from "@/components/sidebar/EmojiPicker";
import { MarkdownContent } from "@/components/sidebar/MarkdownContent";
import { ReactionPills } from "@/components/sidebar/ReactionPills";
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
  /** Current identity name; used by ReactionPills to mark "mine" pills. */
  me?: string | null;
  /**
   * Author of the comment this one replies to. Used as a fallback target
   * for the "to:" chip when `comment.to` is empty — a reply without an
   * explicit `to` field is implicitly addressed to the parent's author.
   */
  parentAuthor?: string;
  /**
   * Toggle the current identity's ack on this comment. `remove` is true
   * when the click should strip the identity's existing ack; false adds
   * a new one.
   */
  onAck: (id: string, remove: boolean) => void;
  onDelete: (id: string) => void;
  onReply?: (id: string) => void;
  /**
   * Called when the user wants to add or remove a reaction. `remove` is
   * true when the click was on a pill the current identity already reacted
   * to.
   */
  onReact?: (id: string, emoji: string, remove: boolean) => void;
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
  me,
  parentAuthor,
  onAck,
  onDelete,
  onReply,
  onReact,
  onGoToLine,
}: CommentCardProps) {
  const isClickable = comment.line > 0 && !!onGoToLine;
  const ackAuthors: string[] = (comment.ack ?? []).map((a) => a.author);
  // Resolve the "to:" chip targets. Prefer the explicit `to` field; fall
  // back to the parent comment's author for replies that did not set `to`.
  // Root comments with neither stay bare (no chip).
  const toTargets: string[] =
    comment.to && comment.to.length > 0
      ? comment.to
      : comment.reply_to && parentAuthor
        ? [parentAuthor]
        : [];

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
      {toTargets.length > 0 && (
        <div className="flex items-center gap-1 flex-wrap">
          {toTargets.map((identity) => (
            <span
              key={identity}
              className="inline-flex items-center gap-1 rounded-[3px] bg-bg-hover px-1.5 py-0.5 font-mono text-[9px] leading-none"
            >
              <span className="text-text-faint">to:</span>
              <span className="text-accent font-semibold">{identity}</span>
            </span>
          ))}
        </div>
      )}

      <CommentHeader comment={comment} isOnline={isOnline} />

      <div className="text-sm text-text-normal leading-[1.4]">
        <MarkdownContent content={comment.content} sourcePath={file} />
      </div>

      <div className="flex items-center justify-between gap-2 w-full">
        <div className="flex items-center gap-1.5 flex-wrap">
          {comment.id && (
            <AckToggle
              ack={ackAuthors}
              me={me}
              onToggle={(remove) => {
                if (!remove) onGoToLine?.(comment.line);
                if (comment.id) onAck(comment.id, remove);
              }}
            />
          )}
          {comment.reactions && (
            <ReactionPills
              reactions={comment.reactions}
              me={me}
              onToggle={(emoji, mine) => {
                if (comment.id) onReact?.(comment.id, emoji, mine);
              }}
            />
          )}
          {onReact && comment.id && (
            <EmojiPicker
              onPick={(emoji) => {
                if (comment.id) {
                  const already = !!me && (comment.reactions?.[emoji]?.includes(me) ?? false);
                  onReact(comment.id, emoji, already);
                }
              }}
            />
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
