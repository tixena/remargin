import { MarkdownView, type TFile } from "obsidian";
import { useCallback, useEffect, useMemo, useState } from "react";
import { InboxSection } from "@/components/sidebar/InboxSection";
import { InlineCommentEditor } from "@/components/sidebar/InlineCommentEditor";
import { PromptSection } from "@/components/sidebar/PromptSection";
import { SandboxSection } from "@/components/sidebar/SandboxSection";
import { SidebarShell } from "@/components/sidebar/SidebarShell";
import { ThreadedComments } from "@/components/sidebar/ThreadedComments";
import { snapAfterCommentBlock } from "@/lib/line-snap.ts";
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
 * Owns the cross-section state: the currently-active file, whether there is
 * an active markdown view (for the `+` button), an inline-compose target for
 * the `+` flow, and a monotonic `refreshKey` that child sections observe to
 * know when to refetch. The refresh button in the header and every
 * successful mutation both bump the key.
 */
export function RemarginSidebar({ plugin }: RemarginSidebarProps) {
  const [activeFile, setActiveFile] = useState<string | undefined>(() => {
    return plugin.app.workspace.getActiveFile()?.path;
  });
  const [hasMarkdownView, setHasMarkdownView] = useState<boolean>(() => {
    return !!plugin.app.workspace.getActiveViewOfType(MarkdownView);
  });
  const [compose, setCompose] = useState<ComposeState | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const bumpRefresh = useCallback(() => {
    setRefreshKey((k) => k + 1);
  }, []);

  // Track file-open and active-leaf changes so the header `+` button's
  // enabled state and the file-named section stay in sync with the workspace.
  useEffect(() => {
    const { workspace } = plugin.app;

    const fileOpenRef = workspace.on("file-open", (file: TFile | null) => {
      setActiveFile(file?.path);
      setHasMarkdownView(!!workspace.getActiveViewOfType(MarkdownView));
      // Switching files closes any in-progress compose — the cursor target
      // would be meaningless on a different file.
      setCompose(null);
    });

    const leafChangeRef = workspace.on("active-leaf-change", () => {
      setHasMarkdownView(!!workspace.getActiveViewOfType(MarkdownView));
    });

    return () => {
      workspace.offref(fileOpenRef);
      workspace.offref(leafChangeRef);
    };
  }, [plugin]);

  const handleOpenAtLine = useCallback(
    (filePath: string, line?: number) => {
      void openFileAtLine(plugin, filePath, line);
    },
    [plugin]
  );

  const handlePlusClick = useCallback(() => {
    const view = plugin.app.workspace.getActiveViewOfType(MarkdownView);
    if (!view || !view.file) return;
    const editor = view.editor;
    // Obsidian's editor API is 0-indexed; remargin's CLI is 1-indexed.
    const cursorLine1 = editor.getCursor().line + 1;
    const lines = editor.getValue().split("\n");
    const snapped = snapAfterCommentBlock(lines, cursorLine1);
    setCompose({ file: view.file.path, afterLine: snapped });
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

  const inlineEditor = useMemo(() => {
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
      plusDisabled={!hasMarkdownView}
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
          />
        ) : undefined
      }
    />
  );
}
