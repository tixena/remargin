import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";

export interface EmojiPickerProps {
  /** Invoked with the chosen emoji (unicode character). */
  onPick: (emoji: string) => void;
  /** Disable the trigger when the underlying comment is not yet persisted. */
  disabled?: boolean;
}

/**
 * Curated set of common reaction emojis. A full emoji-mart integration is
 * deferred — for P2 reactions this short list covers the everyday cases
 * without dragging a multi-megabyte picker into the Obsidian bundle.
 */
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
        {/*
         * Inline style mirrors the panel-header refresh button
         * (SidebarShell) — Obsidian's host theme paints a default
         * background + border on bare <button> elements which Tailwind
         * classes alone don't reliably override. Setting `border:none`
         * and `backgroundColor:transparent` explicitly keeps the icon
         * borderless regardless of theme.
         */}
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
            width: 20,
            height: 20,
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
