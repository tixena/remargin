import {
  Check,
  ChevronDown,
  ChevronRight,
  CircleDashed,
  Folder,
  Loader2,
  Send,
  Sparkles,
  TriangleAlert,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ResolvedSystemPrompt } from "@/backend/types";
import {
  buildPromptGroups,
  DEFAULT_GROUP_KEY,
  type PromptGroup,
  type StagedGroup,
  type SubmitGroupResult,
  type SubmitProgress,
} from "@/components/sidebar/buildPromptGroups";
import {
  InlinePromptEditor,
  type InlinePromptEditorSaveArgs,
} from "@/components/sidebar/InlinePromptEditor";
import { SandboxGroupHeader } from "@/components/sidebar/SandboxGroupHeader";
import { SandboxRow } from "@/components/sidebar/SandboxRow";
import { Button } from "@/components/ui/button";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { useBackend } from "@/hooks/useBackend";
import { buildFileTree, type FileTreeNode } from "@/lib/buildFileTree";
import type { ViewMode } from "@/types";

export type { PromptGroup, StagedGroup };

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
   * Forwarded Submit handler. Receives the staged files grouped by
   * resolved system prompt + a progress hook the parent can call as
   * each group starts and completes. Returns per-group results so this
   * section can render success/failure badges.
   *
   * The handler now owns the per-group cleanup (`sandboxRemove`) so the
   * sandbox section stays purely structural.
   */
  onSubmit?: (
    groups: StagedGroup[],
    progress?: SubmitProgress
  ) => Promise<SubmitGroupResult[] | void> | void;
  /**
   * Persist a `system_prompt:` block to the owning `.remargin.yaml`.
   * Receives the target path, name, and body. Returning resolves the
   * inline editor; rejecting surfaces the error in the editor footer
   * and keeps the buffer.
   */
  onSavePrompt?: (args: InlinePromptEditorSaveArgs) => Promise<void>;
  /**
   * Strip the `system_prompt:` block from the owning `.remargin.yaml`.
   * Resolves when the write succeeds; rejecting surfaces the error
   * inline.
   */
  onDeletePrompt?: (source: string) => Promise<void>;
  /**
   * Tooltip / disabled-reason for the Save button (e.g. strict mode
   * without a key). When set, the button is disabled.
   */
  savePromptDisabledReason?: string;
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
 * remargin sandbox. Renders a by-prompt outer grouping (one section per
 * resolved system prompt; Default group last) wrapping the original
 * Staged/Unstaged structure from task 24. Single Submit-all button at the
 * bottom; the per-group submit was retired by `r8w`/`uhs`.
 */
