import { FileText, Inbox, Mail, MessageSquare, Plus, RefreshCw, Terminal } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
import { ScrollArea } from "@/components/ui/scroll-area";
import type RemarginPlugin from "@/main";
import { SectionHeader } from "./SectionHeader";

interface SidebarShellProps {
  plugin: RemarginPlugin;
  activeFile?: string;
  sandboxCount?: number;
  inboxCount?: number;
  threadPending?: number;
  /**
   * Disables the header `+` button. The parent flips this reactively based
   * on whether there is an active `MarkdownView` in the workspace.
   */
  plusDisabled?: boolean;
  /**
   * Handler for the header `+` button. Only invoked when the button is
   * enabled; the shell does not guess at the semantics.
   */
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
  activeFile,
  sandboxCount = 0,
  inboxCount = 0,
  threadPending = 0,
  plusDisabled,
  onPlusClick,
  onRefreshClick,
  promptContent,
  sandboxContent,
  sandboxActions,
  inboxContent,
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
      <div className="flex items-center justify-between px-4 py-3 gap-2 bg-bg-secondary border-b border-bg-border">
        <div className="flex items-center gap-2">
          <MessageSquare className="w-4 h-4 text-accent" />
          <span className="text-base font-semibold text-text-normal font-sans">Remargin</span>
        </div>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            className="w-7 h-7"
            disabled={plusDisabled}
            onClick={onPlusClick}
            aria-label="New comment at cursor"
            title="New comment at cursor"
          >
            <Plus className="w-3.5 h-3.5 text-text-muted" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="w-7 h-7"
            onClick={onRefreshClick}
            aria-label="Refresh"
            title="Refresh"
          >
            <RefreshCw className="w-3.5 h-3.5 text-text-muted" />
          </Button>
        </div>
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
            />
            <CollapsibleContent>
              {inboxContent ?? (
                <div className="px-4 py-3 text-xs text-text-faint">No pending comments.</div>
              )}
            </CollapsibleContent>
          </Collapsible>

          <Collapsible open={threadOpen} onOpenChange={setThreadOpen}>
            <div className="flex items-center gap-2 px-4 py-2 bg-bg-border border-t border-bg-border">
              <FileText className="w-3.5 h-3.5 text-text-faint" />
              <span className="font-mono text-xs text-text-muted truncate">
                {activeFile ?? "No file open"}
              </span>
              {threadPending > 0 && (
                <span className="px-1.5 py-0 text-[9px] font-semibold leading-4 rounded-full bg-amber-400 text-bg-primary whitespace-nowrap">
                  {threadPending} pending
                </span>
              )}
            </div>
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
