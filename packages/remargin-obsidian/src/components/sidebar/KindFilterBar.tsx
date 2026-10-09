/** The sidebar's kind filter: one togglable chip per kind in view. */

import { X } from "lucide-react";
import { useCallback } from "react";

/** Props for {@link KindFilterBar}. */
export interface KindFilterBarProps {
  /** The kinds present in the visible data, sorted and de-duplicated; empty while loading. */
  availableKinds: string[];
  /** Empty means no filter; otherwise a comment passes when any of its kinds is listed. */
  selected: string[];
  onChange: (next: string[]) => void;
}

/**
 * Horizontal row of togglable chips — one per `remargin_kind` value
 * currently visible in the sidebar's data — plus a clear affordance
 * when anything is selected. Rendered only when at least one kind
 * exists in the visible set; the bar is completely hidden otherwise
 * so a vanilla vault without kind usage shows no UI overhead.
 *
 * The bar is the single control for both the Inbox and Current-file
 * sections (OR filter across both). Filter state lives in
 * `RemarginSidebar` and resets on reload — we deliberately do NOT
 * persist it to plugin settings so opening a file in a new session
 * starts with everything visible.
 */
export function KindFilterBar({ availableKinds, selected, onChange }: KindFilterBarProps) {
  const toggle = useCallback(
    (kind: string) => {
      if (selected.includes(kind)) {
        onChange(selected.filter((k) => k !== kind));
      } else {
        onChange([...selected, kind]);
      }
    },
    [selected, onChange]
  );

  const clear = useCallback(() => {
    onChange([]);
  }, [onChange]);

  if (availableKinds.length === 0) return null;

  return (
    <div
      className="flex flex-wrap items-center gap-1 px-4 py-2 border-b border-bg-border bg-bg-secondary"
      role="toolbar"
      aria-label="Filter by comment kind"
    >
      <span className="text-[10px] font-semibold uppercase tracking-wide text-text-faint mr-1">
        Kind
      </span>
      {availableKinds.map((kind) => {
        const active = selected.includes(kind);
        return (
          <button
            key={kind}
            type="button"
            aria-pressed={active}
            onClick={() => toggle(kind)}
            className={[
              "inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-mono font-semibold transition-colors cursor-pointer border",
              active
                ? "bg-accent text-white border-accent"
                : "bg-transparent text-text-muted border-bg-border hover:bg-bg-hover hover:text-text-normal",
            ].join(" ")}
          >
            {kind}
          </button>
        );
      })}
      {selected.length > 0 && (
        <button
          type="button"
          onClick={clear}
          aria-label="Clear kind filter"
          title="Clear kind filter"
          className="inline-flex items-center gap-0.5 rounded-full px-1.5 py-0.5 text-[10px] font-semibold text-text-faint hover:text-text-normal hover:bg-bg-hover cursor-pointer ml-1"
        >
          <X className="w-2.5 h-2.5" />
          Clear
        </button>
      )}
    </div>
  );
}