export function SandboxSection({
  refreshKey,
  viewMode = "flat",
  onOpenFile,
  onSubmit,
  onSavePrompt,
  onDeletePrompt,
  savePromptDisabledReason,
}: SandboxSectionProps) {
  const backend = useBackend();
  const [files, setFiles] = useState<string[]>([]);
  const [prompts, setPrompts] = useState<Map<string, ResolvedSystemPrompt>>(new Map());
  const [resolveErrors, setResolveErrors] = useState<Map<string, string>>(new Map());
  const [staged, setStaged] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<Set<string>>(new Set());
  // Per-group open state. Keys are PromptGroup.source ?? DEFAULT_GROUP_KEY.
  const [stagedOpenByGroup, setStagedOpenByGroup] = useState<Map<string, boolean>>(new Map());
  const [unstagedOpenByGroup, setUnstagedOpenByGroup] = useState<Map<string, boolean>>(new Map());
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
        setStaged((prev) => {
          const next = new Set<string>();
          for (const path of paths) {
            if (prev.size === 0 || prev.has(path)) {
              next.add(path);
            }
          }
          if (next.size === 0) {
            for (const path of paths) next.add(path);
          }
          return next;
        });
        setSelected((prev) => {
          const next = new Set<string>();
          for (const path of paths) {
            if (prev.has(path)) next.add(path);
          }
          return next;
        });

        // Resolve prompts in parallel; capture per-file errors so a
        // single bad walk doesn't black-hole the whole sidebar.
        const nextPrompts = new Map<string, ResolvedSystemPrompt>();
        const nextErrors = new Map<string, string>();
        await Promise.all(
          paths.map(async (p) => {
            try {
              const resolved = await backend.resolvePrompt(p);
              nextPrompts.set(p, resolved);
            } catch (err) {
              nextErrors.set(p, errorMessage(err));
            }
          })
        );
        setPrompts(nextPrompts);
        setResolveErrors(nextErrors);
        setError(null);
      } catch (err) {
        console.error("SandboxSection.refresh failed:", err);
        setFiles([]);
        setPrompts(new Map());
        setResolveErrors(new Map());
        setError(errorMessage(err));
      } finally {
        setLoading(false);
      }
    },
    [backend]
  );

  useEffect(() => {
    void refresh(refreshKey);
  }, [refresh, refreshKey]);

  const groups = useMemo<PromptGroup[]>(
    () => buildPromptGroups(files, prompts, resolveErrors, staged),
    [files, prompts, resolveErrors, staged]
  );

  const toggleSelected = useCallback((path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const setStagedOpen = useCallback((key: string, open: boolean) => {
    setStagedOpenByGroup((prev) => {
      const next = new Map(prev);
      next.set(key, open);
      return next;
    });
  }, []);

  const setUnstagedOpen = useCallback((key: string, open: boolean) => {
    setUnstagedOpenByGroup((prev) => {
      const next = new Map(prev);
      next.set(key, open);
      return next;
    });
  }, []);

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

  // Per-group submit status. Keys are the group's source ?? DEFAULT_GROUP_KEY.
  // Each group's status is cleared when its own Submit is fired again.
  const [groupStatus, setGroupStatus] = useState<Map<string, "pending" | "ok" | "failed">>(
    new Map()
  );
  const [groupErrors, setGroupErrors] = useState<Map<string, string>>(new Map());

  const handleSubmitGroup = useCallback(
    async (group: PromptGroup) => {
      if (submitting) return;
      if (group.hasError) return;
      const stagedFiles = group.files.filter((f) => staged.has(f));
      if (stagedFiles.length === 0) return;
      const key = group.source ?? DEFAULT_GROUP_KEY;
      const payload: StagedGroup[] = [{ prompt: group.prompt, files: stagedFiles }];
      setSubmitting(true);
      setError(null);
      setGroupStatus((prev) => {
        const next = new Map(prev);
        next.delete(key);
        return next;
      });
      setGroupErrors((prev) => {
        const next = new Map(prev);
        next.delete(key);
        return next;
      });

      const progress: SubmitProgress = {
        onGroupStart: (g) => {
          const k = g.prompt.source ?? DEFAULT_GROUP_KEY;
          setGroupStatus((prev) => new Map(prev).set(k, "pending"));
        },
        onGroupComplete: (g, result) => {
          const k = g.prompt.source ?? DEFAULT_GROUP_KEY;
          setGroupStatus((prev) => new Map(prev).set(k, result.ok ? "ok" : "failed"));
          if (result.error) {
            setGroupErrors((prev) => new Map(prev).set(k, result.error ?? "submit failed"));
          }
        },
      };

      try {
        await onSubmit?.(payload, progress);
      } catch (err) {
        console.error("SandboxSection.onSubmit failed:", err);
        setError(errorMessage(err));
      } finally {
        setSubmitting(false);
      }
    },
    [submitting, staged, onSubmit]
  );

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

      <div className="flex flex-col min-w-0">
        {groups.map((group) => {
          const key = group.source ?? DEFAULT_GROUP_KEY;
          return (
            <PromptGroupSection
              key={key}
              group={group}
              viewMode={viewMode}
              selected={selected}
              stagedOpen={stagedOpenByGroup.get(key) ?? true}
              unstagedOpen={unstagedOpenByGroup.get(key) ?? true}
              status={groupStatus.get(key)}
              statusError={groupErrors.get(key)}
              onToggleStagedOpen={() => setStagedOpen(key, !(stagedOpenByGroup.get(key) ?? true))}
              onToggleUnstagedOpen={() =>
                setUnstagedOpen(key, !(unstagedOpenByGroup.get(key) ?? true))
              }
              onToggleSelected={toggleSelected}
              onStageBulk={(files) => stageFiles(files, setStaged, setSelected)}
              onUnstageBulk={(files) => unstageFiles(files, setStaged, setSelected)}
              onSelectAll={(files) => toggleSelectAll(files, selected, setSelected)}
              onRemoveFile={handleRemove}
              onOpenFile={(p) => onOpenFile?.(p)}
              onSavePrompt={onSavePrompt}
              onDeletePrompt={onDeletePrompt}
              savePromptDisabledReason={savePromptDisabledReason}
              onSubmitGroup={handleSubmitGroup}
              submitting={submitting}
            />
          );
        })}
      </div>
    </div>
  );
}

