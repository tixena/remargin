import { useEffect, useState } from "react";
import { TFile } from "obsidian";
import { SidebarShell } from "@/components/sidebar/SidebarShell";
import { PromptSection } from "@/components/sidebar/PromptSection";
import { InboxSection } from "@/components/sidebar/InboxSection";
import { ThreadedComments } from "@/components/sidebar/ThreadedComments";
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

  return (
    <SidebarShell
      plugin={plugin}
      activeFile={activeFile}
      promptContent={<PromptSection file={activeFile} />}
      inboxContent={<InboxSection />}
      threadContent={
        activeFile ? <ThreadedComments file={activeFile} /> : undefined
      }
    />
  );
}
