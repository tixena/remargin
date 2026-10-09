/** The sidebar's Sandbox section: staged files grouped by their resolved system prompt. */

import { ChevronDown, ChevronRight, CircleDashed, Folder, Send, Sparkles } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ResolvedSystemPrompt } from "@/backend/types";
import {
  buildPromptGroups,
  DEFAULT_GROUP_KEY,
  type PromptGroup,
  type StagedGroup,
} from "@/components/sidebar/buildPromptGroups";
import {
  InlinePromptEditor,
  type InlinePromptEditorSaveArgs,
} from "@/components/sidebar/InlinePromptEditor";
import { readToken } from "@/components/sidebar/readToken";
import { SandboxGroupHeader } from "@/components/sidebar/SandboxGroupHeader";
import { SandboxRow } from "@/components/sidebar/SandboxRow";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { useBackend } from "@/hooks/useBackend";
import { buildFileTree, type FileTreeNode } from "@/lib/buildFileTree";
import type { ViewMode } from "@/types";

export type { PromptGroup, StagedGroup };

/** Props for {@link SandboxSection}. */
interface SandboxSectionProps {
  /** Opaque: any change triggers a refetch of the sandbox list. */
  refreshKey?: number;
  viewMode?: ViewMode;
  onOpenFile?: (path: string) => void;
  /** Receives the staged files grouped by resolved system prompt; fire-and-forget. */
  onSubmit?: (groups: StagedGroup[]) => Promise<void> | void;
  /** Resolving closes the inline editor; rejecting shows the error there and keeps the buffer. */
  onSavePrompt?: (args: InlinePromptEditorSaveArgs) => Promise<void>;
  onDeletePrompt?: (source: string) => Promise<void>;
  /** When set, Save is disabled and this is its tooltip. */
  savePromptDisabledReason?: string;
  availableFolders?: string[];
  /** When set, per-prompt headers strip it from `group.scope` to show a vault-relative path. */
  vaultRoot?: string;
}

