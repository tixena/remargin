import { createElement, useEffect, useMemo, useState } from "react";
import { shouldAutoExpand, summarizeThread } from "@/lib/pendingState";
import type { ThreadNode } from "@/lib/threadTree";
import { walkThread } from "@/lib/threadTree";
import type { CollapseState } from "@/state/collapseState";
import { WidgetCommentView } from "./WidgetCommentView";
import { WidgetThreadActions } from "./WidgetThreadActions";

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
  /**
   * True only at the top-level mount of the thread (the document-side
   * root call). Drives the per-thread bulk expand/collapse icons in
   * the header. Recursive descendant calls leave it unset so nested
   * reply rows never render the toolbar.
   */
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

  // The action toolbar lives only on the root row of a thread; nested
  // recursive calls default `isRoot` to false so descendants never get
  // one. `createElement` (vs JSX) keeps the prop assignment trivial
  // when `isRoot` is false — undefined `headerActions` simply skips the
  // slot inside `WidgetCommentView`.
  //
  // "Collapse all" is a HARD RESET: it overwrites every descendant's
  // existing collapsed/expanded state, by design. Once the user opts in
  // via this control, `CollapseState.has(id)` returns true for every id
  // touched, so the auto-expand priming branch in the effect above
  // won't re-flip them on the next mount — explicit user choice wins.
  const headerActions = isRoot
    ? createElement(WidgetThreadActions, {
        onExpandAll: () => setSubtreeCollapsed(root, collapseState, false),
        onCollapseAll: () => setSubtreeCollapsed(root, collapseState, true),
      })
    : undefined;

  return (
    <div className="remargin-widget-thread">
      <WidgetCommentView
        comment={root.comment}
        sourcePath={sourcePath}
        collapsed={collapsed}
        onToggle={() => collapseState.toggle(id)}
        onClick={onClick}
        summary={summary}
        headerActions={headerActions}
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

/**
 * Bulk-set the collapsed flag for `root` and every descendant. Exported
 * so unit tests can drive the same logic the per-thread expand/collapse
 * toolbar invokes without having to introspect React-rendered click
 * handlers.
 */
export function setSubtreeCollapsed(
  root: ThreadNode,
  collapseState: CollapseState,
  collapsed: boolean
): void {
  const ids = Array.from(walkThread(root)).map((c) => c.id);
  collapseState.setMany(ids, collapsed);
}
