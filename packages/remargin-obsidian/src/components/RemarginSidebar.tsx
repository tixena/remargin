/** The plugin's sidebar: the React tree mounted in its leaf. */

import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join as joinPath } from "node:path";
import { Notice, type TFile } from "obsidian";
import { useCallback, useEffect, useMemo, useState } from "react";
import { buildIdentityArgs } from "@/backend/buildIdentityArgs";
import type { StagedGroup } from "@/components/sidebar/buildPromptGroups";
import { InboxSection } from "@/components/sidebar/InboxSection";
import { InlineCommentEditor } from "@/components/sidebar/InlineCommentEditor";
import type { InlinePromptEditorSaveArgs } from "@/components/sidebar/InlinePromptEditor";
import { InlineReplyEditor } from "@/components/sidebar/InlineReplyEditor";
import { SandboxSection } from "@/components/sidebar/SandboxSection";
import { SidebarShell } from "@/components/sidebar/SidebarShell";
import { ThreadedComments } from "@/components/sidebar/ThreadedComments";
import { ViewToggle } from "@/components/sidebar/ViewToggle";
import { expandPath } from "@/lib/expandPath";
import { openFileAtLine } from "@/lib/openFile";
import {
  buildSubmitShellLine,
  composeInlinePrompt,
  defaultRunner,
  promptFileSlug,
} from "@/lib/submitCommand";
import { launchInTerminal, resolveTerminal } from "@/lib/terminalLauncher";
import type RemarginPlugin from "@/main";
import type { InboxFilter, ViewMode } from "@/types";

/** Props for {@link RemarginSidebar}. */
interface RemarginSidebarProps {
  plugin: RemarginPlugin;
}

/** Where the inline composer is open: the file and the line the new comment goes after. */
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
    // `getActiveFile()` is null while the sidebar has focus, hence the cached markdown view.
    return plugin.app.workspace.getActiveFile()?.path ?? plugin.getLastMarkdownView()?.file?.path;
  });
  const [compose, setCompose] = useState<ComposeState | null>(null);
  const [replyTarget, setReplyTarget] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [sandboxView, setSandboxViewState] = useState<ViewMode>(plugin.settings.sandboxView);
  const [inboxView, setInboxViewState] = useState<ViewMode>(plugin.settings.inboxView);
  const [inboxFilter, setInboxFilterState] = useState<InboxFilter>(plugin.settings.inboxFilter);
  const [availableFolders, setAvailableFolders] = useState<string[]>([]);

  const bumpRefresh = useCallback(() => {
    setRefreshKey((k) => k + 1);
  }, []);

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

  const handleInboxFilter = useCallback(
    (next: InboxFilter) => {
      setInboxFilterState(next);
      void plugin.saveSettings({ ...plugin.settings, inboxFilter: next });
    },
    [plugin]
  );

  useEffect(() => {
    const { workspace } = plugin.app;

    const syncActiveFile = () => {
      const path = workspace.getActiveFile()?.path ?? plugin.getLastMarkdownView()?.file?.path;
      if (path) setActiveFile(path);
    };

    const fileOpenRef = workspace.on("file-open", (file: TFile | null) => {
      setActiveFile(file?.path);
      // A compose in progress targets a line of the old file, so switching files closes it.
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

  // The plugin drains any compose request that arrived before this handler registered.
  useEffect(() => {
    plugin.setComposeHandler((request) => {
      setCompose({ file: request.file, afterLine: request.afterLine });
    });
    return () => plugin.setComposeHandler(null);
  }, [plugin]);

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
        void openFileAtLine(plugin, target.file, insertedLine);
      }
      bumpRefresh();
    },
    [compose, plugin, bumpRefresh]
  );

  const handleSandboxSubmit = useCallback(
    async (groups: StagedGroup[]): Promise<void> => {
      if (groups.length === 0) return;
      const prefix = resolveTerminal(plugin.settings.terminalCommand, process.platform);
      if (!prefix) {
        new Notice(
          "No terminal emulator found. Set the Terminal command option in the Remargin settings."
        );
        return;
      }
      plugin.backend.invalidatePluginPresence();
      const presence = await plugin.backend.detectPlugin();
      const slashAvailable = presence.kind === "installed_enabled";

      const tempDir = mkdtempSync(joinPath(tmpdir(), "remargin-submit-"));
      const usedSlugs = new Set<string>();
      const entries = groups.map((group) => {
        const customRunner = group.prompt.runner?.trim() || "";
        // Marker cleanup runs in the shell line under the submitter's identity: the launched agent is
        // a different identity and cannot see the submitter's markers.
        const promptText =
          !customRunner && slashAvailable
            ? `/remargin:process-sandbox-group ${group.prompt.name}`
            : composeInlinePrompt(group.prompt.prompt, group.files);
        let slug = promptFileSlug(group.prompt.name);
        for (let n = 2; usedSlugs.has(slug); n += 1) {
          slug = `${promptFileSlug(group.prompt.name)}-${n}`;
        }
        usedSlugs.add(slug);
        const promptFile = joinPath(tempDir, `${slug}.md`);
        writeFileSync(promptFile, promptText, "utf-8");
        const runner =
          customRunner ||
          defaultRunner(plugin.backend.resolveClaudeBinary(), plugin.backend.resolveBinary());
        return { promptFile, runner, files: group.files };
      });

      const vaultPath =
        (plugin.app.vault.adapter as unknown as { basePath?: string }).basePath ?? "";
      const cwd = expandPath(plugin.settings.workingDirectory) || vaultPath;
      const cleanup = {
        remarginPath: plugin.backend.resolveBinary(),
        identityArgs: buildIdentityArgs(plugin.settings),
      };
      launchInTerminal(prefix, buildSubmitShellLine(entries, cleanup), cwd);
      new Notice(`Launched terminal for ${groups.length} group(s)`);
      bumpRefresh();
    },
    [plugin, bumpRefresh]
  );

  const handleSavePrompt = useCallback(
    async ({ source, name, prompt, runner }: InlinePromptEditorSaveArgs) => {
      const folder = dirname(source);
      await plugin.backend.promptSet(folder, name, prompt, runner);
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

  // Rendered inline below the targeted comment by ThreadedComments, not at the top of the thread.
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

  // A widget click may target another file: switch the filter so the card mounts before the scroll.
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
        />
      }
      inboxActions={<ViewToggle value={inboxView} onChange={handleInboxView} />}
      inboxContent={
        <InboxSection
          onOpenAtLine={handleOpenAtLine}
          refreshKey={refreshKey}
          viewMode={inboxView}
          filter={inboxFilter}
          onFilterChange={handleInboxFilter}
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
          />
        ) : undefined
      }
    />
  );
}
