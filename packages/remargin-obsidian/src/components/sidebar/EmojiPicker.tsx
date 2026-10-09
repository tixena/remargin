/** The reaction emoji picker. */

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";

/** Props for {@link EmojiPicker}. */
export interface EmojiPickerProps {
  onPick: (emoji: string) => void;
  disabled?: boolean;
}

/** A short curated list, so no multi-megabyte emoji picker enters the Obsidian bundle. */
const QUICK_EMOJIS: readonly string[] = [
  "👍",
  "👎",
  "❤️",
  "🎉",
  "🚀",
  "👀",
  "😄",
  "😕",
  "🔥",
  "✅",
  "❌",
  "🙏",
  "💩",
  "🏆",
  "😒",
  "🏳️‍🌈",
];

/**
 * Small popover-style emoji picker. Uses the existing DropdownMenu primitive
 * (no Popover is currently bundled) with a grid body. Clicking an emoji
 * closes the menu and dispatches the picked character to the parent.
 */
export function EmojiPicker({ onPick, disabled }: EmojiPickerProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        {/* Obsidian's host theme paints a background and border on bare <button> elements that
            Tailwind classes do not reliably beat, hence the explicit inline style. */}
        <button
          type="button"
          onClick={(e) => e.stopPropagation()}
          disabled={disabled}
          aria-label="Add reaction"
          title="Add reaction"
          style={{
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            width: "var(--input-height)",
            height: "var(--input-height)",
            borderRadius: 4,
            border: "none",
            cursor: "pointer",
            backgroundColor: "transparent",
            padding: 0,
            color: "var(--text-faint)",
            flexShrink: 0,
            opacity: disabled ? 0.4 : 1,
            pointerEvents: disabled ? "none" : "auto",
          }}
          onMouseEnter={(e) => {
            if (disabled) return;
            e.currentTarget.style.backgroundColor = "var(--background-modifier-hover)";
            e.currentTarget.style.color = "var(--text-muted)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
            e.currentTarget.style.color = "var(--text-faint)";
          }}
        >
          <ObsidianIcon icon="smile-plus" size={12} />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="p-1 w-auto">
        <div className="grid grid-cols-6 gap-0.5">
          {QUICK_EMOJIS.map((emoji) => (
            <button
              type="button"
              key={emoji}
              className="inline-flex items-center justify-center w-6 h-6 text-sm rounded-sm hover:bg-bg-hover"
              onClick={(e) => {
                e.stopPropagation();
                onPick(emoji);
              }}
            >
              {emoji}
            </button>
          ))}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
