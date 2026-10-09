/** The chevron that collapses or expands an editor-side comment widget. */

import type { MouseEvent } from "react";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";

/** Props for {@link CollapseToggle}. */
interface CollapseToggleProps {
  collapsed: boolean;
  onToggle: () => void;
}

/**
 * Single-chevron toggle that flips between right (collapsed) and down
 * (expanded). Used inside `WidgetCommentView` so the editor-side widget
 * carries the same affordance as the sidebar collapsibles.
 *
 * Click events are stopped at this button so toggling the widget body
 * does NOT also dispatch the surrounding card's "open me in the sidebar"
 * click — those two interactions need to be independently triggerable.
 */
export function CollapseToggle({ collapsed, onToggle }: CollapseToggleProps) {
  const handleClick = (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    onToggle();
  };

  return (
    <button
      type="button"
      className="remargin-collapse-toggle"
      aria-label={collapsed ? "Expand comment" : "Collapse comment"}
      aria-expanded={!collapsed}
      onClick={handleClick}
    >
      <ObsidianIcon icon={collapsed ? "chevron-right" : "chevron-down"} size={12} />
    </button>
  );
}
