/** One comment in the thread list. */

import { AckButton } from "@/components/sidebar/AckButton";
import { AckToggle } from "@/components/sidebar/AckToggle";
import { CommentHeader } from "@/components/sidebar/CommentHeader";
import { EditedLabel } from "@/components/sidebar/EditedLabel";
import { EmojiPicker } from "@/components/sidebar/EmojiPicker";
import { KindChips } from "@/components/sidebar/KindChips";
import { MarkdownContent } from "@/components/sidebar/MarkdownContent";
import { ReactionPills } from "@/components/sidebar/ReactionPills";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import type { Comment } from "@/generated";
import { useParticipants } from "@/hooks/useParticipants";
import { ackAffordanceFor } from "@/lib/ack-state";
import { authorLabel } from "@/lib/authorLabel";
import { activationKeyHandler } from "@/lib/keyboardActivation";

/** Props for {@link CommentCard}. */
interface CommentCardProps {
  comment: Comment;
  file: string;
  depth: number;
  isOnline?: boolean;
  me?: string | null;
  /** Toggles the viewer's ack; `remove` is true when it comes from the Unack menu item. */
  onAck: (id: string, remove: boolean) => void;
  onDelete: (id: string) => void;
  onReply?: (id: string) => void;
  /** `remove` is true when the click was on a pill the current identity already reacted to. */
  onReact?: (id: string, emoji: string, remove: boolean) => void;
  onGoToLine?: (line: number) => void;
}

/**
 * A single comment in the thread list: rich header, body, optional reply targets / reactions, and
 * a split action row (Ack + reactions on the left, Reply + More on the right).
 */
export function CommentCard({
  comment,
  file,
  depth,
  isOnline,
  me,
  onAck,
  onDelete,
  onReply,
  onReact,
  onGoToLine,
}: CommentCardProps) {
  const isClickable = comment.line > 0 && !!onGoToLine;
  const ackAuthors: string[] = (comment.ack ?? []).map((a) => a.author);
  const { resolveDisplayName } = useParticipants();
  const affordance = ackAffordanceFor(comment.author, ackAuthors, me);
  const toTargets: readonly string[] = comment.to ?? [];

  return (
    <div
      // `data-comment-id` lets the editor-side focus bridge find the card to scroll to and highlight.
      // An empty id leaves the attribute off so the selector cannot match unrelated cards.
      data-comment-id={comment.id || undefined}
      className={`flex flex-col gap-[5px] px-2.5 py-2 border-b border-bg-border hover:bg-bg-hover remargin-comment-card ${
        depth > 0 ? "border-l-2 border-l-accent" : ""
      } ${isClickable ? "cursor-pointer" : ""}`}
      style={{ paddingLeft: `${10 + depth * 16}px` }}
      role={isClickable ? "button" : undefined}
      tabIndex={isClickable ? 0 : undefined}
      onClick={() => {
        if (isClickable) {
          onGoToLine?.(comment.line);
        }
      }}
      onKeyDown={activationKeyHandler(() => {
        if (isClickable) {
          onGoToLine?.(comment.line);
        }
      })}
    >
      {toTargets.length > 0 && (
        <div className="flex items-center gap-1 flex-wrap">
          {toTargets.map((identity) => {
            const { label, title } = authorLabel(identity, resolveDisplayName);
            return (
              <span
                key={identity}
                className="inline-flex items-center gap-1 rounded-[3px] bg-bg-hover px-1.5 py-0.5 font-mono text-[9px] leading-none"
                title={title}
              >
                <span className="text-text-faint">to:</span>
                <span className="text-accent font-semibold">{label}</span>
              </span>
            );
          })}
        </div>
      )}

      <CommentHeader comment={comment} isOnline={isOnline} />

      <div className="text-sm text-text-normal leading-[1.4]">
        <MarkdownContent content={comment.content} sourcePath={file} />
      </div>

      <div className="flex items-center justify-between gap-2 w-full">
        <div className="flex items-center gap-1.5 flex-wrap">
          {comment.id &&
            (affordance.kind === "label" ? (
              <AckToggle ack={ackAuthors} me={me} toTargets={toTargets} />
            ) : (
              <AckButton
                ack={ackAuthors}
                me={me}
                toTargets={toTargets}
                onAck={() => {
                  if (comment.id) onAck(comment.id, false);
                }}
              />
            ))}
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
                  const already =
                    !!me && (comment.reactions?.[emoji]?.some((e) => e.author === me) ?? false);
                  onReact(comment.id, emoji, already);
                }
              }}
            />
          )}
          {comment.edited_at && <EditedLabel editedAt={comment.edited_at} />}
          <KindChips kinds={comment.remargin_kind} />
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
            <ObsidianIcon icon="reply" size={12} />
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
                <ObsidianIcon icon="more-horizontal" size={12} />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem
                onClick={(e) => {
                  e.stopPropagation();
                  void navigator.clipboard.writeText(comment.content).catch((err) => {
                    console.error("Copy contents failed:", err);
                  });
                }}
              >
                <ObsidianIcon icon="copy" size={12} className="mr-1.5" />
                Copy contents
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {affordance.kebab === "unack" && (
                <>
                  <DropdownMenuItem
                    onClick={(e) => {
                      e.stopPropagation();
                      if (comment.id) onAck(comment.id, true);
                    }}
                  >
                    <ObsidianIcon icon="check" size={12} className="mr-1.5" />
                    Unack
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                </>
              )}
              {affordance.kebab === "ack" && (
                <>
                  <DropdownMenuItem
                    onClick={(e) => {
                      e.stopPropagation();
                      if (comment.id) onAck(comment.id, false);
                    }}
                  >
                    <ObsidianIcon icon="check" size={12} className="mr-1.5" />
                    Ack
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                </>
              )}
              <DropdownMenuItem
                className="text-red-400"
                onClick={(e) => {
                  e.stopPropagation();
                  if (comment.id) onDelete(comment.id);
                }}
              >
                <ObsidianIcon icon="trash-2" size={12} className="mr-1.5" />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    </div>
  );
}
