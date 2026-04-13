import { setIcon } from "obsidian";
import { useEffect, useRef } from "react";
import type { ViewMode } from "@/types";

export interface ViewToggleProps {
  value: ViewMode;
  onChange: (next: ViewMode) => void;
}

function IconButton({
  icon,
  active,
  label,
  onClick,
}: {
  icon: string;
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  const iconRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (iconRef.current) {
      setIcon(iconRef.current, icon);
    }
  }, [icon]);

  return (
    <button
      type="button"
      style={{
        width: 22,
        height: 22,
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        borderRadius: 4,
        transition: "background-color 120ms",
        border: "none",
        cursor: "pointer",
        backgroundColor: active ? "var(--background-modifier-hover)" : "transparent",
        color: active ? "var(--text-normal)" : "var(--text-muted)",
      }}
      aria-pressed={active}
      aria-label={label}
      title={label}
      onClick={onClick}
    >
      <span
        ref={iconRef}
        style={{
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          width: 14,
          height: 14,
        }}
      />
    </button>
  );
}

/**
 * Paired list/tree toggle used in the right-slot of the Sandbox and Inbox
 * section headers. Icons are rendered via Obsidian's native `setIcon` API
 * rather than inline SVGs — the host theme scopes custom SVGs out of
 * buttons, so we use the icon system the app expects.
 */
export function ViewToggle({ value, onChange }: ViewToggleProps) {
  return (
    <div
      className="flex items-center gap-0.5"
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
    >
      <IconButton
        icon="list"
        active={value === "flat"}
        label="Flat view"
        onClick={() => onChange("flat")}
      />
      <IconButton
        icon="folder-tree"
        active={value === "tree"}
        label="Tree view"
        onClick={() => onChange("tree")}
      />
    </div>
  );
}
