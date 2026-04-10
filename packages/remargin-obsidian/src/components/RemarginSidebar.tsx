import type { TFile } from "obsidian";
import { useCallback, useEffect, useState } from "react";
import { InboxSection } from "@/components/sidebar/InboxSection";
import { PromptSection } from "@/components/sidebar/PromptSection";
import { SidebarShell } from "@/components/sidebar/SidebarShell";
import { ThreadedComments } from "@/components/sidebar/ThreadedComments";
import { openFileAtLine } from "@/lib/openFile";
import type RemarginPlugin from "@/main";

interface RemarginSidebarProps {
  plugin: RemarginPlugin;
}

export function RemarginSidebar({ plugin }: RemarginSidebarProps) {
  const [activeFile, setActiveFile] = useState<string | undefined>(() => {
    return plugin.app.workspace.getActiveFile()?.path;
  });

  useEffect(() => {
    const ref = plugin.app.workspace.on("file-open", (file: TFile | null) => {
      setActiveFile(file?.path);
    });
    return () => {
      plugin.app.workspace.offref(ref);
    };
  }, [plugin]);

  const handleOpenAtLine = useCallback(
    (filePath: string, line?: number) => {
      void openFileAtLine(plugin, filePath, line);
    },
    [plugin]
  );

  return (
    <SidebarShell
      plugin={plugin}
      activeFile={activeFile}
      promptContent={<PromptSection file={activeFile} />}
      inboxContent={<InboxSection onOpenAtLine={handleOpenAtLine} />}
      threadContent={
        activeFile ? (
          <ThreadedComments
            file={activeFile}
            onGoToLine={(line) => handleOpenAtLine(activeFile, line)}
          />
        ) : undefined
      }
    />
  );
}
