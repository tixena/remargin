import { Check, Copy, FileText, Wand2 } from "lucide-react";
import { Notice, TFile } from "obsidian";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useContainerWidth } from "@/hooks/useContainerWidth";
import { abbreviatePath } from "@/lib/abbreviatePath";
import { extractTitle } from "@/lib/file-title";
import { hasRemarginFrontmatter } from "@/lib/hasRemarginFrontmatter";
import type RemarginPlugin from "@/main";

interface FilePathHeaderProps {
  plugin: RemarginPlugin;
  filePath?: string;
  /** Pending comment count badge. */
  pendingCount?: number;
  /**
   * Monotonically bumped by the sidebar shell on any mutation. Used by
   * the 'Initialize' flow (rem-rvk6) so the file contents re-read after
   * `remargin write` injects frontmatter — the button then disappears
   * because `hasRemarginFrontmatter` now returns true.
   */
  refreshKey?: number;
  /**
   * Called after a successful initialize so the rest of the sidebar
   * (thread list, inbox, sandbox) refreshes against the newly-managed
   * file.
   */
  onInitialized?: () => void;
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
export function FilePathHeader({
  plugin,
  filePath,
  pendingCount,
  refreshKey,
  onInitialized,
}: FilePathHeaderProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const containerWidth = useContainerWidth(containerRef);
  const [copied, setCopied] = useState(false);
  const [initializing, setInitializing] = useState(false);

  // Derive directory path and filename/title.
  const dirPath = useMemo(() => {
    if (!filePath) return "";
    const lastSlash = filePath.lastIndexOf("/");
    return lastSlash >= 0 ? filePath.slice(0, lastSlash) : "";
  }, [filePath]);

  const [fileContents, setFileContents] = useState<string>("");

  // Re-read the file contents whenever the active path changes so the
  // title can follow H1 edits. Uses cachedRead (non-blocking, metadata-
  // cache-backed) to avoid hammering the vault on every render.
  // `refreshKey` bumps force a re-read after any sidebar mutation —
  // specifically so the 'Initialize' button (rem-rvk6) disappears once
  // frontmatter has been injected.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshKey is a trigger-only dep; bumping it must re-run the read (rem-rvk6).
  useEffect(() => {
    let cancelled = false;
    setFileContents("");
    if (!filePath) return;
    const file = plugin.app.vault.getAbstractFileByPath(filePath);
    if (!(file instanceof TFile)) return;
    plugin.app.vault
      .cachedRead(file)
      .then((contents) => {
        if (!cancelled) setFileContents(contents);
      })
      .catch(() => {
        // Best-effort: title falls back to the filename stem if the read
        // fails.
      });
    return () => {
      cancelled = true;
    };
  }, [plugin, filePath, refreshKey]);

  const title = useMemo(() => {
    if (!filePath) return "No file open";
    return extractTitle(fileContents, filePath);
  }, [filePath, fileContents]);

  /**
   * Bare .md files (no remargin frontmatter) get an 'Initialize'
   * affordance so users can one-click them into the managed tree
   * (rem-rvk6). The check runs off the already-read `fileContents`,
   * so it costs nothing beyond the existing title read. Files with
   * non-markdown extensions never qualify — they're not documents.
   */
  const isBareMarkdown = useMemo(() => {
    if (!filePath || !filePath.toLowerCase().endsWith(".md")) return false;
    // Treat the initial empty string (pre-first-read) as "unknown" and
    // suppress the button until we actually have contents; otherwise
    // the button would flicker in for every file open.
    if (fileContents === "") return false;
    return !hasRemarginFrontmatter(fileContents);
  }, [filePath, fileContents]);

  const handleInitialize = useCallback(async () => {
    if (!filePath || initializing) return;
    setInitializing(true);
    try {
      // `remargin write <path> <current-contents>` triggers the
      // frontmatter-injection pass that every managed file already
      // runs through (rem-is4z / rem-rvk6). No special subcommand:
      // the write is a no-op on the body but a full canonicalization
      // on the frontmatter.
      await plugin.backend.write(filePath, fileContents);
      onInitialized?.();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      new Notice(`Remargin: initialize failed — ${message}`);
    } finally {
      setInitializing(false);
    }
  }, [filePath, fileContents, initializing, plugin, onInitialized]);

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
      className="flex items-start gap-2 px-4 py-2 min-w-0 bg-bg-border border-t border-bg-border overflow-hidden"
      title={filePath ?? "No file open"}
    >
      <FileText className="w-3.5 h-3.5 text-text-faint mt-0.5 shrink-0" />
      <div className="flex flex-col min-w-0 flex-1">
        {displayDir && (
          <span className="font-mono text-[10px] text-accent truncate leading-tight">
            {displayDir}
          </span>
        )}
        <span className="font-sans text-xs text-text-normal truncate leading-tight font-medium">
          {title}
        </span>
      </div>
      <div className="flex items-center gap-1 shrink-0">
        {isBareMarkdown && (
          <Button
            variant="ghost"
            size="sm"
            className="h-5 px-2 text-[10px] font-semibold text-accent gap-1"
            onClick={handleInitialize}
            disabled={initializing}
            aria-label="Initialize remargin frontmatter for this file"
            title="Inject canonical remargin frontmatter so the file becomes a managed document."
          >
            <Wand2 className="w-3 h-3" />
            {initializing ? "Initializing..." : "Initialize"}
          </Button>
        )}
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
