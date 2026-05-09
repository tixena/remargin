import type { MouseEvent } from "react";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";

export interface WidgetThreadActionsProps {
  onExpandAll: () => void;
  onCollapseAll: () => void;
}

/**
 * Right-aligned icon pair rendered only inside ROOT widget headers
 * (never on nested replies). Drives bulk expand/collapse across the
 * root + every descendant in one click. Icon names chosen from
 * Obsidian's bundled Lucide set: `chevrons-down` (expand) and
 * `chevrons-up` (collapse) read as "open/close everything below".
 *
 * Each handler calls `event.stopPropagation()` so the outer
 * `WidgetCommentView` click — which routes to `plugin.focusComment`
 * for sidebar focus — does NOT also fire when the user clicks the
 * action icons.
 */
export function WidgetThreadActions({ onExpandAll, onCollapseAll }: WidgetThreadActionsProps) {
  const handle = (cb: () => void) => (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    cb();
  };
  return (
    <div className="remargin-widget-thread-actions">
      <button
        type="button"
        aria-label="Expand all replies in this thread"
        title="Expand all replies in this thread"
        className="remargin-widget-thread-actions__btn"
        onClick={handle(onExpandAll)}
      >
        <ObsidianIcon icon="chevrons-down" size={12} />
      </button>
      <button
        type="button"
        aria-label="Collapse all replies in this thread"
        title="Collapse all replies in this thread"
        className="remargin-widget-thread-actions__btn"
        onClick={handle(onCollapseAll)}
      >
        <ObsidianIcon icon="chevrons-up" size={12} />
      </button>
    </div>
  );
}
