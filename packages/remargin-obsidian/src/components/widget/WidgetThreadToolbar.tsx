import type { CollapseState } from "@/state/collapseState";

export interface WidgetThreadToolbarProps {
  /**
   * Ids of the root comments this toolbar controls. Bulk Expand/Collapse
   * affects exactly these ids — children inside each root keep their own
   * collapse state (a collapsed root hides its replies anyway).
   */
  rootIds: readonly string[];
  collapseState: CollapseState;
}

/**
 * Per-block "Expand all" / "Collapse all" toolbar. One click bulk-sets
 * every root id through `CollapseState.setMany`, which fires one
 * notification per id that actually changed value (subscribers get N
 * mutations, not one all-clear).
 */
export function WidgetThreadToolbar({ rootIds, collapseState }: WidgetThreadToolbarProps) {
  return (
    <div className="remargin-widget-toolbar">
      <button
        type="button"
        className="remargin-widget-toolbar__btn"
        onClick={() => collapseState.setMany(rootIds, false)}
      >
        Expand all
      </button>
      <button
        type="button"
        className="remargin-widget-toolbar__btn"
        onClick={() => collapseState.setMany(rootIds, true)}
      >
        Collapse all
      </button>
    </div>
  );
}
