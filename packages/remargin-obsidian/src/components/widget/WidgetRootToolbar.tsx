import type { MouseEvent } from "react";
import { Badge } from "@/components/ui/badge";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import type { Comment } from "@/generated/types";
import type { PendingSummary } from "@/lib/pendingState";

export interface WidgetRootToolbarProps {
  /** The thread's root comment. Drives the identity badge + id badge. */
  comment: Comment;
  /** Reply + pending counts for the entire subtree. */
  summary: PendingSummary;
  /** Bulk-expand the root + every descendant. */
  onExpandAll: () => void;
  /** Bulk-collapse the root + every descendant (hard reset). */
  onCollapseAll: () => void;
}

/**
 * Toolbar row rendered above a root comment widget. Shows the thread's
 * identity badge, root id, reply / pending counts, and the per-thread
 * bulk expand/collapse icons. Renders only for ROOTS (the parent
 * `WidgetCommentThread` decides via its `isRoot` prop); nested replies
 * never get one.
 *
 * Uses `e.stopPropagation()` on every button so clicks don't bubble to
 * the surrounding `WidgetCommentView`'s outer onClick (which would
 * otherwise route the click to `plugin.focusComment` as a sidebar
 * focus request).
 */
export function WidgetRootToolbar({
  comment,
  summary,
  onExpandAll,
  onCollapseAll,
}: WidgetRootToolbarProps) {
  const isAgent = comment.author_type === "agent";
  const initials = isAgent ? "AI" : "H";
  const avatarClass = isAgent ? "bg-purple-400 text-white" : "bg-blue-400 text-white";

  const handle = (cb: () => void) => (e: MouseEvent) => {
    e.stopPropagation();
    cb();
  };

  const replyNoun = summary.totalReplies === 1 ? "reply" : "replies";

  return (
    <div className="remargin-widget-root-toolbar">
      <Badge
        className={`px-1 py-0 rounded-full font-mono text-[10px] font-semibold leading-none ${avatarClass}`}
        aria-label={isAgent ? "AI agent thread" : "Human thread"}
      >
        {initials}
      </Badge>
      {comment.id && (
        <Badge className="px-1 py-0 rounded-sm bg-slate-500 text-white font-mono text-[10px] font-semibold leading-none">
          {comment.id}
        </Badge>
      )}
      {summary.totalReplies > 0 && (
        <span className="font-mono text-[10px] font-normal leading-none text-[var(--text-muted)]">
          {summary.totalReplies} {replyNoun}
        </span>
      )}
      {summary.pendingForMe > 0 && (
        <Badge className="px-1.5 py-0 rounded-sm bg-amber-500 text-white font-mono text-[10px] font-semibold leading-none">
          {summary.pendingForMe} pending
        </Badge>
      )}
      <div className="remargin-widget-root-toolbar__actions">
        <button
          type="button"
          aria-label="Expand all replies in this thread"
          title="Expand all replies in this thread"
          className="remargin-widget-root-toolbar__btn"
          onClick={handle(onExpandAll)}
        >
          <ObsidianIcon icon="chevrons-down" />
        </button>
        <button
          type="button"
          aria-label="Collapse all replies in this thread"
          title="Collapse all replies in this thread"
          className="remargin-widget-root-toolbar__btn"
          onClick={handle(onCollapseAll)}
        >
          <ObsidianIcon icon="chevrons-up" />
        </button>
      </div>
    </div>
  );
}
