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
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { useBackend } from "@/hooks/useBackend";
import { buildFileTree, type FileTreeNode } from "@/lib/buildFileTree";
import { submitLogPath } from "@/lib/submitLogPath";
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
  /**
   * Vault folders the create-mode picker can offer. Forwarded to
   * `<InlinePromptEditor>` via `<PromptGroupSection>`.
   */
  availableFolders?: string[];
  /**
   * Absolute filesystem path of the vault root. When set, per-prompt
   * headers strip it from `group.scope` so the second row renders as a
   * vault-relative path (e.g. `./src/01_personal/remargin/`).
   */
  vaultRoot?: string;
  /**
   * Called with an absolute log-file path so the caller can open it in
   * an Obsidian leaf. Invoked once per group at Submit time (for live
   * tail) and again when the user clicks a failed group's log link.
   */
  onOpenLog?: (logPath: string) => void;
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

function firstErrorLine(raw?: string): string | undefined {
  if (!raw) return undefined;
  const lines = raw
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
  if (lines.length === 0) return undefined;
  const flagged = lines.find((l) => /error|fail/i.test(l));
  return flagged ?? lines[0];
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
  onOpenLog,
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
  const [groupLogPaths, setGroupLogPaths] = useState<Map<string, string>>(new Map());

  const buildStagedWithLog = useCallback(
    (group: PromptGroup, stagedFiles: string[]): StagedGroup => {
      const lp = vaultRoot ? submitLogPath(vaultRoot, group.prompt.name) : undefined;
      const key = group.source ?? DEFAULT_GROUP_KEY;
      if (lp) {
        setGroupLogPaths((prev) => new Map(prev).set(key, lp));
        onOpenLog?.(lp);
      }
      return { prompt: group.prompt, files: stagedFiles, logPath: lp };
    },
    [vaultRoot, onOpenLog]
  );

  const handleSubmitGroup = useCallback(
    async (group: PromptGroup) => {
      if (submitting) return;
      if (group.hasError) return;
      const stagedFiles = group.files.filter((f) => staged.has(f));
      if (stagedFiles.length === 0) return;
      const key = group.source ?? DEFAULT_GROUP_KEY;
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

      const payload: StagedGroup[] = [buildStagedWithLog(group, stagedFiles)];

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
    [submitting, staged, onSubmit, buildStagedWithLog]
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
    setGroupStatus(new Map());
    setGroupErrors(new Map());
    const payload: StagedGroup[] = eligibleSubmitGroups.map((g) => buildStagedWithLog(g, g.staged));
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
      console.error("SandboxSection.handleSubmitAll failed:", err);
      setError(errorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }, [submitting, eligibleSubmitGroups, onSubmit, buildStagedWithLog]);

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
              availableFolders={availableFolders}
              vaultRoot={vaultRoot}
              onSubmitGroup={handleSubmitGroup}
              submitting={submitting}
              logPath={groupLogPaths.get(key)}
              onOpenLog={onOpenLog}
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
  /** Forwarded to the create-mode folder picker in `<InlinePromptEditor>`. */
  availableFolders?: string[];
  /** Forwarded so the header's second row can render a vault-relative scope. */
  vaultRoot?: string;
  /** Per-group Submit. Renders inside the Staged sub-section. */
  onSubmitGroup?: (group: PromptGroup) => void | Promise<void>;
  /** True while any group's Submit is in flight; disables every group's Submit. */
  submitting?: boolean;
  /** Absolute log-file path for this group's most recent submit, if any. */
  logPath?: string;
  /** Click handler bound to the failed-status icon when a logPath exists. */
  onOpenLog?: (logPath: string) => void;
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
  availableFolders,
  vaultRoot,
  onSubmitGroup,
  submitting,
  logPath,
  onOpenLog,
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
          {status === "pending" && (
            <span className="rmg-l2__status rmg-l2__status--pending">
              <Loader2 className="animate-spin" aria-label="Submitting" />
            </span>
          )}
          {status === "ok" && (
            <span className="rmg-l2__status rmg-l2__status--ok text-green-500">
              <Check aria-label="Submitted" />
            </span>
          )}
          {status === "failed" &&
            (logPath && onOpenLog ? (
              <button
                type="button"
                className="rmg-icon-btn rmg-icon-btn--sm rmg-l2__status rmg-l2__status--fail text-red-400"
                title={firstErrorLine(statusError) ?? "Open submit log"}
                onClick={(e) => {
                  e.stopPropagation();
                  onOpenLog(logPath);
                }}
                aria-label="Open submit log"
              >
                <TriangleAlert />
              </button>
            ) : (
              <span
                className="rmg-l2__status rmg-l2__status--fail text-red-400"
                title={statusError}
              >
                <TriangleAlert aria-label="Submit failed" />
              </span>
            ))}
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
          onSave={handleSave}
          onDelete={!group.isDefault && onDeletePrompt ? handleDelete : undefined}
          onCancel={() => setEditing(false)}
          saveDisabledReason={savePromptDisabledReason}
          availableFolders={availableFolders}
        />
      )}

      {headerOpen && (
        <>
          {/* ====== L3: Staged ====== */}
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

          {/* ====== L3: Unstaged ====== */}
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
