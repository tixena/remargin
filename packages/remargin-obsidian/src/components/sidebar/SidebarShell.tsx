import { Inbox, Mail, Terminal } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { ReMarginLogo } from "@/components/icons/ReMarginLogo";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { ScrollArea } from "@/components/ui/scroll-area";
import type RemarginPlugin from "@/main";
import type { RemarginFocusDetail } from "@/main";
import { FilePathHeader } from "./FilePathHeader";
import { focusCardInRoot } from "./focusCard";
import { SectionHeader } from "./SectionHeader";

interface SidebarShellProps {
  plugin: RemarginPlugin;
  activeFile?: string;
  /**
   * Called when a focus request from the plugin (`focusComment`) targets
   * a file other than `activeFile`. The parent should switch the active
   * filter to `file` so the targeted card mounts before the shell
   * scrolls + highlights it. When omitted, cross-file focus events are
   * silently ignored — the scroll path still runs but matches nothing.
   */
  onFocusFile?: (file: string) => void;
  sandboxCount?: number;
  inboxCount?: number;
  threadPending?: number;
  /**
   * Monotonic refresh signal — bumped by the sidebar on any
   * mutation. Forwarded to children that cache per-file state
   * (currently the `Initialize` detection in `FilePathHeader`,
   * rem-rvk6).
   */
  refreshKey?: number;
  /** Called by the `Initialize` flow after `remargin write` succeeds. */
  onInitialized?: () => void;
  /** Handler for the header `+` button. */
  onPlusClick?: () => void;
  /**
   * Handler for the header refresh button. Firing it should cause every
   * sidebar section to refetch its data.
   */
  onRefreshClick?: () => void;
  promptContent?: React.ReactNode;
  /**
   * Optional filter/toolbar row rendered directly below the top header
   * and above the scrollable section list. Stays visible while the
   * user scrolls long Inbox or thread lists, which is the point of
   * having a filter. Hidden entirely when omitted, so plugins that
   * don't provide one see the original layout.
   */
  filterBar?: React.ReactNode;
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
  onFocusFile,
  sandboxCount = 0,
  inboxCount = 0,
  threadPending = 0,
  refreshKey,
  onInitialized,
  onPlusClick,
  onRefreshClick,
  promptContent,
  filterBar,
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
  const rootRef = useRef<HTMLDivElement>(null);

  // Bridge: subscribe to the plugin's `remargin:focus` event bus so a
  // widget click in the editor (T37 reading-mode / T38 Live Preview)
  // can scroll the matching sidebar card into view + briefly highlight
  // it. When the event names a different file, ask the parent to
  // switch the filter first — the card will mount on the next render
  // and the scroll runs in the next animation frame.
  // The `latestFile` ref keeps the listener stable across `activeFile`
  // changes; without it, every active-file flip would tear down and
  // re-attach the listener mid-render.
  const latestFile = useRef(activeFile);
  latestFile.current = activeFile;
  useEffect(() => {
    const target = plugin.focusEvents;
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<RemarginFocusDetail>).detail;
      if (!detail) return;
      const { commentId, file } = detail;
      const sameFile = latestFile.current === file;
      const focus = () => {
        const root = rootRef.current ?? (typeof document !== "undefined" ? document : null);
        if (!root) return;
        focusCardInRoot(root, commentId);
      };
      if (sameFile) {
        focus();
        return;
      }
      // Switch the filter first; defer the scroll so the new card has
      // a chance to mount under the updated filter. A microtask is the
      // smallest delay that reliably runs after React re-renders the
      // section list.
      onFocusFile?.(file);
      Promise.resolve().then(focus);
    };
    target.addEventListener("remargin:focus", handler);
    return () => {
      target.removeEventListener("remargin:focus", handler);
    };
  }, [plugin, onFocusFile]);

  return (
    <div ref={rootRef} className="flex flex-col h-full min-w-0 bg-bg-primary">
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

      {filterBar}

      <ScrollArea className="flex-1 min-w-0">
        <div className="flex flex-col min-w-0">
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
            <FilePathHeader
              plugin={plugin}
              filePath={activeFile}
              pendingCount={threadPending}
              refreshKey={refreshKey}
              onInitialized={onInitialized}
            />
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
