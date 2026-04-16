import { ChevronDown, ChevronRight, Folder, Send } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { SandboxGroupHeader } from "@/components/sidebar/SandboxGroupHeader";
import { SandboxRow } from "@/components/sidebar/SandboxRow";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useBackend } from "@/hooks/useBackend";
import { buildFileTree, type FileTreeNode } from "@/lib/buildFileTree";
import type { ViewMode } from "@/types";

interface SandboxSectionProps {
  /**
   * Bumped by the parent whenever the sandbox list should be refetched
   * (e.g. after a successful inline-comment submit or a sidepanel refresh
   * button click). The value itself is opaque — any change triggers a
   * refetch.
   */
  refreshKey?: number;
  /** View mode owned by RemarginSidebar (persisted in plugin settings). */
  viewMode?: ViewMode;
  /**
   * Called to open a staged file in the editor when the user clicks on it.
   * Receives the path exactly as the CLI reported it.
   */
  onOpenFile?: (path: string) => void;
  /**
   * Forwarded Submit handler. The parent is responsible for the actual
   * Claude work; this component only tracks which files are staged and
   * clears them from the sandbox via `remargin sandbox remove` after the
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
 * Sidebar section that lists files the current identity has touched in its
 * remargin sandbox. Renders a VSCode-style two-group layout: **Staged**
 * (files checked for submit, with Submit inside the group) and **Unstaged**
 * (touched files not yet staged, with per-row trash + bulk stage actions).
 *
 * The authoritative list of touched files comes from `remargin sandbox list
 * --json`. Which of those are "staged" (vs "unstaged") is local state:
 * unchecking a file excludes it from the next Submit but does not mutate
 * the persistent sandbox. Only a successful Submit clears the submitted
 * rows (via `remargin sandbox remove`), and the per-row trash on unstaged
 * rows does an explicit `sandbox remove` to forget the file entirely.
 */
export function SandboxSection({
  refreshKey,
  viewMode = "flat",
  onOpenFile,
  onSubmit,
}: SandboxSectionProps) {
  const backend = useBackend();
  const [files, setFiles] = useState<string[]>([]);
  const [staged, setStaged] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [stagedOpen, setStagedOpen] = useState(true);
  const [unstagedOpen, setUnstagedOpen] = useState(true);
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
        // Keep any previously-staged files that are still present; newly-
        // added files default to staged so the user does not have to re-
        // stage them after adding with `+`.
        setStaged((prev) => {
          const next = new Set<string>();
          for (const path of paths) {
            if (prev.size === 0 || prev.has(path)) {
              next.add(path);
            }
          }
          // If nothing carried over (e.g. all previous entries were cleared),
          // default to all-staged.
          if (next.size === 0) {
            for (const path of paths) next.add(path);
          }
          return next;
        });
        // Drop any selection entries that no longer correspond to real files.
        setSelected((prev) => {
          const next = new Set<string>();
          for (const path of paths) {
            if (prev.has(path)) next.add(path);
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

  const stagedFiles = useMemo(() => files.filter((f) => staged.has(f)), [files, staged]);
  const unstagedFiles = useMemo(() => files.filter((f) => !staged.has(f)), [files, staged]);

  const toggleSelected = useCallback((path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  /** Staged bulk `check-check`: toggle select-all across staged rows. */
  const toggleSelectAllStaged = useCallback(() => {
    setSelected((prev) => {
      // If every staged row is already selected, clear the staged selection.
      // Otherwise, add every staged row to the selection.
      const allSelected = stagedFiles.length > 0 && stagedFiles.every((f) => prev.has(f));
      const next = new Set(prev);
      if (allSelected) {
        for (const f of stagedFiles) next.delete(f);
      } else {
        for (const f of stagedFiles) next.add(f);
      }
      return next;
    });
  }, [stagedFiles]);

  /**
   * Staged bulk `minus`: unstage the selected staged files, or all staged
   * files when the staged selection is empty.
   */
  const unstageBulk = useCallback(() => {
    if (stagedFiles.length === 0) return;
    const selectedStaged = stagedFiles.filter((f) => selected.has(f));
    const targets = selectedStaged.length > 0 ? selectedStaged : stagedFiles;
    setStaged((prev) => {
      const next = new Set(prev);
      for (const t of targets) next.delete(t);
      return next;
    });
    setSelected((prev) => {
      const next = new Set(prev);
      for (const t of targets) next.delete(t);
      return next;
    });
  }, [stagedFiles, selected]);

  /**
   * Unstaged bulk `plus`: stage selected unstaged files, or every unstaged
   * file when the unstaged selection is empty.
   */
  const stageBulk = useCallback(() => {
    if (unstagedFiles.length === 0) return;
    const selectedUnstaged = unstagedFiles.filter((f) => selected.has(f));
    const targets = selectedUnstaged.length > 0 ? selectedUnstaged : unstagedFiles;
    setStaged((prev) => {
      const next = new Set(prev);
      for (const t of targets) next.add(t);
      return next;
    });
    setSelected((prev) => {
      const next = new Set(prev);
      for (const t of targets) next.delete(t);
      return next;
    });
  }, [unstagedFiles, selected]);

  /** Unstaged bulk `check-check`: stage every unstaged file unconditionally. */
  const stageAllUnstaged = useCallback(() => {
    if (unstagedFiles.length === 0) return;
    setStaged((prev) => {
      const next = new Set(prev);
      for (const f of unstagedFiles) next.add(f);
      return next;
    });
    setSelected((prev) => {
      const next = new Set(prev);
      for (const f of unstagedFiles) next.delete(f);
      return next;
    });
  }, [unstagedFiles]);

  const handleRemove = useCallback(
    async (path: string) => {
      try {
        await backend.sandboxRemove([path]);
        await refresh();
      } catch (err) {
        console.error("SandboxSection.handleRemove failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend, refresh]
  );

  const handleSubmit = useCallback(async () => {
    if (submitting) return;
    const toSubmit = stagedFiles;
    if (toSubmit.length === 0) return;
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit?.(toSubmit);
      // Submit succeeded — clear the submitted files from the persistent
      // sandbox and refetch. We only call `sandbox remove` for files the
      // user actually submitted; anything they unstaged stays as a touched
      // (unstaged) file.
      try {
        await backend.sandboxRemove(toSubmit);
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
  }, [backend, stagedFiles, onSubmit, refresh, submitting]);

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
    return <div className="px-4 py-3 text-xs text-text-faint">No touched files</div>;
  }

  return (
    <div className="flex flex-col min-w-0">
      {error && (
        <div className="px-4 py-2 text-[10px] text-red-400 font-mono whitespace-pre-wrap break-words">
          {error}
        </div>
      )}

      <ScrollArea className="max-h-64">
        <SandboxGroupHeader
          label="Staged"
          count={stagedFiles.length}
          open={stagedOpen}
          onToggleOpen={() => setStagedOpen((v) => !v)}
          leftBulkIcon="check-check"
          leftBulkTitle="Toggle select all staged"
          onLeftBulk={toggleSelectAllStaged}
          rightBulkIcon="minus"
          rightBulkTitle="Unstage selected (or all)"
          onRightBulk={unstageBulk}
          disabled={stagedFiles.length === 0}
        />
        {stagedOpen && viewMode === "flat"
          ? stagedFiles.map((file) => (
              <SandboxRow
                key={`s:${file}`}
                path={file}
                variant="staged"
                selected={selected.has(file)}
                onToggleSelected={toggleSelected}
                onOpenFile={(p) => onOpenFile?.(p)}
              />
            ))
          : stagedOpen && (
              <SandboxTreeGroup
                files={stagedFiles}
                variant="staged"
                selected={selected}
                onToggleSelected={toggleSelected}
                onOpenFile={(p) => onOpenFile?.(p)}
              />
            )}
        {stagedOpen && (
          <div className="flex items-center justify-end px-4 py-2">
            <Button
              size="sm"
              className="h-7 px-3 text-xs bg-accent text-white hover:bg-accent-hover"
              disabled={stagedFiles.length === 0 || submitting}
              onClick={handleSubmit}
            >
              <Send className="w-3 h-3 mr-1" />
              {submitting ? "Submitting..." : "Submit"}
            </Button>
          </div>
        )}

        <SandboxGroupHeader
          label="Unstaged"
          count={unstagedFiles.length}
          open={unstagedOpen}
          onToggleOpen={() => setUnstagedOpen((v) => !v)}
          leftBulkIcon="plus"
          leftBulkTitle="Stage selected (or all)"
          onLeftBulk={stageBulk}
          rightBulkIcon="check-check"
          rightBulkTitle="Stage everything unstaged"
          onRightBulk={stageAllUnstaged}
          disabled={unstagedFiles.length === 0}
        />
        {unstagedOpen && viewMode === "flat"
          ? unstagedFiles.map((file) => (
              <SandboxRow
                key={`u:${file}`}
                path={file}
                variant="unstaged"
                selected={selected.has(file)}
                onToggleSelected={toggleSelected}
                onOpenFile={(p) => onOpenFile?.(p)}
                onRemoveFile={handleRemove}
              />
            ))
          : unstagedOpen && (
              <SandboxTreeGroup
                files={unstagedFiles}
                variant="unstaged"
                selected={selected}
                onToggleSelected={toggleSelected}
                onOpenFile={(p) => onOpenFile?.(p)}
                onRemoveFile={handleRemove}
              />
            )}
      </ScrollArea>
    </div>
  );
}

interface SandboxTreeGroupProps {
  files: string[];
  variant: "staged" | "unstaged";
  selected: Set<string>;
  onToggleSelected: (path: string) => void;
  onOpenFile: (path: string) => void;
  onRemoveFile?: (path: string) => void;
}

/**
 * Tree-view renderer for a Sandbox sub-group. Groups files by directory
 * with collapsible folder nodes; file leaves reuse the same row
 * affordances as the flat view (checkbox + trash for unstaged, path-only
 * for staged), indented per depth.
 */
function SandboxTreeGroup(props: SandboxTreeGroupProps) {
  const tree = useMemo(() => buildFileTree(props.files), [props.files]);
  return (
    <>
      {tree.map((node) => (
        <SandboxTreeNode key={node.fullPath} node={node} depth={0} {...props} />
      ))}
    </>
  );
}

interface SandboxTreeNodeProps {
  node: FileTreeNode;
  depth: number;
  variant: "staged" | "unstaged";
  selected: Set<string>;
  onToggleSelected: (path: string) => void;
  onOpenFile: (path: string) => void;
  onRemoveFile?: (path: string) => void;
}

function SandboxTreeNode({
  node,
  depth,
  variant,
  selected,
  onToggleSelected,
  onOpenFile,
  onRemoveFile,
}: SandboxTreeNodeProps) {
  const [expanded, setExpanded] = useState(true);
  if (!node.isDir) {
    return (
      <SandboxRow
        path={node.fullPath}
        depth={depth}
        variant={variant}
        selected={selected.has(node.fullPath)}
        onToggleSelected={onToggleSelected}
        onOpenFile={onOpenFile}
        onRemoveFile={onRemoveFile}
      />
    );
  }
  const Chevron = expanded ? ChevronDown : ChevronRight;
  return (
    <>
      <div
        className="group flex items-center gap-1.5 py-1 pr-4 hover:bg-bg-hover cursor-pointer"
        style={{ paddingLeft: `${24 + depth * 16}px` }}
        onClick={() => setExpanded((v) => !v)}
      >
        <Chevron className="w-3 h-3 text-text-faint shrink-0" />
        <Folder className="w-3 h-3 text-text-faint shrink-0" />
        <span className="flex-1 text-xs font-mono text-text-muted truncate">{node.name}</span>
      </div>
      {expanded &&
        node.children.map((child) => (
          <SandboxTreeNode
            key={child.fullPath}
            node={child}
            depth={depth + 1}
            variant={variant}
            selected={selected}
            onToggleSelected={onToggleSelected}
            onOpenFile={onOpenFile}
            onRemoveFile={onRemoveFile}
          />
        ))}
    </>
  );
}
