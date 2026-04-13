import { FolderTree, List } from "lucide-react";
import type { ViewMode } from "@/types";

export interface ViewToggleProps {
  value: ViewMode;
  onChange: (next: ViewMode) => void;
}

/**
 * Paired list/tree toggle used in the right-slot of the Sandbox and Inbox
 * section headers. The active option renders with a filled `bg-hover`
 * background and a muted icon; the other stays transparent with a faint
 * icon. Clicks stop propagation so toggling does not also collapse the
 * enclosing Collapsible section.
 */
export function ViewToggle({ value, onChange }: ViewToggleProps) {
  return (
    <div
      className="flex items-center gap-0.5"
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
    >
      <button
        type="button"
        className={`inline-flex items-center justify-center w-[22px] h-[22px] rounded-sm transition-colors ${
          value === "flat"
            ? "bg-bg-hover text-text-normal"
            : "bg-transparent text-text-muted hover:text-text-normal"
        }`}
        aria-pressed={value === "flat"}
        aria-label="Flat view"
        title="Flat view"
        onClick={() => onChange("flat")}
      >
        <List className="w-3 h-3" />
      </button>
      <button
        type="button"
        className={`inline-flex items-center justify-center w-[22px] h-[22px] rounded-sm transition-colors ${
          value === "tree"
            ? "bg-bg-hover text-text-normal"
            : "bg-transparent text-text-muted hover:text-text-normal"
        }`}
        aria-pressed={value === "tree"}
        aria-label="Tree view"
        title="Tree view"
        onClick={() => onChange("tree")}
      >
        <FolderTree className="w-3 h-3" />
      </button>
    </div>
  );
}
