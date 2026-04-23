import { Notice, type TFile } from "obsidian";
import { useCallback, useEffect, useMemo, useState } from "react";
import { InboxSection } from "@/components/sidebar/InboxSection";
import { InlineCommentEditor } from "@/components/sidebar/InlineCommentEditor";
import { InlineReplyEditor } from "@/components/sidebar/InlineReplyEditor";
import { PromptSection } from "@/components/sidebar/PromptSection";
import { SandboxSection } from "@/components/sidebar/SandboxSection";
import { SidebarShell } from "@/components/sidebar/SidebarShell";
import { ThreadedComments } from "@/components/sidebar/ThreadedComments";
import { ViewToggle } from "@/components/sidebar/ViewToggle";
import { openFileAtLine } from "@/lib/openFile";
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

  const bumpRefresh = useCallback(() => {
    setRefreshKey((k) => k + 1);
  }, []);

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

  const handleSandboxSubmit = useCallback((_stagedFiles: string[]) => {
    // Placeholder for the actual Submit-to-Claude pipeline. Returning a
    // resolved promise is enough for SandboxSection to proceed with the
    // post-submit `sandbox remove` + refetch flow. Once the Claude handoff
    // lands it can be swapped in here without touching SandboxSection.
    return Promise.resolve();
  }, []);

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

  return (
    <SidebarShell
      plugin={plugin}
      activeFile={activeFile}
      refreshKey={refreshKey}
      onInitialized={bumpRefresh}
      onPlusClick={handlePlusClick}
      onRefreshClick={bumpRefresh}
      promptContent={<PromptSection file={activeFile} />}
      sandboxActions={<ViewToggle value={sandboxView} onChange={handleSandboxView} />}
      sandboxContent={
        <SandboxSection
          refreshKey={refreshKey}
          viewMode={sandboxView}
          onOpenFile={(path) => handleOpenAtLine(path)}
          onSubmit={handleSandboxSubmit}
        />
      }
      inboxActions={<ViewToggle value={inboxView} onChange={handleInboxView} />}
      inboxContent={
        <InboxSection
          onOpenAtLine={handleOpenAtLine}
          refreshKey={refreshKey}
          viewMode={inboxView}
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