function stageFiles(
  files: string[],
  setStaged: React.Dispatch<React.SetStateAction<Set<string>>>,
  setSelected: React.Dispatch<React.SetStateAction<Set<string>>>
) {
  if (files.length === 0) return;
  setStaged((prev) => {
    const next = new Set(prev);
    for (const f of files) next.add(f);
    return next;
  });
  setSelected((prev) => {
    const next = new Set(prev);
    for (const f of files) next.delete(f);
    return next;
  });
}

function unstageFiles(
  files: string[],
  setStaged: React.Dispatch<React.SetStateAction<Set<string>>>,
  setSelected: React.Dispatch<React.SetStateAction<Set<string>>>
) {
  if (files.length === 0) return;
  setStaged((prev) => {
    const next = new Set(prev);
    for (const f of files) next.delete(f);
    return next;
  });
  setSelected((prev) => {
    const next = new Set(prev);
    for (const f of files) next.delete(f);
    return next;
  });
}

function toggleSelectAll(
  files: string[],
  selected: Set<string>,
  setSelected: React.Dispatch<React.SetStateAction<Set<string>>>
) {
  if (files.length === 0) return;
  const allSelected = files.every((f) => selected.has(f));
  setSelected((prev) => {
    const next = new Set(prev);
    if (allSelected) {
      for (const f of files) next.delete(f);
    } else {
      for (const f of files) next.add(f);
    }
    return next;
  });
}

export interface PromptGroupSectionProps {
  group: PromptGroup;
  viewMode: ViewMode;
  selected: Set<string>;
  stagedOpen: boolean;
  unstagedOpen: boolean;
  /**
   * Submit-all status for this group. `pending` while `claude -p` is
   * in flight, `ok` after a successful invocation + cleanup, `failed`
   * when the run rejected. `undefined` between runs.
   */
  status?: "pending" | "ok" | "failed";
  /** Error message for the failed status, surfaced as a tooltip. */
  statusError?: string;
  onToggleStagedOpen: () => void;
  onToggleUnstagedOpen: () => void;
  onToggleSelected: (path: string) => void;
  onStageBulk: (files: string[]) => void;
  onUnstageBulk: (files: string[]) => void;
  onSelectAll: (files: string[]) => void;
  onRemoveFile: (path: string) => void;
  onOpenFile: (path: string) => void;
  onSavePrompt?: (args: InlinePromptEditorSaveArgs) => Promise<void>;
  onDeletePrompt?: (source: string) => Promise<void>;
  savePromptDisabledReason?: string;
  /** Per-group Submit. Renders inside the Staged sub-section. */
  onSubmitGroup?: (group: PromptGroup) => void | Promise<void>;
  /** True while any group's Submit is in flight; disables every group's Submit. */
  submitting?: boolean;
}

