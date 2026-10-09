/** The sidebar's frame: header toolbar, collapsible sections and the scroll viewport. */

import { Inbox, Mail } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { ReMarginLogo } from "@/components/icons/ReMarginLogo";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { ScrollArea } from "@/components/ui/scroll-area";
import type RemarginPlugin from "@/main";
import type { RemarginFocusDetail } from "@/main";
import { FilePathHeader } from "./FilePathHeader";
import { FontScaleControl } from "./FontScaleControl";
import { focusCardInRoot } from "./focusCard";
import { SectionHeader } from "./SectionHeader";

/** Props for {@link SidebarShell}. */
interface SidebarShellProps {
  plugin: RemarginPlugin;
  activeFile?: string;
  /** Called when a focus request targets another file, so the parent can switch to it first. */
  onFocusFile?: (file: string) => void;
  sandboxCount?: number;
  inboxCount?: number;
  threadPending?: number;
  refreshKey?: number;
  onInitialized?: () => void;
  onPlusClick?: () => void;
  onRefreshClick?: () => void;
  sandboxContent?: React.ReactNode;
  sandboxActions?: React.ReactNode;
  inboxContent?: React.ReactNode;
  inboxActions?: React.ReactNode;
  threadContent?: React.ReactNode;
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
  sandboxContent,
  sandboxActions,
  inboxContent,
  inboxActions,
  threadContent,
  threadInlineEditor,
  footerContent,
}: SidebarShellProps) {
  const [sandboxOpen, setSandboxOpen] = useState(true);
  const [inboxOpen, setInboxOpen] = useState(true);
  const [threadOpen, setThreadOpen] = useState(true);
  const rootRef = useRef<HTMLDivElement>(null);

  // Subscribes to the plugin's `remargin:focus` bus so a widget click in the editor scrolls to
  // the matching card. The `latestFile` ref keeps the listener stable across `activeFile` changes.
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
      // The filter switches first; the scroll is deferred so the new card can mount under it.
      onFocusFile?.(file);
      Promise.resolve().then(focus);
    };
    target.addEventListener("remargin:focus", handler);
    return () => {
      target.removeEventListener("remargin:focus", handler);
    };
  }, [plugin, onFocusFile]);

  // Obsidian's hotkey scope intercepts Ctrl+C / Cmd+C, so a capture-phase listener copies the
  // selection itself, and only when the selection lives inside the sidebar root.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const doc = root.ownerDocument ?? document;
    const onKeyDown = (event: KeyboardEvent) => {
      const isCopy =
        (event.ctrlKey || event.metaKey) &&
        !event.shiftKey &&
        !event.altKey &&
        (event.key === "c" || event.key === "C");
      if (!isCopy) return;
      const selection = doc.getSelection();
      if (!selection || selection.isCollapsed) return;
      const anchorNode = selection.anchorNode;
      if (!anchorNode || !root.contains(anchorNode)) return;
      const text = selection.toString();
      if (!text) return;
      event.preventDefault();
      event.stopPropagation();
      void navigator.clipboard.writeText(text).catch((err) => {
        console.error("Remargin sidebar copy failed:", err);
      });
    };
    doc.addEventListener("keydown", onKeyDown, true);
    return () => {
      doc.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);

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
        <div className="flex items-center gap-2 shrink-0">
          <FontScaleControl />
          <button
            type="button"
            onClick={onPlusClick}
            aria-label="New comment at cursor"
            title="New comment at cursor"
            style={{
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              height: "var(--input-height)",
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
      </div>

      {/* `min-h-0` is load-bearing: without it `flex-1` never caps this height and the whole
          Obsidian pane scrolls instead of the panel. `min-w-0` is the same fix on the other axis. */}
      <ScrollArea className="flex-1 min-h-0 min-w-0">
        <div className="flex flex-col min-w-0">
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
