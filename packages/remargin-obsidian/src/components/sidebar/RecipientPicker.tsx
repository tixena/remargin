import { Lock, Plus, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useParticipants } from "@/hooks/useParticipants";
import { authorLabel } from "@/lib/authorLabel";
import { pickerOptions } from "@/lib/pickerOptions";

const MAX_VISIBLE = 20;

export interface RecipientPickerProps {
  /** Currently selected participant ids, rendered as chips in order. */
  selected: string[];
  /** Called with the new selection after any add/remove. */
  onChange: (next: string[]) => void;
  /**
   * Participant ids that cannot be removed via the UI (e.g. the parent
   * author on a reply). Rendered with a lock icon and no `x` button.
   * This is purely decorative — the CLI enforces the "parent author
   * always in `to:`" invariant server-side (see task rem-kja).
   */
  locked?: string[];
}

/**
 * Multi-select recipient picker used by the inline composers. Renders a
 * row of chips plus an inline "+ Add" button that opens a filterable
 * participant list. Hides itself entirely when the vault has no
 * registered participants, so composers keep working on un-configured
 * vaults without a `to:` affordance at all.
 */
export function RecipientPicker({ selected, onChange, locked = [] }: RecipientPickerProps) {
  const { participants, resolveDisplayName, loading } = useParticipants();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const rowRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // The full list of options (active participants minus currently
  // selected). Filter is applied as a second pass so we can display a
  // "N more" hint based on the pre-filter size.
  const options = useMemo(() => pickerOptions(participants, selected), [participants, selected]);
  const filtered = useMemo(() => {
    const trimmed = query.trim().toLowerCase();
    if (!trimmed) return options;
    return options.filter((p) => {
      return (
        p.name.toLowerCase().includes(trimmed) || p.display_name.toLowerCase().includes(trimmed)
      );
    });
  }, [options, query]);
  const visible = filtered.slice(0, MAX_VISIBLE);
  const hiddenCount = filtered.length - visible.length;

  // Close the popover on outside click so it feels like a real popover
  // without pulling in a portal-rendering component.
  useEffect(() => {
    if (!open) return;
    const handler = (ev: MouseEvent) => {
      if (!rowRef.current) return;
      if (!rowRef.current.contains(ev.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  useEffect(() => {
    if (open) inputRef.current?.focus();
    else setQuery("");
  }, [open]);

  const handleAdd = useCallback(
    (id: string) => {
      onChange([...selected, id]);
      setQuery("");
      // Close after picking so the user can continue typing the comment.
      setOpen(false);
    },
    [onChange, selected]
  );

  const handleRemove = useCallback(
    (id: string) => {
      if (locked.includes(id)) return;
      onChange(selected.filter((s) => s !== id));
    },
    [locked, onChange, selected]
  );

  const handleInputKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLInputElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false);
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        const first = visible[0];
        if (first) handleAdd(first.name);
        return;
      }
      if (event.key === "Backspace" && query.length === 0) {
        // Drop the last removable (non-locked) chip on backspace with an
        // empty query — classic chip-input UX.
        for (let i = selected.length - 1; i >= 0; i -= 1) {
          const id = selected[i];
          if (id && !locked.includes(id)) {
            event.preventDefault();
            onChange(selected.slice(0, i).concat(selected.slice(i + 1)));
            return;
          }
        }
      }
    },
    [handleAdd, locked, onChange, query, selected, visible]
  );

  // Registry not yet loaded OR empty — hide the row entirely so
  // composers on un-registered vaults behave exactly like before. We
  // still wait for `loading` to settle so the row doesn't flash in and
  // out on the first render.
  if (!loading && participants.length === 0) return null;

  return (
    <div ref={rowRef} className="flex items-center gap-1 flex-wrap text-[10px] relative">
      <span className="text-text-faint">To:</span>
      {selected.map((id) => {
        const isLocked = locked.includes(id);
        const { label, title } = authorLabel(id, resolveDisplayName);
        return (
          <Badge
            key={id}
            className="gap-1 pl-1.5 pr-1 py-0 font-normal bg-bg-hover text-text-normal border-transparent"
            title={title}
          >
            {isLocked && <Lock className="w-2.5 h-2.5 text-text-faint" />}
            <span>{label}</span>
            {!isLocked && (
              <button
                type="button"
                aria-label={`Remove ${label}`}
                className="p-0 text-text-faint hover:text-text-normal"
                onClick={() => handleRemove(id)}
              >
                <X className="w-2.5 h-2.5" />
              </button>
            )}
          </Badge>
        );
      })}
      <Button
        variant="ghost"
        size="sm"
        className="h-5 px-1.5 text-[10px] gap-0.5 text-text-faint hover:text-text-normal"
        onClick={() => setOpen((prev) => !prev)}
        aria-label="Add recipient"
        aria-expanded={open}
      >
        <Plus className="w-2.5 h-2.5" />
        Add
      </Button>
      {open && (
        <div className="absolute top-full left-0 z-30 mt-1 w-60 max-w-full rounded border border-bg-border bg-bg-primary shadow-lg p-1">
          <Input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleInputKeyDown}
            placeholder="Search participants..."
            className="h-6 text-xs"
            aria-label="Search participants"
          />
          <div className="mt-1 max-h-48 overflow-y-auto">
            {visible.length === 0 ? (
              <div className="px-2 py-1 text-[9px] text-text-faint">
                {options.length === 0 ? "No more participants" : "No matches"}
              </div>
            ) : (
              visible.map((p) => (
                <button
                  type="button"
                  key={p.name}
                  className="flex items-center w-full gap-2 px-2 py-1 text-left text-xs hover:bg-bg-hover rounded-sm"
                  onClick={() => handleAdd(p.name)}
                >
                  <span className="text-text-normal">{p.display_name}</span>
                  {p.display_name !== p.name && (
                    <span className="ml-auto text-text-faint font-mono text-[9px]">{p.name}</span>
                  )}
                </button>
              ))
            )}
            {hiddenCount > 0 && (
              <div className="px-2 py-1 text-[9px] text-text-faint">
                {hiddenCount} more — keep typing to filter
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
