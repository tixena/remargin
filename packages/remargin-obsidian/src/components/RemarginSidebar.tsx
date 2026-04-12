import type { TFile } from "obsidian";
import { useCallback, useEffect, useMemo, useState } from "react";
import { InboxSection } from "@/components/sidebar/InboxSection";
import { InlineCommentEditor } from "@/components/sidebar/InlineCommentEditor";
import { InlineReplyEditor } from "@/components/sidebar/InlineReplyEditor";
import { PromptSection } from "@/components/sidebar/PromptSection";
import { SandboxSection } from "@/components/sidebar/SandboxSection";
import { SidebarShell } from "@/components/sidebar/SidebarShell";
import { ThreadedComments } from "@/components/sidebar/ThreadedComments";
import { openFileAtLine } from "@/lib/openFile";
import type RemarginPlugin from "@/main";

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
    return plugin.app.workspace.getActiveFile()?.path;
  });
  const [plusEnabled, setPlusEnabled] = useState<boolean>(() => {
    return plugin.getLastMarkdownView() !== null;
  });
  const [compose, setCompose] = useState<ComposeState | null>(null);
  const [replyTarget, setReplyTarget] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const bumpRefresh = useCallback(() => {
    setRefreshKey((k) => k + 1);
  }, []);

  // Track file-open and active-leaf changes so the header `+` button's
  // enabled state and the file-named section stay in sync with the workspace.
  // Note: `plusEnabled` reads from `plugin.getLastMarkdownView()`, which is
  // only cleared when the cached view's file is closed — it does NOT flip to
  // false just because focus moved to the sidebar.
  useEffect(() => {
    const { workspace } = plugin.app;

    const syncPlus = () => {
      setPlusEnabled(plugin.getLastMarkdownView() !== null);
    };

    const fileOpenRef = workspace.on("file-open", (file: TFile | null) => {
      setActiveFile(file?.path);
      syncPlus();
      // Switching files closes any in-progress compose — the cursor target
      // would be meaningless on a different file.
      setCompose(null);
    });

    const leafChangeRef = workspace.on("active-leaf-change", syncPlus);
    const layoutChangeRef = workspace.on("layout-change", syncPlus);

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
    void plugin.addComment();
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

  const inlineEditor = useMemo(() => {
    if (replyTarget && activeFile) {
      return (
        <InlineReplyEditor
          file={activeFile}
          replyTo={replyTarget}
          onClose={handleReplyClose}
          onSubmitted={handleReplySubmitted}
        />
      );
    }
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
  }, [
    compose,
    replyTarget,
    activeFile,
    handleComposeClose,
    handleComposeSubmitted,
    handleReplyClose,
    handleReplySubmitted,
  ]);

  return (
    <SidebarShell
      plugin={plugin}
      activeFile={activeFile}
      plusDisabled={!plusEnabled}
      onPlusClick={handlePlusClick}
      onRefreshClick={bumpRefresh}
      promptContent={<PromptSection file={activeFile} />}
      sandboxContent={
        <SandboxSection
          refreshKey={refreshKey}
          onOpenFile={(path) => handleOpenAtLine(path)}
          onSubmit={handleSandboxSubmit}
        />
      }
      inboxContent={<InboxSection onOpenAtLine={handleOpenAtLine} />}
      threadInlineEditor={inlineEditor}
      threadContent={
        activeFile ? (
          <ThreadedComments
            key={`${activeFile}:${refreshKey}`}
            file={activeFile}
            onGoToLine={(line) => handleOpenAtLine(activeFile, line)}
            onReply={(commentId) => {
              setCompose(null);
              setReplyTarget(commentId);
            }}
          />
        ) : undefined
      }
    />
  );
}
