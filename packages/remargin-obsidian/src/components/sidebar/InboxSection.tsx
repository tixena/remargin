import { Check, Clock, FileText } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { ExpandedComment } from "@/generated";
import { useBackend } from "@/hooks/useBackend";

interface InboxItem {
  file: string;
  comment: ExpandedComment;
}

interface InboxSectionProps {
  onOpenAtLine?: (filePath: string, line?: number) => void;
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

export function InboxSection({ onOpenAtLine }: InboxSectionProps = {}) {
  const backend = useBackend();
  const [filter, setFilter] = useState<"all" | "pending">("pending");
  const [items, setItems] = useState<InboxItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
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
    } catch (err) {
      console.error("InboxSection.refresh failed:", err);
      setItems([]);
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [backend, filter]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleAck = useCallback(
    async (file: string, id: string) => {
      try {
        await backend.ack(file, [id]);
        await refresh();
      } catch (err) {
        console.error("InboxSection.ack failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, refresh]
  );

  if (loading) {
    return <div className="px-4 py-3 text-xs text-text-faint">Loading...</div>;
  }

  return (
    <div className="flex flex-col">
      <div className="flex items-center gap-2 px-4 py-2 border-b border-bg-border">
        <Select value={filter} onValueChange={(v) => setFilter(v as "all" | "pending")}>
          <SelectTrigger className="h-7 text-xs w-28">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="pending">Pending</SelectItem>
            <SelectItem value="all">All</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div>
        {error ? (
          <div className="px-4 py-3 text-xs text-red-400 whitespace-pre-wrap break-words">
            <div className="font-semibold mb-1">Failed to load inbox</div>
            <div className="font-mono text-[10px]">{error}</div>
          </div>
        ) : items.length === 0 ? (
          <div className="px-4 py-3 text-xs text-text-faint">
            {filter === "pending" ? "No pending comments." : "No comments found."}
          </div>
        ) : (
          <div className="flex flex-col">
            {items.map((item) => (
              <div
                key={`${item.file}:${item.comment.id}`}
                className="flex flex-col gap-1 px-4 py-2 border-b border-bg-border hover:bg-bg-hover cursor-pointer"
                onClick={() => onOpenAtLine?.(item.file, item.comment.line)}
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-1.5 min-w-0">
                    <Badge
                      className={`px-1 py-0 text-[9px] font-semibold ${
                        item.comment.author_type === "agent"
                          ? "bg-purple-400 text-white"
                          : "bg-blue-400 text-white"
                      }`}
                    >
                      {item.comment.author_type === "agent" ? "AI" : "H"}
                    </Badge>
                    {item.comment.id && (
                      <Badge className="px-1 py-0 text-[9px] font-mono font-semibold bg-slate-500 text-white">
                        {item.comment.id}
                      </Badge>
                    )}
                    {item.comment.line > 0 && (
                      <span className="text-[9px] text-text-faint font-mono">
                        L{item.comment.line}
                      </span>
                    )}
                    <span className="text-xs font-medium text-text-normal truncate">
                      {item.comment.author}
                    </span>
                  </div>
                  <div className="flex items-center gap-1">
                    <Clock className="w-3 h-3 text-text-faint" />
                    <span className="text-[10px] text-text-faint whitespace-nowrap">
                      {formatRelativeTime(item.comment.ts)}
                    </span>
                  </div>
                </div>
                <p className="text-xs text-text-muted line-clamp-2">
                  {item.comment.content?.split("\n")[0] ?? ""}
                </p>
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-1">
                    <FileText className="w-3 h-3 text-text-faint" />
                    <span className="font-mono text-[10px] text-text-faint truncate">
                      {item.file}
                    </span>
                  </div>
                  {item.comment.ack?.length === 0 && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-5 px-1.5 text-[10px] text-green-500 hover:text-green-400"
                      onClick={(e) => {
                        e.stopPropagation();
                        if (item.comment.id) {
                          handleAck(item.file, item.comment.id);
                        }
                      }}
                    >
                      <Check className="w-3 h-3 mr-0.5" />
                      Ack
                    </Button>
                  )}
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