// WHY: the backend returns absolute host paths in resolved.source; the
// vault-relative form is what the user actually recognises in the UI.
function formatScopeRelative(scope: string, vaultRoot?: string): string {
  if (scope === "(vault)") return "./";
  if (scope === "(unknown)" || scope === "resolve failed") return scope;
  let rel = scope;
  if (vaultRoot && (rel === vaultRoot || rel.startsWith(`${vaultRoot}/`))) {
    rel = rel.length === vaultRoot.length ? "" : rel.slice(vaultRoot.length + 1);
  }
  if (rel.length === 0) return "./";
  if (rel.startsWith("/")) return rel;
  return `./${rel}/`;
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

export function SandboxSection({
  refreshKey,
  viewMode = "flat",
  onOpenFile,
  onSubmit,
  onSavePrompt,
  onDeletePrompt,
  savePromptDisabledReason,
  availableFolders,
  vaultRoot,
}: SandboxSectionProps) {
  const backend = useBackend();
  const [files, setFiles] = useState<string[]>([]);
  const [prompts, setPrompts] = useState<Map<string, ResolvedSystemPrompt>>(new Map());
  const [resolveErrors, setResolveErrors] = useState<Map<string, string>>(new Map());
  const [staged, setStaged] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [stagedOpenByGroup, setStagedOpenByGroup] = useState<Map<string, boolean>>(new Map());
  const [unstagedOpenByGroup, setUnstagedOpenByGroup] = useState<Map<string, boolean>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // A removal re-lists through the same effect by advancing a counter of its own.
  const [localRefresh, setLocalRefresh] = useState(0);
  // Token of the newest listing: a slow response that a newer listing superseded is dropped.
  const newestListing = useRef<string | null>(null);

  const refresh = useCallback(
    async (token: string) => {
      newestListing.current = token;
      setLoading(true);
      try {
        const entries = await backend.sandboxList();
        if (newestListing.current !== token) return;
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

        // Per-file errors are captured so one bad walk does not blank the whole section.
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
        if (newestListing.current !== token) return;
        setPrompts(nextPrompts);
        setResolveErrors(nextErrors);
        setError(null);
      } catch (err) {
        console.error("SandboxSection.refresh failed:", err);
        if (newestListing.current !== token) return;
        setFiles([]);
        setPrompts(new Map());
        setResolveErrors(new Map());
        setError(errorMessage(err));
      } finally {
        // A superseded run leaves the loading state to the listing that replaced it.
        if (newestListing.current === token) setLoading(false);
      }
    },
    [backend]
  );

  useEffect(() => {
    void refresh(readToken(refreshKey ?? 0, localRefresh));
  }, [refresh, refreshKey, localRefresh]);

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
        setLocalRefresh((n) => n + 1);
      } catch (err) {
        console.error("SandboxSection.handleRemove failed:", err);
        setError(errorMessage(err));
      }
    },
    [backend]
  );

  const handleSubmitGroup = useCallback(
    async (group: PromptGroup) => {
      if (submitting) return;
      if (group.hasError) return;
      const stagedFiles = group.files.filter((f) => staged.has(f));
      if (stagedFiles.length === 0) return;
      setSubmitting(true);
      setError(null);
      try {
        await onSubmit?.([{ prompt: group.prompt, files: stagedFiles }]);
      } catch (err) {
        console.error("SandboxSection.onSubmit failed:", err);
        setError(errorMessage(err));
      } finally {
        setSubmitting(false);
      }
    },
    [submitting, staged, onSubmit]
  );

  const eligibleSubmitGroups = useMemo(
    () => groups.filter((g) => !g.hasError && g.staged.length > 0),
    [groups]
  );
  const submitAllCount = useMemo(
    () => eligibleSubmitGroups.reduce((acc, g) => acc + g.staged.length, 0),
    [eligibleSubmitGroups]
  );

  const handleSubmitAll = useCallback(async () => {
    if (submitting || eligibleSubmitGroups.length === 0) return;
    setSubmitting(true);
    setError(null);
    const payload: StagedGroup[] = eligibleSubmitGroups.map((g) => ({
      prompt: g.prompt,
      files: g.staged,
    }));
    try {
      await onSubmit?.(payload);
    } catch (err) {
      console.error("SandboxSection.handleSubmitAll failed:", err);
      setError(errorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }, [submitting, eligibleSubmitGroups, onSubmit]);

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
    <div className="remargin-sandbox">
      {error && <div className="remargin-sandbox__error">{error}</div>}

      <div className="rmg-l1-body">
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
              availableFolders={availableFolders}
              vaultRoot={vaultRoot}
              onSubmitGroup={handleSubmitGroup}
              submitting={submitting}
            />
          );
        })}

        {submitAllCount > 0 && (
          <div className="rmg-l1__action">
            <button
              type="button"
              className="rmg-btn-submit rmg-btn-submit--primary"
              disabled={!!submitting}
              onClick={() => void handleSubmitAll()}
            >
              <Send />
              <span>{submitting ? "Submitting…" : "Submit all"}</span>
              {!submitting && <span className="rmg-btn-submit__num">{submitAllCount}</span>}
            </button>
          </div>
        )}
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

/** Props for {@link PromptGroupSection}. */
export interface PromptGroupSectionProps {
  group: PromptGroup;
  viewMode: ViewMode;
  selected: Set<string>;
  stagedOpen: boolean;
  unstagedOpen: boolean;
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
  availableFolders?: string[];
  vaultRoot?: string;
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
  availableFolders,
  vaultRoot,
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

  const scopeDisplay = formatScopeRelative(group.scope, vaultRoot);

