import { CommentHeader } from "@/components/sidebar/CommentHeader";
import { MarkdownContent } from "@/components/sidebar/MarkdownContent";
import type { Comment } from "@/generated/types";
import { CollapseToggle } from "./CollapseToggle";

export interface WidgetCommentViewProps {
  comment: Comment;
  /** Source path forwarded to MarkdownRenderer for relative link resolution. */
  sourcePath: string;
  collapsed: boolean;
  /** Flip the collapsed state — wired by the caller to a `CollapseState`. */
  onToggle: () => void;
  /**
   * Click on the widget body. Receives the comment id and the source path
   * so the parent can route the click to the sidebar focus receiver
   * (`plugin.focusComment`).
   */
  onClick: (commentId: string, file: string) => void;
}

/**
 * Read-only widget rendering of a single remargin comment, shared by
 * the reading-mode post-processor (T37) and the Live Preview CM6 widget
 * (T38). Reuses the sidebar's `CommentHeader` and `MarkdownContent`
 * primitives so the visual language is identical across surfaces.
 *
 * Editing is intentionally not surfaced here — every edit affordance
 * still lives in the sidebar. Clicking the widget body is the bridge:
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
}: WidgetCommentViewProps) {
  // Plain inline handler (no `useCallback`) keeps the component
  // hook-free, which lets unit tests call the function directly without
  // a React renderer to introspect prop wiring. Re-renders here are
  // already cheap — no children memoize on the click identity.
  const handleClick = () => {
    onClick(comment.id, sourcePath);
  };

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: widget click forwards to the sidebar; keyboard users open the sidebar directly via the existing command (T36 ships only the click bridge).
    // biome-ignore lint/a11y/noStaticElementInteractions: as above — widget root is a structural container, not an interactive control.
    <div className="remargin-widget-comment" onClick={handleClick}>
      <div className="remargin-widget-comment__header">
        <CollapseToggle collapsed={collapsed} onToggle={onToggle} />
        <CommentHeader comment={comment} />
      </div>
      {!collapsed && (
        <MarkdownContent
          content={comment.content ?? ""}
          sourcePath={sourcePath}
          className="remargin-widget-comment__body"
        />
      )}
    </div>
  );
}
