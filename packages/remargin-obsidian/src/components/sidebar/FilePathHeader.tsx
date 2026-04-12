import { Check, Copy, FileText } from "lucide-react";
import { TFile } from "obsidian";
import { useCallback, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useContainerWidth } from "@/hooks/useContainerWidth";
import { abbreviatePath } from "@/lib/abbreviatePath";
import type RemarginPlugin from "@/main";

interface FilePathHeaderProps {
  plugin: RemarginPlugin;
  filePath?: string;
  /** Pending comment count badge. */
  pendingCount?: number;
}

/** Average character width in px for the 10px monospace font used in the header. */
const CHAR_WIDTH_PX = 6;
/** Horizontal padding + icon + gaps that reduce usable text width. */
const RESERVED_PX = 60;

/**
 * Two-line file section header:
 * - Top: abbreviated folder path (auto-recalculates on resize)
 * - Bottom: frontmatter title or filename
 * - Tooltip: full vault-relative path
 * - Copy icon: copies full path to clipboard
 */
export function FilePathHeader({ plugin, filePath, pendingCount }: FilePathHeaderProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const containerWidth = useContainerWidth(containerRef);
  const [copied, setCopied] = useState(false);

  // Derive directory path and filename/title.
  const dirPath = useMemo(() => {
    if (!filePath) return "";
    const lastSlash = filePath.lastIndexOf("/");
    return lastSlash >= 0 ? filePath.slice(0, lastSlash) : "";
  }, [filePath]);

  const title = useMemo(() => {
    if (!filePath) return "No file open";
    const file = plugin.app.vault.getAbstractFileByPath(filePath);
    if (file instanceof TFile) {
      const cache = plugin.app.metadataCache.getFileCache(file);
      const fmTitle = cache?.frontmatter?.title;
      if (typeof fmTitle === "string" && fmTitle.trim()) {
        return fmTitle.trim();
      }
    }
    // Fallback to filename without extension.
    const basename = filePath.split("/").pop() ?? filePath;
    const dotIdx = basename.lastIndexOf(".");
    return dotIdx > 0 ? basename.slice(0, dotIdx) : basename;
  }, [plugin, filePath]);

  // Abbreviate the directory path based on available width.
  const maxChars = Math.max(8, Math.floor((containerWidth - RESERVED_PX) / CHAR_WIDTH_PX));
  const displayDir = useMemo(() => abbreviatePath(dirPath, maxChars), [dirPath, maxChars]);

  const handleCopy = useCallback(async () => {
    if (!filePath) return;
    try {
      await navigator.clipboard.writeText(filePath);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API may fail in some contexts; degrade silently.
    }
  }, [filePath]);

  return (
    <div
      ref={containerRef}
      className="flex items-start gap-2 px-4 py-2 bg-bg-border border-t border-bg-border"
      title={filePath ?? "No file open"}
    >
      <FileText className="w-3.5 h-3.5 text-text-faint mt-0.5 shrink-0" />
      <div className="flex flex-col min-w-0 flex-1">
        {displayDir && (
          <span className="font-mono text-[10px] text-text-faint truncate leading-tight">
            {displayDir}
          </span>
        )}
        <span className="font-sans text-xs text-text-muted truncate leading-tight font-medium">
          {title}
        </span>
      </div>
      <div className="flex items-center gap-1 shrink-0">
        {pendingCount != null && pendingCount > 0 && (
          <span className="px-1.5 py-0 text-[9px] font-semibold leading-4 rounded-full bg-amber-400 text-bg-primary whitespace-nowrap">
            {pendingCount} pending
          </span>
        )}
        {filePath && (
          <Button
            variant="ghost"
            size="icon"
            className="w-5 h-5 p-0 text-text-faint hover:text-text-muted"
            onClick={handleCopy}
            aria-label="Copy file path"
            title="Copy file path"
          >
            {copied ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />}
          </Button>
        )}
      </div>
    </div>
  );
}