  return (
    <section className="rmg-l2" aria-label={group.name}>
      <div
        className="rmg-l2__head"
        data-open={headerOpen ? "true" : "false"}
        role="button"
        tabIndex={0}
        onClick={() => setHeaderOpen((v) => !v)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setHeaderOpen((v) => !v);
          }
        }}
        title={group.hasError ? group.errorMessage : undefined}
      >
        <HeaderChevron className="rmg-l2__chev" />
        <div className="rmg-l2__title-wrap">
          <PromptIcon className="rmg-l2__icon" />
          <span className="rmg-l2__title">{group.name}</span>
          <span className="rmg-l2__count">{group.files.length}</span>
        </div>
        <div className="rmg-l2__actions">
          {group.isDefault && !group.hasError && !editing && (
            <button
              type="button"
              className="rmg-l2__configure"
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
              className="rmg-icon-btn rmg-icon-btn--sm"
              title={editing ? "Close editor" : "Edit prompt"}
              aria-label={editing ? "Close editor" : "Edit prompt"}
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
      <span className="rmg-l2__path" title={group.scope}>
        {scopeDisplay}
      </span>

      {headerOpen && editing && onSavePrompt && (
        <InlinePromptEditor
          source={group.isDefault ? null : group.source}
          folder={group.isDefault ? folderHint() : group.source ? group.scope : folderHint()}
          initialName={group.isDefault ? "" : group.name}
          initialBody={group.isDefault ? "" : group.prompt.prompt}
          initialRunner={group.isDefault ? "" : (group.prompt.runner ?? "")}
          onSave={handleSave}
          onDelete={!group.isDefault && onDeletePrompt ? handleDelete : undefined}
          onCancel={() => setEditing(false)}
          saveDisabledReason={savePromptDisabledReason}
          availableFolders={availableFolders}
        />
      )}

      {headerOpen && (
        <>
          <section className="rmg-l3 rmg-l3--staged" aria-label="Staged files">
            <SandboxGroupHeader
              label="Staged"
              count={group.staged.length}
              open={stagedOpen}
              onToggleOpen={onToggleStagedOpen}
              leftBulkIcon="square-check"
              leftBulkTitle="Select all"
              onLeftBulk={() => onSelectAll(group.staged)}
              rightBulkIcon="arrow-down-to-line"
              rightBulkTitle="Unstage selected"
              onRightBulk={unstageSelectedOrAll}
              disabled={group.staged.length === 0 || group.hasError}
            />
            {stagedOpen && (
              <div className="rmg-l3__body">
                {viewMode === "flat" ? (
                  group.staged.map((file) => (
                    <SandboxRow
                      key={`s:${file}`}
                      path={file}
                      variant="staged"
                      selected={selected.has(file)}
                      onToggleSelected={onToggleSelected}
                      onOpenFile={onOpenFile}
                    />
                  ))
                ) : (
                  <SandboxTreeGroup
                    files={group.staged}
                    variant="staged"
                    selected={selected}
                    onToggleSelected={onToggleSelected}
                    onOpenFile={onOpenFile}
                  />
                )}
              </div>
            )}
            {stagedOpen && onSubmitGroup && (
              <div className="rmg-l3__action">
                <button
                  type="button"
                  className="rmg-btn-submit"
                  disabled={group.staged.length === 0 || group.hasError || !!submitting}
                  onClick={() => void onSubmitGroup(group)}
                >
                  <Send />
                  <span>{submitting ? "Submitting…" : "Submit"}</span>
                  {!submitting && group.staged.length > 0 && (
                    <span className="rmg-btn-submit__num">{group.staged.length}</span>
                  )}
                </button>
              </div>
            )}
          </section>

          <section className="rmg-l3 rmg-l3--unstaged" aria-label="Unstaged files">
            <SandboxGroupHeader
              label="Unstaged"
              count={group.unstaged.length}
              open={unstagedOpen}
              onToggleOpen={onToggleUnstagedOpen}
              leftBulkIcon="arrow-up-to-line"
              leftBulkTitle="Stage selected"
              onLeftBulk={stageSelectedOrAll}
              rightBulkIcon="chevrons-up"
              rightBulkTitle="Stage all unstaged"
              onRightBulk={() => onStageBulk(group.unstaged)}
              disabled={group.unstaged.length === 0 || group.hasError}
            />
            {unstagedOpen && (
              <div className="rmg-l3__body">
                {viewMode === "flat" ? (
                  group.unstaged.map((file) => (
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
                ) : (
                  <SandboxTreeGroup
                    files={group.unstaged}
                    variant="unstaged"
                    selected={selected}
                    onToggleSelected={onToggleSelected}
                    onOpenFile={onOpenFile}
                    onRemoveFile={onRemoveFile}
                  />
                )}
              </div>
            )}
          </section>
        </>
      )}
    </section>
  );
}

/** Props for {@link SandboxTreeGroup}. */
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

/** Props for {@link SandboxTreeNode}. */
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
        className="rmg-sandbox-folder"
        role="button"
        tabIndex={0}
        onClick={() => setExpanded((v) => !v)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setExpanded((v) => !v);
          }
        }}
      >
        <Chevron />
        <Folder />
        <span className="rmg-sandbox-folder__name">{node.name}</span>
      </div>
      {expanded && node.children.length > 0 && (
        <div className="rmg-tree-children">
          {node.children.map((child) => (
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
        </div>
      )}
    </>
  );
}