// Exported for component-test isolation; internal in production use.
export function PromptGroupSection({
  group,
  viewMode,
  selected,
  stagedOpen,
  unstagedOpen,
  status,
  statusError,
  onToggleStagedOpen,
  onToggleUnstagedOpen,
  onToggleSelected,
  onStageBulk,
  onUnstageBulk,
  onSelectAll,
  onRemoveFile,
  onOpenFile,
  onSavePrompt,
  onDeletePrompt,
  savePromptDisabledReason,
  onSubmitGroup,
  submitting,
}: PromptGroupSectionProps) {
  const [headerOpen, setHeaderOpen] = useState(true);
  const [editing, setEditing] = useState(false);
  const HeaderChevron = headerOpen ? ChevronDown : ChevronRight;
  const PromptIcon = group.isDefault ? CircleDashed : Sparkles;

  const unstageSelectedOrAll = useCallback(() => {
    const targets = group.staged.filter((f) => selected.has(f));
    onUnstageBulk(targets.length > 0 ? targets : group.staged);
  }, [group.staged, selected, onUnstageBulk]);

  const stageSelectedOrAll = useCallback(() => {
    const targets = group.unstaged.filter((f) => selected.has(f));
    onStageBulk(targets.length > 0 ? targets : group.unstaged);
  }, [group.unstaged, selected, onStageBulk]);

  // Derive a folder hint for the create flow on the Default group.
  // Falls back to the vault root when no Staged file is around to
  // anchor a target folder.
  const folderHint = useCallback((): string => {
    const sample = group.staged[0] ?? group.files[0];
    if (sample) {
      const idx = Math.max(sample.lastIndexOf("/"), sample.lastIndexOf("\\"));
      if (idx > 0) return sample.slice(0, idx);
    }
    return ".";
  }, [group.staged, group.files]);

  const handleSave = useCallback(
    async (args: InlinePromptEditorSaveArgs) => {
      if (!onSavePrompt) return;
      await onSavePrompt(args);
      setEditing(false);
    },
    [onSavePrompt]
  );

  const handleDelete = useCallback(
    async (source: string) => {
      if (!onDeletePrompt) return;
      await onDeletePrompt(source);
      setEditing(false);
    },
    [onDeletePrompt]
  );

  return (
    <div className="flex flex-col">
      <div
        className="flex items-center justify-between px-3 py-2 cursor-pointer select-none hover:bg-bg-hover"
        onClick={() => setHeaderOpen((v) => !v)}
        title={group.hasError ? group.errorMessage : undefined}
      >
        <div className="flex items-center gap-1.5 min-w-0">
          <HeaderChevron className="w-3 h-3 text-text-faint shrink-0" />
          <PromptIcon className="w-3 h-3 text-text-faint shrink-0" />
          <span className="text-xs font-semibold text-text-normal truncate">{group.name}</span>
          <span className="text-[10px] text-text-faint truncate">{group.scope}</span>
          <span className="inline-flex items-center justify-center min-w-4 h-4 px-1.5 text-[9px] text-text-muted bg-bg-border rounded-full">
            {group.files.length}
          </span>
          {status === "pending" && (
            <Loader2
              className="w-3 h-3 text-text-faint shrink-0 animate-spin"
              aria-label="Submitting"
            />
          )}
          {status === "ok" && (
            <Check className="w-3 h-3 text-green-500 shrink-0" aria-label="Submitted" />
          )}
          {status === "failed" && (
            <span title={statusError} className="inline-flex shrink-0">
              <TriangleAlert className="w-3 h-3 text-red-400 shrink-0" aria-label="Submit failed" />
            </span>
          )}
        </div>
        <div className="flex items-center gap-0.5">
          {group.isDefault && !group.hasError && !editing && (
            <button
              type="button"
              className="text-[10px] text-text-faint hover:text-text-normal px-1 py-0.5"
              title="Configure prompt"
              onClick={(e) => {
                e.stopPropagation();
                setEditing(true);
                setHeaderOpen(true);
              }}
            >
              + Configure
            </button>
          )}
          {!group.hasError && (
            <button
              type="button"
              className="flex items-center justify-center w-5 h-5 rounded-sm text-text-faint hover:text-text-normal hover:bg-bg-border"
              title={editing ? "Close editor" : "Edit prompt"}
              onClick={(e) => {
                e.stopPropagation();
                setEditing((v) => !v);
                setHeaderOpen(true);
              }}
            >
              <ObsidianIcon icon="settings" size={12} />
            </button>
          )}
        </div>
      </div>

      {headerOpen && editing && onSavePrompt && (
        <InlinePromptEditor
          source={group.isDefault ? null : group.source}
          folder={group.isDefault ? folderHint() : group.source ? group.scope : folderHint()}
          initialName={group.isDefault ? "" : group.name}
          initialBody={group.isDefault ? "" : group.prompt.prompt}
          onSave={handleSave}
          onDelete={!group.isDefault && onDeletePrompt ? handleDelete : undefined}
          onCancel={() => setEditing(false)}
          saveDisabledReason={savePromptDisabledReason}
        />
      )}

      {headerOpen && (
        <>
          <SandboxGroupHeader
            label="Staged"
            count={group.staged.length}
            open={stagedOpen}
            onToggleOpen={onToggleStagedOpen}
            leftBulkIcon="check-check"
            leftBulkTitle="Toggle select all staged"
            onLeftBulk={() => onSelectAll(group.staged)}
            rightBulkIcon="minus"
            rightBulkTitle="Unstage selected (or all)"
            onRightBulk={unstageSelectedOrAll}
            disabled={group.staged.length === 0 || group.hasError}
          />
          {stagedOpen && viewMode === "flat"
            ? group.staged.map((file) => (
                <SandboxRow
                  key={`s:${file}`}
                  path={file}
                  variant="staged"
                  selected={selected.has(file)}
                  onToggleSelected={onToggleSelected}
                  onOpenFile={onOpenFile}
                />
              ))
            : stagedOpen && (
                <SandboxTreeGroup
                  files={group.staged}
                  variant="staged"
                  selected={selected}
                  onToggleSelected={onToggleSelected}
                  onOpenFile={onOpenFile}
                />
              )}
          {stagedOpen && onSubmitGroup && (
            <div className="flex items-center justify-end px-4 py-2">
              <Button
                size="sm"
                className="h-7 px-3 text-xs bg-accent text-white hover:bg-accent-hover"
                disabled={group.staged.length === 0 || group.hasError || !!submitting}
                onClick={() => void onSubmitGroup(group)}
              >
                <Send className="w-3 h-3 mr-1" />
                {submitting ? "Submitting..." : `Submit (${group.staged.length})`}
              </Button>
            </div>
          )}

          <SandboxGroupHeader
            label="Unstaged"
            count={group.unstaged.length}
            open={unstagedOpen}
            onToggleOpen={onToggleUnstagedOpen}
            leftBulkIcon="plus"
            leftBulkTitle="Stage selected (or all)"
            onLeftBulk={stageSelectedOrAll}
            rightBulkIcon="check-check"
            rightBulkTitle="Stage everything unstaged"
            onRightBulk={() => onStageBulk(group.unstaged)}
            disabled={group.unstaged.length === 0 || group.hasError}
          />
          {unstagedOpen && viewMode === "flat"
            ? group.unstaged.map((file) => (
                <SandboxRow
                  key={`u:${file}`}
                  path={file}
                  variant="unstaged"
                  selected={selected.has(file)}
                  onToggleSelected={onToggleSelected}
                  onOpenFile={onOpenFile}
                  onRemoveFile={onRemoveFile}
                />
              ))
            : unstagedOpen && (
                <SandboxTreeGroup
                  files={group.unstaged}
                  variant="unstaged"
                  selected={selected}
                  onToggleSelected={onToggleSelected}
                  onOpenFile={onOpenFile}
                  onRemoveFile={onRemoveFile}
                />
              )}
        </>
      )}
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
