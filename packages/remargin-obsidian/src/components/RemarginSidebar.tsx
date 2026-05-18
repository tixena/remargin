import { dirname, relative as relativePath } from "node:path";
import { Notice, TFile } from "obsidian";
import { useCallback, useEffect, useMemo, useState } from "react";
import type {
  StagedGroup,
  SubmitGroupResult,
  SubmitProgress,
} from "@/components/sidebar/buildPromptGroups";
import { InboxSection } from "@/components/sidebar/InboxSection";
import { InlineCommentEditor } from "@/components/sidebar/InlineCommentEditor";
import type { InlinePromptEditorSaveArgs } from "@/components/sidebar/InlinePromptEditor";
import { InlineReplyEditor } from "@/components/sidebar/InlineReplyEditor";
import { KindFilterBar } from "@/components/sidebar/KindFilterBar";
import { SandboxSection } from "@/components/sidebar/SandboxSection";
import { SidebarShell } from "@/components/sidebar/SidebarShell";
import { ThreadedComments } from "@/components/sidebar/ThreadedComments";
import { ViewToggle } from "@/components/sidebar/ViewToggle";
import { pruneKindFilter } from "@/lib/kindFilter";
import { openFileAtLine } from "@/lib/openFile";
import { runSubmitAll } from "@/lib/submitAllPipeline";
import type RemarginPlugin from "@/main";
import type { ViewMode } from "@/types";

interface RemarginSidebarProps {
  plugin: RemarginPlugin;
}

interface ComposeState {
  file: string;
  afterLine: number;
}

/**
 * Top-level React tree mounted inside the plugin's sidebar leaf.
 *
 * Owns the cross-section state: the currently-active file, whether the
 * header `+` button should be enabled (driven by the plugin's stable
 * last-markdown-view cache — NOT by the focused leaf, which flips to null
 * when the user clicks the sidebar), an inline-compose target for the `+`
 * flow, and a monotonic `refreshKey` that child sections observe to know
 * when to refetch. The refresh button in the header and every successful
 * mutation both bump the key.
 */
