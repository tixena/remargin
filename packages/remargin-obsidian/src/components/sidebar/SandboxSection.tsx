import { FileText, FolderTree, List, Send } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { FileTree } from "@/components/sidebar/FileTree";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useBackend } from "@/hooks/useBackend";

interface SandboxSectionProps {
  /**
   * Bumped by the parent whenever the sandbox list should be refetched
   * (e.g. after a successful inline-comment submit or a sidepanel refresh
   * button click). The value itself is opaque — any change triggers a
   * refetch.
   */
  refreshKey?: number;
  /**
   * Called to open a staged file in the editor when the user clicks on it.
   * Receives the path exactly as the CLI reported it.
   */
  onOpenFile?: (path: string) => void;
  /**
   * Forwarded Submit-to-Claude handler. The parent is responsible for the
   * actual Claude work; this component only tracks which files are checked
   * and clears them from the sandbox via `remargin sandbox remove` after the
   * handler resolves.
   */
  onSubmit?: (stagedFiles: string[]) => Promise<void> | void;
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

/**
 * Sidebar section that lists files the current identity has staged in its
 * remargin sandbox. The authoritative list comes from `remargin sandbox list
 * --json` — this component never invents entries of its own.
 *
 * Checkbox state is local: unchecking a row excludes it from the next Submit
 * but does NOT mutate the persistent sandbox. Only a successful Submit-to-
 * Claude clears the submitted rows (via `remargin sandbox remove`).
 */
export function SandboxSection({ refreshKey, onOpenFile, onSubmit }: SandboxSectionProps) {
  const backend = useBackend();
  const [files, setFiles] = useState<string[]>([]);
  const [staged, setStaged] = useState<Set<string>>(new Set());
  const [viewMode, setViewMode] = useState<"flat" | "tree">("tree");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const refresh = useCallback(
    async (_key?: number) => {
      // `_key` is accepted so the useEffect below can pass `refreshKey` and
      // satisfy the useExhaustiveDependencies lint — the value itself is
      // unused; it just ensures the callback identity is tied to the current
      // refresh generation.
      setLoading(true);
      try {
        const entries = await backend.sandboxList();
        const paths = entries.map((e) => e.path);
        setFiles(paths);
        // Reset the "staged" selection to mirror the new authoritative list.
        // Keep any previously-checked files that are still present; newly-
        // added files default to checked so the user does not have to re-
        // check them after adding with `+`.
        setStaged((prev) => {
          const next = new Set<string>();
          for (const path of paths) {
            if (prev.size === 0 || prev.has(path)) {
              next.add(path);
            }
          }
          // If nothing carried over (e.g. all previous entries were cleared),
          // default to all-checked.
          if (next.size === 0) {
            for (const path of paths) next.add(path);
          }
          return next;
        });
        setError(null);
      } catch (err) {
        console.error("SandboxSection.refresh failed:", err);
        setFiles([]);
        setError(errorMessage(err));
      } finally {
        setLoading(false);
      }
    },
    [backend]
  );

  useEffect(() => {
    refresh(refreshKey);
  }, [refresh, refreshKey]);

  const toggleStaged = useCallback((path: string) => {
    setStaged((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const toggleAll = useCallback(() => {
    setStaged((prev) => {
      if (prev.size === files.length) return new Set();
      return new Set(files);
    });
  }, [files]);

  const handleSubmit = useCallback(async () => {
    if (submitting) return;
    const selected = files.filter((f) => staged.has(f));
    if (selected.length === 0) return;
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit?.(selected);
      // Submit succeeded — clear the selected files from the persistent
      // sandbox and refetch. We only call `sandbox remove` for files the
      // user actually submitted; anything they unchecked stays staged.
      try {
        await backend.sandboxRemove(selected);
      } catch (err) {
        // Do NOT roll back: the parent's Submit already happened (Claude has
        // done the work). Surface the specific unstage failure so the user
        // can clean up manually.
        console.error("SandboxSection.sandboxRemove failed:", err);
        setError(`Submit succeeded but failed to unstage: ${errorMessage(err)}`);
      }
      await refresh();
    } catch (err) {
      console.error("SandboxSection.onSubmit failed:", err);
      setError(errorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }, [backend, files, staged, onSubmit, refresh, submitting]);

  if (loading && files.length === 0) {
    return <div className="px-4 py-3 text-xs text-text-faint">Loading sandbox...</div>;
  }

  if (error && files.length === 0) {
    return (
      <div className="px-4 py-3 text-xs text-red-400 whitespace-pre-wrap break-words">
        <div className="font-semibold mb-1">Failed to load sandbox</div>
        <div className="font-mono text-[10px]">{error}</div>
      </div>
    );
  }

  if (files.length === 0) {
    return (
      <div className="px-4 py-3 text-xs text-text-faint">
        Sandbox is empty. Add a comment with + and it will appear here.
      </div>
    );
  }

  return (
    <div className="flex flex-col">
      <div className="flex items-center justify-between px-4 py-1.5 border-b border-bg-border">
        <div className="flex items-center gap-2">
          <Checkbox
            checked={staged.size === files.length}
            onCheckedChange={toggleAll}
            className="w-3.5 h-3.5"
          />
          <span className="text-[10px] text-text-faint">
            {staged.size}/{files.length} staged
          </span>
        </div>
        <ToggleGroup
          type="single"
          value={viewMode}
          onValueChange={(v) => v && setViewMode(v as "flat" | "tree")}
          className="gap-0"
        >
          <ToggleGroupItem value="flat" className="h-6 w-6 p-0">
            <List className="w-3 h-3" />
          </ToggleGroupItem>
          <ToggleGroupItem value="tree" className="h-6 w-6 p-0">
            <FolderTree className="w-3 h-3" />
          </ToggleGroupItem>
        </ToggleGroup>
      </div>

      {error && (
        <div className="px-4 py-2 text-[10px] text-red-400 font-mono whitespace-pre-wrap break-words">
          {error}
        </div>
      )}

      <ScrollArea className="max-h-40">
        {viewMode === "flat" ? (
          <div className="flex flex-col">
            {files.map((file) => (
              <div key={file} className="flex items-center gap-2 px-4 py-1.5 hover:bg-bg-hover">
                <Checkbox
                  checked={staged.has(file)}
                  onCheckedChange={() => toggleStaged(file)}
                  className="w-3.5 h-3.5"
                />
                <FileText className="w-3 h-3 text-text-faint shrink-0" />
                <button
                  type="button"
                  className="text-xs font-mono text-text-muted truncate text-left hover:text-text-normal"
                  onClick={() => onOpenFile?.(file)}
                >
                  {file}
                </button>
              </div>
            ))}
          </div>
        ) : (
          <FileTree
            files={files}
            staged={staged}
            onToggleStaged={toggleStaged}
            onOpenFile={(f) => onOpenFile?.(f)}
          />
        )}
      </ScrollArea>

      <div className="flex items-center justify-end px-4 py-2 border-t border-bg-border">
        <Button
          size="sm"
          className="h-7 px-3 text-xs bg-accent text-white hover:bg-accent-hover"
          disabled={staged.size === 0 || submitting}
          onClick={handleSubmit}
        >
          <Send className="w-3 h-3 mr-1" />
          {submitting ? "Submitting..." : "Submit to Claude"}
        </Button>
      </div>
    </div>
  );
}
