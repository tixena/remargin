/** The read-only widget for a single comment. */

import { CommentHeader } from "@/components/sidebar/CommentHeader";
import { EditedLabel } from "@/components/sidebar/EditedLabel";
import { KindChips } from "@/components/sidebar/KindChips";
import { MarkdownContent } from "@/components/sidebar/MarkdownContent";
import type { Comment } from "@/generated/types";
import { activationKeyHandler } from "@/lib/keyboardActivation";
import type { PendingSummary } from "@/lib/pendingState";
import { CollapseToggle } from "./CollapseToggle";

/** Props for {@link WidgetCommentView}. */
export interface WidgetCommentViewProps {
  comment: Comment;
  sourcePath: string;
  collapsed: boolean;
  onToggle: () => void;
  onClick: (commentId: string, file: string) => void;
  /** Rendered only when the comment is collapsed and `summary.totalReplies > 0`. */
  summary?: PendingSummary;
}

/**
 * Read-only widget rendering of a single remargin comment, shared by
 * the reading-mode post-processor and the Live Preview CM6 widget.
 * Reuses the sidebar's `CommentHeader` and `MarkdownContent`
 * primitives so the visual language is identical across surfaces.
 *
 * Editing is intentionally not surfaced here — every edit affordance
 * lives in the sidebar. Clicking the widget body is the bridge:
 * `onClick(commentId, file)` lets the parent dispatch the focus-receiver
 * call so the sidebar scrolls and highlights the corresponding card.
 *
 * Collapse state is passed in (NOT held locally) so the same comment can
 * mirror its collapsed/expanded state across reading mode and Live
 * Preview without needing a re-render bridge.
 */
export function WidgetCommentView({
  comment,
  sourcePath,
  collapsed,
  onToggle,
  onClick,
  summary,
}: WidgetCommentViewProps) {
  // No `useCallback`: the component stays hook-free so unit tests can call it directly.
  const handleClick = () => {
    onClick(comment.id, sourcePath);
  };

  const showSummary = collapsed && summary !== undefined && summary.totalReplies > 0;
  const hasTags = !!comment.edited_at || (comment.remargin_kind ?? []).length > 0;

  return (
    // The whole card is the click and keyboard target for the sidebar-focus bridge.
    <div
      className="remargin-widget-comment"
      role="button"
      tabIndex={0}
      onClick={handleClick}
      onKeyDown={activationKeyHandler(handleClick)}
    >
      <div className="remargin-widget-comment__header">
        <CollapseToggle collapsed={collapsed} onToggle={onToggle} />
        <CommentHeader comment={comment} />
      </div>
      {showSummary && summary !== undefined && (
        <span className="remargin-widget-comment__summary">{formatSummary(summary)}</span>
      )}
      {!collapsed && (
        <MarkdownContent
          content={comment.content ?? ""}
          sourcePath={sourcePath}
          className="remargin-widget-comment__body"
        />
      )}
      {!collapsed && hasTags && (
        <div className="remargin-widget-comment__tags">
          {comment.edited_at && <EditedLabel editedAt={comment.edited_at} />}
          <KindChips kinds={comment.remargin_kind} />
        </div>
      )}
    </div>
  );
}

function formatSummary(summary: PendingSummary): string {
  const noun = summary.totalReplies === 1 ? "reply" : "replies";
  const base = `${summary.totalReplies} ${noun}`;
  if (summary.pendingForMe > 0) {
    return `${base} · ${summary.pendingForMe} pending for you`;
  }
  return base;
}
