/** The row of reaction pills on a comment card. */

import type { ReactionEntry } from "@/generated";
import { useParticipants } from "@/hooks/useParticipants";
import { cn } from "@/lib/utils";

/** Props for {@link ReactionPills}. */
export interface ReactionPillsProps {
  reactions: Partial<Record<string, ReactionEntry[]>>;
  me?: string | null;
  /** `mine` is the state before the click: when true the handler should remove the reaction. */
  onToggle: (emoji: string, mine: boolean) => void;
}

/**
 * Inline row of reaction pills. Alphabetically sorted for stable ordering
 * across refreshes. Pills the current identity has reacted to render with
 * an accent-colored background so "mine vs theirs" is visible at a glance.
 */
export function ReactionPills({ reactions, me, onToggle }: ReactionPillsProps) {
  const { resolveDisplayName } = useParticipants();
  const entries: Array<[string, string[]]> = [];
  for (const [emoji, items] of Object.entries(reactions)) {
    if (items && items.length > 0) entries.push([emoji, items.map((e) => e.author)]);
  }
  entries.sort(([a], [b]) => a.localeCompare(b));
  if (entries.length === 0) return null;
  return (
    <>
      {entries.map(([emoji, authors]) => {
        const mine = !!me && authors.includes(me);
        const tooltip = authors.map((a) => resolveDisplayName(a)).join(", ");
        return (
          <button
            type="button"
            key={emoji}
            className={cn(
              "inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px] leading-none transition-colors",
              mine
                ? "bg-accent/20 text-text-normal border border-accent"
                : "bg-bg-border text-text-muted border border-transparent hover:bg-bg-hover"
            )}
            onClick={(e) => {
              e.stopPropagation();
              onToggle(emoji, mine);
            }}
            title={tooltip}
            aria-label={`${emoji} ${authors.length}${mine ? " (click to remove)" : ""}`}
          >
            <span className="text-[11px]">{emoji}</span>
            <span className="font-mono text-[9px] font-semibold">{authors.length}</span>
          </button>
        );
      })}
    </>
  );
}
