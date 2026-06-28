import { useState } from "react";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { usePlugin } from "@/hooks/usePlugin";
import {
  clampMarkdownScale,
  MARKDOWN_SCALE_DEFAULT,
  MARKDOWN_SCALE_MAX,
  MARKDOWN_SCALE_MIN,
  MARKDOWN_SCALE_STEP,
} from "@/types";

const iconBtnStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  width: 22,
  height: 22,
  borderRadius: 4,
  border: "none",
  cursor: "pointer",
  backgroundColor: "transparent",
  padding: 0,
  flexShrink: 0,
  color: "var(--text-muted)",
};

/**
 * Sidebar-toolbar control for the single global comment-markdown font
 * scale. minus / reset / plus adjust `plugin.settings.markdownScale`,
 * which the plugin persists and pushes to the shared `--remargin-md-scale`
 * CSS var so both the sidebar and the editor widgets restyle live.
 */
export function FontScaleControl() {
  const plugin = usePlugin();
  const [scale, setScale] = useState(() => clampMarkdownScale(plugin.settings.markdownScale));

  const commit = (next: number) => {
    const clamped = clampMarkdownScale(next);
    setScale(clamped);
    void plugin.setMarkdownScale(clamped);
  };

  const percent = Math.round(scale * 100);
  const atMin = scale <= MARKDOWN_SCALE_MIN;
  const atMax = scale >= MARKDOWN_SCALE_MAX;

  return (
    <div style={{ display: "inline-flex", alignItems: "center", gap: 2, flexShrink: 0 }}>
      <button
        type="button"
        onClick={() => commit(scale - MARKDOWN_SCALE_STEP)}
        disabled={atMin}
        aria-label="Decrease comment font size"
        title="Decrease comment font size"
        style={{ ...iconBtnStyle, opacity: atMin ? 0.4 : 1, cursor: atMin ? "default" : "pointer" }}
      >
        <ObsidianIcon icon="minus" size={12} />
      </button>
      <button
        type="button"
        onClick={() => commit(MARKDOWN_SCALE_DEFAULT)}
        aria-label="Reset comment font size"
        title={`Reset comment font size (${percent}%)`}
        style={{
          border: "none",
          background: "transparent",
          cursor: "pointer",
          color: "var(--text-muted)",
          fontSize: 11,
          fontVariantNumeric: "tabular-nums",
          minWidth: 30,
          textAlign: "center",
          padding: 0,
        }}
      >
        {percent}%
      </button>
      <button
        type="button"
        onClick={() => commit(scale + MARKDOWN_SCALE_STEP)}
        disabled={atMax}
        aria-label="Increase comment font size"
        title="Increase comment font size"
        style={{ ...iconBtnStyle, opacity: atMax ? 0.4 : 1, cursor: atMax ? "default" : "pointer" }}
      >
        <ObsidianIcon icon="plus" size={12} />
      </button>
    </div>
  );
}
