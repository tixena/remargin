import { cn } from "@/lib/utils";

export interface ReactionPillsProps {
  /**
   * Reaction map as parsed from the comment frontmatter:
   * `{ [emoji]: [author, author, ...] }`. Values may be undefined to match
   * the generated `Partial<Record<string, string[]>>` shape.
   */
  reactions: Partial<Record<string, string[]>>;
  /** Current identity name; used to tell "mine" apart from others'. */
  me?: string | null;
  /**
   * Invoked when the user clicks a pill. `mine` reflects the state BEFORE
   * the click — if true, the handler should call `react --remove`.
   */
  onToggle: (emoji: string, mine: boolean) => void;
}

/**
 * Inline row of reaction pills. Alphabetically sorted for stable ordering
 * across refreshes. Pills the current identity has reacted to render with
 * an accent-colored background so "mine vs theirs" is visible at a glance.
 */
export function ReactionPills({ reactions, me, onToggle }: ReactionPillsProps) {
  const entries: Array<[string, string[]]> = [];
  for (const [emoji, authors] of Object.entries(reactions)) {
    if (authors && authors.length > 0) entries.push([emoji, authors]);
  }
  entries.sort(([a], [b]) => a.localeCompare(b));
  if (entries.length === 0) return null;
  return (
    <>
      {entries.map(([emoji, authors]) => {
        const mine = !!me && authors.includes(me);
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
            title={authors.join(", ")}
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
