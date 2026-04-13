import { Inbox, Mail, Terminal } from "lucide-react";
import { useState } from "react";
import { ReMarginLogo } from "@/components/icons/ReMarginLogo";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { ScrollArea } from "@/components/ui/scroll-area";
import type RemarginPlugin from "@/main";
import { FilePathHeader } from "./FilePathHeader";
import { SectionHeader } from "./SectionHeader";

interface SidebarShellProps {
  plugin: RemarginPlugin;
  activeFile?: string;
  sandboxCount?: number;
  inboxCount?: number;
  threadPending?: number;
  /** Handler for the header `+` button. */
  onPlusClick?: () => void;
  /**
   * Handler for the header refresh button. Firing it should cause every
   * sidebar section to refetch its data.
   */
  onRefreshClick?: () => void;
  promptContent?: React.ReactNode;
  sandboxContent?: React.ReactNode;
  sandboxActions?: React.ReactNode;
  inboxContent?: React.ReactNode;
  /** Right-aligned actions slot for the Inbox section header. */
  inboxActions?: React.ReactNode;
  threadContent?: React.ReactNode;
  /**
   * Optional content rendered inside the file-named section, above the
   * thread list. Used by the `+` flow to show an inline comment editor
   * next to the file it targets (no modal).
   */
  threadInlineEditor?: React.ReactNode;
  footerContent?: React.ReactNode;
}

export function SidebarShell({
  plugin,
  activeFile,
  sandboxCount = 0,
  inboxCount = 0,
  threadPending = 0,
  onPlusClick,
  onRefreshClick,
  promptContent,
  sandboxContent,
  sandboxActions,
  inboxContent,
  inboxActions,
  threadContent,
  threadInlineEditor,
  footerContent,
}: SidebarShellProps) {
  const [promptOpen, setPromptOpen] = useState(false);
  const [sandboxOpen, setSandboxOpen] = useState(true);
  const [inboxOpen, setInboxOpen] = useState(true);
  const [threadOpen, setThreadOpen] = useState(true);

  return (
    <div className="flex flex-col h-full bg-bg-primary">
      <div className="flex items-center justify-between px-4 py-3 gap-2 bg-bg-secondary border-b border-bg-border overflow-hidden">
        <div className="flex items-center gap-2 min-w-0">
          <ReMarginLogo size={22} className="text-accent shrink-0" />
          <span className="text-base font-semibold text-text-normal font-sans truncate min-w-0">
            Remargin
          </span>
          <button
            type="button"
            onClick={onRefreshClick}
            aria-label="Refresh"
            title="Refresh"
            style={{
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              width: 22,
              height: 22,
              borderRadius: 4,
              border: "none",
              cursor: "pointer",
              backgroundColor: "transparent",
              padding: 0,
              flexShrink: 0,
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.backgroundColor = "var(--background-modifier-hover)";
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.backgroundColor = "transparent";
            }}
          >
            <ObsidianIcon icon="refresh-cw" size={12} />
          </button>
        </div>
        <button
          type="button"
          onClick={onPlusClick}
          aria-label="New comment at cursor"
          title="New comment at cursor"
          style={{
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            height: 28,
            padding: "0 12px",
            borderRadius: 6,
            border: "none",
            cursor: "pointer",
            backgroundColor: "var(--interactive-accent)",
            color: "var(--text-on-accent)",
            fontSize: 12,
            fontWeight: 600,
            gap: 4,
            flexShrink: 0,
          }}
        >
          <ObsidianIcon icon="plus" size={14} />
          New
        </button>
      </div>

      <ScrollArea className="flex-1">
        <div className="flex flex-col">
          <Collapsible open={promptOpen} onOpenChange={setPromptOpen}>
            <SectionHeader icon={Terminal} title="Prompt" open={promptOpen} />
            <CollapsibleContent>
              {promptContent ?? (
                <div className="px-4 py-3 text-xs text-text-faint">No prompt configured.</div>
              )}
            </CollapsibleContent>
          </Collapsible>

          <Collapsible open={sandboxOpen} onOpenChange={setSandboxOpen}>
            <SectionHeader
              icon={Inbox}
              title="Sandbox"
              badge={sandboxCount || undefined}
              open={sandboxOpen}
              actions={sandboxActions}
            />
            <CollapsibleContent>
              {sandboxContent ?? (
                <div className="px-4 py-3 text-xs text-text-faint">No staged comments.</div>
              )}
            </CollapsibleContent>
          </Collapsible>

          <Collapsible open={inboxOpen} onOpenChange={setInboxOpen}>
            <SectionHeader
              icon={Mail}
              title="Inbox"
              badge={inboxCount || undefined}
              badgeVariant="warning"
              open={inboxOpen}
              actions={inboxActions}
            />
            <CollapsibleContent>
              {inboxContent ?? (
                <div className="px-4 py-3 text-xs text-text-faint">No pending comments.</div>
              )}
            </CollapsibleContent>
          </Collapsible>

          <Collapsible open={threadOpen} onOpenChange={setThreadOpen}>
            <FilePathHeader plugin={plugin} filePath={activeFile} pendingCount={threadPending} />
            <CollapsibleContent>
              {threadInlineEditor}
              {threadContent ?? (
                <div className="px-4 py-3 text-xs text-text-faint">
                  Open a markdown file to see comments.
                </div>
              )}
            </CollapsibleContent>
          </Collapsible>
        </div>
      </ScrollArea>

      {footerContent && <div className="border-t border-bg-border">{footerContent}</div>}
    </div>
  );
}
