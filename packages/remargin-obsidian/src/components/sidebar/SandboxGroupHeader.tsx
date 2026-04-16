import { ChevronDown, ChevronRight } from "lucide-react";
import { useCallback } from "react";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";

export type SandboxGroupBulkIcon = "check-check" | "minus" | "plus";

export interface SandboxGroupHeaderProps {
  /** Group label shown to the right of the chevron. */
  label: string;
  /** Number of files currently in this group; shown as a pill badge. */
  count: number;
  /** Whether the group is currently expanded. */
  open: boolean;
  /** Toggle the group open/closed. */
  onToggleOpen: () => void;
  /**
   * Left bulk-action icon. The exact semantics depend on the group:
   *   - Staged: "check-check" = toggle select-all across staged rows
   *   - Unstaged: "plus" = stage the current selection (or everything)
   */
  leftBulkIcon: "check-check" | "plus";
  /** Tooltip for the left bulk-action button. */
  leftBulkTitle: string;
  /** Invoked when the user clicks the left bulk-action button. */
  onLeftBulk: () => void;
  /**
   * Right bulk-action icon.
   *   - Staged: "minus" = unstage selected (or all) staged rows
   *   - Unstaged: "check-check" = stage everything in the unstaged group
   */
  rightBulkIcon: "minus" | "check-check";
  /** Tooltip for the right bulk-action button. */
  rightBulkTitle: string;
  /** Invoked when the user clicks the right bulk-action button. */
  onRightBulk: () => void;
  /** Disable bulk actions when the group is empty. */
  disabled?: boolean;
}

function BulkIcon({ name }: { name: SandboxGroupBulkIcon }) {
  return <ObsidianIcon icon={name} size={12} />;
}

/**
 * Header row for a Sandbox sub-group (Staged / Unstaged). Renders a chevron,
 * a label, a count badge, and two bulk-action icon buttons. The bulk-action
 * semantics are delegated to the parent via callbacks; this component is
 * presentational and only dispatches clicks.
 */
export function SandboxGroupHeader({
  label,
  count,
  open,
  onToggleOpen,
  leftBulkIcon,
  leftBulkTitle,
  onLeftBulk,
  rightBulkIcon,
  rightBulkTitle,
  onRightBulk,
  disabled,
}: SandboxGroupHeaderProps) {
  const Chevron = open ? ChevronDown : ChevronRight;

  const stopAndRun = useCallback(
    (fn: () => void) => (e: React.MouseEvent) => {
      e.stopPropagation();
      fn();
    },
    []
  );

  return (
    <div
      className="flex items-center justify-between px-4 py-1 bg-bg-hover cursor-pointer select-none"
      onClick={onToggleOpen}
    >
      <div className="flex items-center gap-1.5">
        <Chevron className="w-2.5 h-2.5 text-text-faint" />
        <span className="text-[11px] font-semibold text-text-muted">{label}</span>
        <span className="inline-flex items-center justify-center min-w-4 h-4 px-1.5 text-[9px] text-text-muted bg-bg-border rounded-full">
          {count}
        </span>
      </div>
      <div className="flex items-center gap-0.5">
        <button
          type="button"
          className="flex items-center justify-center w-5 h-5 rounded-sm text-text-faint hover:text-text-normal hover:bg-bg-border disabled:opacity-40 disabled:pointer-events-none"
          title={leftBulkTitle}
          onClick={stopAndRun(onLeftBulk)}
          disabled={disabled}
        >
          <BulkIcon name={leftBulkIcon} />
        </button>
        <button
          type="button"
          className="flex items-center justify-center w-5 h-5 rounded-sm text-text-faint hover:text-text-normal hover:bg-bg-border disabled:opacity-40 disabled:pointer-events-none"
          title={rightBulkTitle}
          onClick={stopAndRun(onRightBulk)}
          disabled={disabled}
        >
          <BulkIcon name={rightBulkIcon} />
        </button>
      </div>
    </div>
  );
}
