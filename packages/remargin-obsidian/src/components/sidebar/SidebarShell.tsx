import { Inbox, Mail, Plus, RefreshCw, Terminal } from "lucide-react";
import { useState } from "react";
import { ReMarginLogo } from "@/components/icons/ReMarginLogo";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
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
  plusDisabled,
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
      <div className="flex items-center justify-between px-4 py-3 gap-2 bg-bg-secondary border-b border-bg-border">
        <div className="flex items-center gap-2 min-w-0">
          <ReMarginLogo size={18} className="text-accent shrink-0" />
          <span className="text-base font-semibold text-text-normal font-sans truncate">
            Remargin
          </span>
          <Button
            variant="ghost"
            size="icon"
            className="w-[22px] h-[22px] rounded-sm shrink-0"
            onClick={onRefreshClick}
            aria-label="Refresh"
            title="Refresh"
          >
            <RefreshCw className="w-3 h-3 text-text-faint" />
          </Button>
        </div>
        <Button
          size="sm"
          className="h-7 px-3 text-xs gap-1 bg-accent text-white hover:bg-accent-hover font-semibold shrink-0"
          disabled={plusDisabled}
          onClick={onPlusClick}
          aria-label="New comment at cursor"
          title="New comment at cursor"
        >
          <Plus className="w-3.5 h-3.5" />
          New
        </Button>
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
