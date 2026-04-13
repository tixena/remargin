import { toRegex } from "diacritic-regex";
import { ChevronDown, Clock, FileText, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { InboxTree } from "@/components/sidebar/InboxTree";
import { MarkdownContent } from "@/components/sidebar/MarkdownContent";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import type { ExpandedComment } from "@/generated";
import { useBackend } from "@/hooks/useBackend";
import type { ViewMode } from "@/types";

/**
 * Build the diacritic- and case-insensitive pattern shipped to
 * `remargin search --regex`. `diacritic-regex` produces character classes
 * like `[CcÇç][AaÀàÁáÂâ...]...` which are compatible with the Rust `regex`
 * crate; because case-insensitivity is baked into every character class,
 * the CLI's `--ignore-case` flag is redundant and is intentionally omitted.
 */
const buildSearchPattern = toRegex({ flags: "i" });

/** Debounce window (ms) before a keystroke triggers a search CLI call. */
const SEARCH_DEBOUNCE_MS = 250;

type InboxFilter = "pending" | "all";

interface InboxFilterOption {
  value: InboxFilter;
  label: string;
}

// Extensible list of filter options. Add entries here (e.g. "to-you",
// "mentions", "acked") and extend the `InboxFilter` union to light up
// additional dropdown choices without touching the trigger markup.
const INBOX_FILTER_OPTIONS: readonly InboxFilterOption[] = [
  { value: "pending", label: "Pending" },
  { value: "all", label: "All" },
];

interface InboxItem {
  file: string;
  comment: ExpandedComment;
}

interface InboxSectionProps {
  onOpenAtLine?: (filePath: string, line?: number) => void;
  onMutation?: () => void;
  refreshKey?: number;
  /** View mode owned by RemarginSidebar (persisted in plugin settings). */
  viewMode?: ViewMode;
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

export function InboxSection({
  onOpenAtLine,
  onMutation,
  refreshKey,
  viewMode = "tree",
}: InboxSectionProps = {}) {
  const backend = useBackend();
  const [filter, setFilter] = useState<InboxFilter>("pending");
  const [items, setItems] = useState<InboxItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searchInput, setSearchInput] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");

  // Debounce keystrokes in the search textbox so we don't spawn a CLI
  // process on every character.
  useEffect(() => {
    const handle = setTimeout(() => {
      setDebouncedSearch(searchInput);
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [searchInput]);

  const isSearching = debouncedSearch.trim().length > 0;

  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshKey is a monotonic counter prop; including it recreates the callback when sibling sections mutate, triggering a re-fetch.
  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      if (isSearching) {
        // Global search mode: match across every document regardless of the
        // Pending/All filter. Build a diacritic- and case-insensitive regex
        // that remains literal for non-alpha characters (spaces, punctuation,
        // etc.), then hydrate matched comment IDs via `query` so the rest of
        // the inbox UI can render them uniformly.
        const pattern = buildSearchPattern(debouncedSearch.trim()).source;
        const matches = await backend.search(pattern, {
          regex: true,
          scope: "comments",
        });
        // Group matched comment IDs by file so we make one query call per
        // file rather than per match.
        const idsByFile = new Map<string, Set<string>>();
        for (const m of matches) {
          if (!m.comment_id) continue;
          let set = idsByFile.get(m.path);
          if (!set) {
            set = new Set<string>();
            idsByFile.set(m.path, set);
          }
          set.add(m.comment_id);
        }
        const flat: InboxItem[] = [];
        for (const [file, ids] of idsByFile) {
          const results = await backend.query(file, { expanded: true });
          for (const result of results) {
            for (const comment of result.comments) {
              if (comment.id && ids.has(comment.id)) {
                flat.push({ file: result.path, comment });
              }
            }
          }
        }
        flat.sort((a, b) => (b.comment.ts ?? "").localeCompare(a.comment.ts ?? ""));
        setItems(flat);
        setError(null);
      } else {
        const results = await backend.query(".", {
          pending: filter === "pending",
          expanded: true,
        });
        const flat: InboxItem[] = [];
        for (const result of results) {
          for (const comment of result.comments) {
            flat.push({ file: result.path, comment });
          }
        }
        flat.sort((a, b) => (b.comment.ts ?? "").localeCompare(a.comment.ts ?? ""));
        setItems(flat);
        setError(null);
      }
    } catch (err) {
      console.error("InboxSection.refresh failed:", err);
      setItems([]);
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [backend, filter, refreshKey, isSearching, debouncedSearch]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleAck = useCallback(
    async (file: string, id: string) => {
      try {
        await backend.ack(file, [id]);
        // Stage the file in the user's sandbox so the interaction is
        // visible in the next Submit-to-Claude cycle.
        try {
          await backend.sandboxAdd([file]);
        } catch {
          // Best-effort: ack succeeded, don't fail the whole operation.
        }
        await refresh();
        onMutation?.();
      } catch (err) {
        console.error("InboxSection.ack failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, refresh, onMutation]
  );

  const filterLabel = useMemo(
    () => INBOX_FILTER_OPTIONS.find((o) => o.value === filter)?.label ?? filter,
    [filter]
  );

  if (loading) {
    return <div className="px-4 py-3 text-xs text-text-faint">Loading...</div>;
  }

  return (
    <div className="flex flex-col">
      <div className="flex flex-col gap-2 px-4 py-2 border-b border-bg-border">
        <div className="relative">
          <Input
            type="text"
            value={searchInput}
            onChange={(e) => setSearchInput(e.target.value)}
            placeholder="Search comments..."
            aria-label="Search comments"
            className="h-7 text-xs pr-7"
          />
          {searchInput.length > 0 && (
            <button
              type="button"
              aria-label="Clear search"
              onClick={() => setSearchInput("")}
              className="absolute right-1 top-1/2 -translate-y-1/2 p-0.5 text-text-faint hover:text-text-normal"
            >
              <X className="w-3 h-3" />
            </button>
          )}
        </div>
        <div className="flex items-center justify-between gap-2">
          <DropdownMenu>
            <DropdownMenuTrigger asChild disabled={isSearching}>
              <Button
                variant="outline"
                size="sm"
                disabled={isSearching}
                title={
                  isSearching ? "Pending/All is disabled while a search is active." : undefined
                }
                className={`h-7 px-2 text-xs font-medium text-text-normal gap-1.5 ${
                  isSearching ? "opacity-50" : ""
                }`}
              >
                {isSearching ? "Search results" : filterLabel}
                <ChevronDown className="w-3 h-3 text-text-muted" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="min-w-28">
              {INBOX_FILTER_OPTIONS.map((option) => (
                <DropdownMenuItem
                  key={option.value}
                  onClick={() => setFilter(option.value)}
                  className="text-xs"
                >
                  {option.label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      <div>
        {error ? (
          <div className="px-4 py-3 text-xs text-red-400 whitespace-pre-wrap break-words">
            <div className="font-semibold mb-1">Failed to load inbox</div>
            <div className="font-mono text-[10px]">{error}</div>
          </div>
        ) : items.length === 0 ? (
          <div className="px-4 py-3 text-xs text-text-faint">
            {isSearching
              ? "No comments match your search."
              : filter === "pending"
                ? "No pending comments."
                : "No comments found."}
          </div>
        ) : viewMode === "tree" ? (
          <InboxTree items={items} onOpenAtLine={onOpenAtLine} onAck={handleAck} />
        ) : (
          <div className="flex flex-col">
            {items.map((item) => (
              <div
                key={`${item.file}:${item.comment.id}`}
                className="flex flex-col gap-1 px-4 py-2 border-b border-bg-border hover:bg-bg-hover cursor-pointer min-w-0 overflow-hidden"
                onClick={() => onOpenAtLine?.(item.file, item.comment.line)}
              >
                <div className="flex items-center justify-between gap-2 min-w-0">
                  <div className="flex items-center gap-1.5 min-w-0 flex-1">
                    <Badge
                      className={`px-1 py-0 text-[9px] font-semibold shrink-0 ${
                        item.comment.author_type === "agent"
                          ? "bg-purple-400 text-white"
                          : "bg-blue-400 text-white"
                      }`}
                    >
                      {item.comment.author_type === "agent" ? "AI" : "H"}
                    </Badge>
                    {item.comment.id && (
                      <Badge className="px-1 py-0 text-[9px] font-mono font-semibold bg-slate-500 text-white shrink-0">
                        {item.comment.id}
                      </Badge>
                    )}
                    {item.comment.line > 0 && (
                      <span className="text-[9px] text-text-faint font-mono shrink-0">
                        L{item.comment.line}
                      </span>
                    )}
                    <span className="text-xs font-medium text-text-normal truncate min-w-0">
                      {item.comment.author}
                    </span>
                  </div>
                  <div className="flex items-center gap-1 shrink-0">
                    <Clock className="w-3 h-3 text-text-faint" />
                    <span className="text-[10px] text-text-faint whitespace-nowrap">
                      {formatRelativeTime(item.comment.ts)}
                    </span>
                  </div>
                </div>
                <div className="line-clamp-2 overflow-hidden min-w-0">
                  <MarkdownContent
                    content={item.comment.content ?? ""}
                    sourcePath={item.file}
                    className="min-w-0"
                  />
                </div>
                <div className="flex items-center justify-between gap-2 min-w-0">
                  <div className="flex items-center gap-1 min-w-0 flex-1">
                    <FileText className="w-3 h-3 text-text-faint shrink-0" />
                    <span className="font-mono text-[10px] text-text-faint truncate min-w-0">
                      {item.file}
                    </span>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function formatRelativeTime(ts?: string): string {
  if (!ts) return "";
  try {
    const diff = Date.now() - new Date(ts).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "now";
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.floor(hours / 24);
    return `${days}d`;
  } catch {
    return "";
  }
}
