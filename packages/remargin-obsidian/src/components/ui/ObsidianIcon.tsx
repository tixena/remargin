import { setIcon } from "obsidian";
import { useEffect, useRef } from "react";

interface ObsidianIconProps {
  /** Obsidian/Lucide icon name, e.g. "smile-plus", "reply", "trash-2". */
  icon: string;
  size?: number;
  className?: string;
}

/**
 * Renders a single Obsidian-native icon via `setIcon`. Use this instead of
 * lucide-react SVG components inside buttons — Obsidian's host theme scopes
 * custom SVGs out of button elements, making them invisible.
 */
export function ObsidianIcon({ icon, size = 14, className }: ObsidianIconProps) {
  const ref = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (ref.current) {
      setIcon(ref.current, icon);
    }
  }, [icon]);

  return (
    <span
      ref={ref}
      className={className}
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: size,
        height: size,
        flexShrink: 0,
      }}
    />
  );
}
