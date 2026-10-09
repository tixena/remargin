/** The editor-side widget for one comment thread. */

import { useEffect, useMemo, useState } from "react";
import { shouldAutoExpand, summarizeThread } from "@/lib/pendingState";
import type { ThreadNode } from "@/lib/threadTree";
import { walkThread } from "@/lib/threadTree";
import type { CollapseState } from "@/state/collapseState";
import { WidgetCommentView } from "./WidgetCommentView";
import { WidgetRootToolbar } from "./WidgetRootToolbar";

/** Props for {@link WidgetCommentThread}. */
export interface WidgetCommentThreadProps {
  root: ThreadNode;
  sourcePath: string;
  /** The resolved identity, or null; drives the auto-expand and pending-for-me counts. */
  me: string | null;
  collapseState: CollapseState;
  onClick: (commentId: string, file: string) => void;
  /** True only at the thread's top-level mount; nested reply rows never render the toolbar. */
  isRoot?: boolean;
}

/**
 * Render a remargin widget thread tree: the root comment, then any
 * replies nested with a 16px-per-level left indent. Each comment has
 * its own collapse state — collapsing the root hides ALL replies.
 *
 * Auto-expand priming: on mount, if the root has never been touched
 * AND its subtree contains a pending comment (broadcast OR for me),
 * seed the collapse store as expanded. Once the user explicitly
 * toggles the chevron, `CollapseState.has` returns true on subsequent
 * mounts so the user's choice wins.
 */
export function WidgetCommentThread({
  root,
  sourcePath,
  me,
  collapseState,
  onClick,
  isRoot = false,
}: WidgetCommentThreadProps) {
  const id = root.comment.id;

  // A layout effect: plain `useEffect` would paint once collapsed (the default), then flip.
  useEffect(() => {
    if (!collapseState.has(id) && shouldAutoExpand(root, me)) {
      collapseState.setExpanded(id);
    }
  }, [id, root, me, collapseState]);

  // Chevron toggles from any surface sharing the CollapseState re-render this subtree.
  const [, force] = useState(0);
  useEffect(() => {
    return collapseState.subscribe(() => {
      force((n) => n + 1);
    });
  }, [collapseState]);

  const collapsed = collapseState.isCollapsed(id);
  const summary = useMemo(() => summarizeThread(root, me), [root, me]);

  // "Collapse all" is a hard reset: it marks every descendant as touched, so auto-expand priming
  // never re-flips them. Roots drop the card-side summary because the toolbar shows those counts.
  return (
    <div className="remargin-widget-thread">
      {isRoot && (
        <WidgetRootToolbar
          comment={root.comment}
          summary={summary}
          onExpandAll={() => setSubtreeCollapsed(root, collapseState, false)}
          onCollapseAll={() => setSubtreeCollapsed(root, collapseState, true)}
        />
      )}
      <WidgetCommentView
        comment={root.comment}
        sourcePath={sourcePath}
        collapsed={collapsed}
        onToggle={() => collapseState.toggle(id)}
        onClick={onClick}
        summary={isRoot ? undefined : summary}
      />
      {!collapsed && root.replies.length > 0 && (
        <div className="remargin-widget-thread__replies" style={{ paddingLeft: 16 }}>
          {root.replies.map((reply) => (
            <WidgetCommentThread
              key={reply.comment.id}
              root={reply}
              sourcePath={sourcePath}
              me={me}
              collapseState={collapseState}
              onClick={onClick}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** Bulk-set the collapsed flag for `root` and every descendant. */
export function setSubtreeCollapsed(
  root: ThreadNode,
  collapseState: CollapseState,
  collapsed: boolean
): void {
  const ids = Array.from(walkThread(root)).map((c) => c.id);
  collapseState.setMany(ids, collapsed);
}
