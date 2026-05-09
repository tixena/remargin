import { useEffect, useMemo, useState } from "react";
import { shouldAutoExpand, summarizeThread } from "@/lib/pendingState";
import type { ThreadNode } from "@/lib/threadTree";
import type { CollapseState } from "@/state/collapseState";
import { WidgetCommentView } from "./WidgetCommentView";

export interface WidgetCommentThreadProps {
  root: ThreadNode;
  sourcePath: string;
  /**
   * Resolved current identity (`backend.identity().identity`) or null
   * when no identity is set. Drives the auto-expand / pending-for-me
   * counts. Threading it through the widget is what wires identity
   * into the widget render path (was previously identity-blind).
   */
  me: string | null;
  collapseState: CollapseState;
  onClick: (commentId: string, file: string) => void;
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
}: WidgetCommentThreadProps) {
  const id = root.comment.id;

  // Auto-expand priming MUST run before the first paint that consults
  // `collapsed` below — using a layout effect keeps the chevron and
  // body in sync with the seeded state on initial mount. Plain
  // `useEffect` would render once collapsed (the default), then flip.
  useEffect(() => {
    if (!collapseState.has(id) && shouldAutoExpand(root, me)) {
      collapseState.setExpanded(id);
    }
    // The store mutation will fire a notification that re-renders us
    // through the subscription effect below, so we don't need to
    // re-bind dependencies here.
  }, [id, root, me, collapseState]);

  // Subscribe to the shared CollapseState so chevron toggles in any
  // surface (this widget, the sibling reading-mode widget, the
  // thread-level toolbar) re-render this subtree.
  const [, force] = useState(0);
  useEffect(() => {
    return collapseState.subscribe(() => {
      force((n) => n + 1);
    });
  }, [collapseState]);

  const collapsed = collapseState.isCollapsed(id);
  const summary = useMemo(() => summarizeThread(root, me), [root, me]);

  return (
    <div className="remargin-widget-thread">
      <WidgetCommentView
        comment={root.comment}
        sourcePath={sourcePath}
        collapsed={collapsed}
        onToggle={() => collapseState.toggle(id)}
        onClick={onClick}
        summary={summary}
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