export function RemarginSidebar({ plugin }: RemarginSidebarProps) {
  const [activeFile, setActiveFile] = useState<string | undefined>(() => {
    // getActiveFile() returns null when the sidebar is the active leaf, so fall
    // back to the plugin's cached last markdown view.
    return plugin.app.workspace.getActiveFile()?.path ?? plugin.getLastMarkdownView()?.file?.path;
  });
  const [compose, setCompose] = useState<ComposeState | null>(null);
  const [replyTarget, setReplyTarget] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [sandboxView, setSandboxViewState] = useState<ViewMode>(plugin.settings.sandboxView);
  const [inboxView, setInboxViewState] = useState<ViewMode>(plugin.settings.inboxView);
  // Session-only remargin_kind filter — deliberately NOT persisted so a
  // new session starts with every comment visible. The two discovered-
  // kinds buckets track what each section currently has loaded so the
  // chip row reflects the union across both.
  const [kindFilter, setKindFilter] = useState<string[]>([]);
  const [inboxKinds, setInboxKinds] = useState<string[]>([]);
  const [threadKinds, setThreadKinds] = useState<string[]>([]);
  const [availableFolders, setAvailableFolders] = useState<string[]>([]);

  const bumpRefresh = useCallback(() => {
    setRefreshKey((k) => k + 1);
  }, []);

  // Keep the create-mode folder picker fed with current vault folders.
  // Sort lexicographically; vault root (path "") sorts to the top and
  // renders as "(vault root)" inside the picker.
  useEffect(() => {
    const refreshFolders = () => {
      const folders = plugin.app.vault.getAllFolders(true).map((f) => f.path);
      folders.sort((a, b) => a.localeCompare(b));
      setAvailableFolders(folders);
    };
    refreshFolders();
    const createRef = plugin.app.vault.on("create", refreshFolders);
    const deleteRef = plugin.app.vault.on("delete", refreshFolders);
    const renameRef = plugin.app.vault.on("rename", refreshFolders);
    return () => {
      plugin.app.vault.offref(createRef);
      plugin.app.vault.offref(deleteRef);
      plugin.app.vault.offref(renameRef);
    };
  }, [plugin]);

  const handleSandboxView = useCallback(
    (next: ViewMode) => {
      setSandboxViewState(next);
      void plugin.saveSettings({ ...plugin.settings, sandboxView: next });
    },
    [plugin]
  );

  const handleInboxView = useCallback(
    (next: ViewMode) => {
      setInboxViewState(next);
      void plugin.saveSettings({ ...plugin.settings, inboxView: next });
    },
    [plugin]
  );

  // Union of the two sections' discovered kinds, deduped and sorted
  // case-insensitively. This is what the chip row renders; children
  // already supply pre-sorted lists so a merge + re-sort is cheap.
  const availableKinds = useMemo(() => {
    if (inboxKinds.length === 0) return threadKinds;
    if (threadKinds.length === 0) return inboxKinds;
    const merged = new Set<string>();
    for (const k of inboxKinds) merged.add(k);
    for (const k of threadKinds) merged.add(k);
    return Array.from(merged).sort((a, b) =>
      a.localeCompare(b, undefined, { sensitivity: "base" })
    );
  }, [inboxKinds, threadKinds]);

  // When the visible data shrinks (switching files, inbox toggled from
  // All to Pending, etc.) drop any selection that is no longer offered.
  // Otherwise the chip row shows nothing active but the filter keeps
  // hiding comments — confusing and sticky.
  useEffect(() => {
    setKindFilter((prev) => pruneKindFilter(prev, availableKinds));
  }, [availableKinds]);

  // Switching to a no-file state (or a different file) must clear the
  // previous file's discovered kinds, otherwise the chip row keeps
  // advertising tags that the inbox alone cannot back. ThreadedComments
  // only mounts when `activeFile` is defined, so it can't push a reset
  // itself — we do it here in the one place that knows about the
  // transition.
  useEffect(() => {
    if (!activeFile) setThreadKinds([]);
  }, [activeFile]);

  // Keep `activeFile` in sync with the workspace so the file-named section and
  // the inline composer always target whichever markdown file the user last
  // interacted with.
  useEffect(() => {
    const { workspace } = plugin.app;

    const syncActiveFile = () => {
      const path = workspace.getActiveFile()?.path ?? plugin.getLastMarkdownView()?.file?.path;
      if (path) setActiveFile(path);
    };

    const fileOpenRef = workspace.on("file-open", (file: TFile | null) => {
      setActiveFile(file?.path);
      // Switching files closes any in-progress compose — the cursor target
      // would be meaningless on a different file.
      setCompose(null);
    });

    const leafChangeRef = workspace.on("active-leaf-change", syncActiveFile);
    const layoutChangeRef = workspace.on("layout-change", syncActiveFile);

    return () => {
      workspace.offref(fileOpenRef);
      workspace.offref(leafChangeRef);
      workspace.offref(layoutChangeRef);
    };
  }, [plugin]);

  // Register our compose handler with the plugin so its `Add comment`
  // command and `+` button can both request the composer to open. The
  // plugin drains any compose request that arrived before we registered.
  useEffect(() => {
    plugin.setComposeHandler((request) => {
      setCompose({ file: request.file, afterLine: request.afterLine });
    });
    return () => plugin.setComposeHandler(null);
  }, [plugin]);

  // Register our refresh handler with the plugin so the `Refresh comments`
  // command (and any future external triggers) can bump every section's
  // refreshKey and force a refetch. Mirrors the compose-handler pattern:
  // the plugin drains any refresh requested before we mounted.
  useEffect(() => {
    plugin.setRefreshHandler(bumpRefresh);
    return () => plugin.setRefreshHandler(null);
  }, [plugin, bumpRefresh]);

  const handleOpenAtLine = useCallback(
    (filePath: string, line?: number) => {
      void openFileAtLine(plugin, filePath, line);
    },
    [plugin]
  );

  const handlePlusClick = useCallback(() => {
    // Surface any failure instead of letting the promise reject silently —
    // the user clicked a button, they deserve to know if it failed.
    plugin.addComment().catch((err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err);
      console.error("[remargin] addComment failed:", err);
      new Notice(`Add comment failed: ${msg}`);
    });
  }, [plugin]);

  const handleComposeClose = useCallback(() => {
    setCompose(null);
  }, []);

  const handleComposeSubmitted = useCallback(
    (insertedLine: number) => {
      const target = compose;
      setCompose(null);
      if (target) {
        // Re-open the file at the new comment's line. This both refreshes
        // the editor view (so the new block renders) and scrolls to it.
        void openFileAtLine(plugin, target.file, insertedLine);
      }
      // Fire the plugin-wide refresh so every sidebar section refetches.
      bumpRefresh();
    },
    [compose, plugin, bumpRefresh]
  );

  const handleSandboxSubmit = useCallback(
    async (groups: StagedGroup[], progress?: SubmitProgress): Promise<SubmitGroupResult[]> => {
      plugin.backend.invalidatePluginPresence();
      const presence = await plugin.backend.detectPlugin();
      const useSlash = presence.kind === "installed_enabled";
      const results = await runSubmitAll({
        groups,
        runGroup: (group) =>
          useSlash
            ? plugin.backend.invokeClaude("", [], {
                logPath: group.logPath,
                promptName: group.prompt.name,
                useSlashCommand: {
                  command: "remargin:process-sandbox-group",
                  arg: group.prompt.name,
                },
              })
            : plugin.backend.invokeClaude(group.prompt.prompt, group.files, {
                logPath: group.logPath,
                promptName: group.prompt.name,
              }),
        cleanupGroup: (group) =>
          useSlash ? Promise.resolve() : plugin.backend.sandboxRemove(group.files),
        bumpRefresh,
        progress,
      });
      // Final rescan so the list reflects every successful clear.
      bumpRefresh();
      return results;
    },
    [plugin, bumpRefresh]
  );

  const handleOpenLog = useCallback(
    async (absLogPath: string): Promise<void> => {
      const vaultPath =
        (plugin.app.vault.adapter as unknown as { basePath?: string }).basePath ?? "";
      const relPath = vaultPath ? relativePath(vaultPath, absLogPath) : absLogPath;
      const adapter = plugin.app.vault.adapter;
      try {
        const dir = dirname(relPath);
        // WHY: vault.adapter keeps Obsidian's file index in sync; node's
        // fs would leave the new file invisible to getAbstractFileByPath
        // until the file watcher polls, and the fallback through
        // openLinkText then re-tries createFolder and throws.
        if (dir && dir !== "." && !(await adapter.exists(dir))) {
          await adapter.mkdir(dir);
        }
        if (!(await adapter.exists(relPath))) {
          await adapter.write(relPath, "");
        }
      } catch (err) {
        console.error("[remargin] failed to ensure log file exists:", err);
        new Notice(`Could not prepare submit log: ${err instanceof Error ? err.message : err}`);
        return;
      }
      const tfile = plugin.app.vault.getAbstractFileByPath(relPath);
      if (!(tfile instanceof TFile)) {
        console.error("[remargin] submit log not in vault index after write:", relPath);
        new Notice(`Submit log written but not yet indexed: ${relPath}`);
        return;
      }
      try {
        await plugin.app.workspace.getLeaf("tab").openFile(tfile);
      } catch (err) {
        console.error("[remargin] failed to open submit log:", err);
        new Notice(`Could not open submit log: ${err instanceof Error ? err.message : err}`);
      }
    },
    [plugin]
  );

  const handleSavePrompt = useCallback(
    async ({ source, name, prompt }: InlinePromptEditorSaveArgs) => {
      const folder = dirname(source);
      await plugin.backend.promptSet(folder, name, prompt);
      bumpRefresh();
    },
    [plugin, bumpRefresh]
  );

  const handleDeletePrompt = useCallback(
    async (source: string) => {
      const folder = dirname(source);
      await plugin.backend.promptDelete(folder);
      bumpRefresh();
    },
    [plugin, bumpRefresh]
  );

  const handleReplyClose = useCallback(() => {
    setReplyTarget(null);
  }, []);

  const handleReplySubmitted = useCallback(() => {
    setReplyTarget(null);
    bumpRefresh();
  }, [bumpRefresh]);

  // Reply composer — rendered inline below the targeted comment by
  // ThreadedComments (see `threadContent` below), NOT at the top of the
  // thread. Keeping it here centralizes the identity/file/callback
  // plumbing so ThreadedComments only needs to know where to drop it.
  const replyEditor = useMemo(() => {
    if (!replyTarget || !activeFile) return null;
    return (
      <InlineReplyEditor
        file={activeFile}
        replyTo={replyTarget}
        onClose={handleReplyClose}
        onSubmitted={handleReplySubmitted}
      />
    );
  }, [replyTarget, activeFile, handleReplyClose, handleReplySubmitted]);

  // Compose-new-comment — the `+` / "Add comment" flow. This is not tied
  // to a specific comment row, so it stays in the top-of-thread slot.
  const composeEditor = useMemo(() => {
    if (!compose) return null;
    if (activeFile !== compose.file) return null;
    return (
      <InlineCommentEditor
        file={compose.file}
        afterLine={compose.afterLine}
        onClose={handleComposeClose}
        onSubmitted={handleComposeSubmitted}
      />
    );
  }, [compose, activeFile, handleComposeClose, handleComposeSubmitted]);

  // Plugin → sidebar focus bridge: when a widget click in the editor
  // requests a comment in a different file, switch the active filter
  // so the targeted card mounts before SidebarShell scrolls + highlights.
  const handleFocusFile = useCallback((file: string) => {
    setActiveFile(file);
  }, []);

  return (
    <SidebarShell
      plugin={plugin}
      activeFile={activeFile}
      onFocusFile={handleFocusFile}
      refreshKey={refreshKey}
      onInitialized={bumpRefresh}
      onPlusClick={handlePlusClick}
      onRefreshClick={bumpRefresh}
      sandboxActions={<ViewToggle value={sandboxView} onChange={handleSandboxView} />}
      sandboxContent={
        <SandboxSection
          refreshKey={refreshKey}
          viewMode={sandboxView}
          onOpenFile={(path) => handleOpenAtLine(path)}
          onSubmit={handleSandboxSubmit}
          onSavePrompt={handleSavePrompt}
          onDeletePrompt={handleDeletePrompt}
          availableFolders={availableFolders}
          vaultRoot={
            (plugin.app.vault.adapter as unknown as { basePath?: string }).basePath ?? undefined
          }
          onOpenLog={handleOpenLog}
        />
      }
      inboxActions={<ViewToggle value={inboxView} onChange={handleInboxView} />}
      inboxContent={
        <InboxSection
          onOpenAtLine={handleOpenAtLine}
          refreshKey={refreshKey}
          viewMode={inboxView}
          kindFilter={kindFilter}
          onKindsDiscovered={setInboxKinds}
        />
      }
      filterBar={
        <KindFilterBar
          availableKinds={availableKinds}
          selected={kindFilter}
          onChange={setKindFilter}
        />
      }
      threadInlineEditor={composeEditor}
      threadContent={
        activeFile ? (
          <ThreadedComments
            key={activeFile}
            file={activeFile}
            refreshKey={refreshKey}
            onGoToLine={(line) => handleOpenAtLine(activeFile, line)}
            onMutation={bumpRefresh}
            onReply={(commentId) => {
              setCompose(null);
              setReplyTarget(commentId);
            }}
            replyTarget={replyTarget}
            replyEditor={replyEditor}
            kindFilter={kindFilter}
            onKindsDiscovered={setThreadKinds}
          />
        ) : undefined
      }
    />
  );
}
