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
        <button
          type="button"
          className="inline-flex items-center justify-center w-5 h-5 rounded-sm text-text-faint hover:text-text-muted hover:bg-bg-hover disabled:opacity-40 disabled:pointer-events-none"
          onClick={(e) => e.stopPropagation()}
          disabled={disabled}
          aria-label="Add reaction"
          title="Add reaction"
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
