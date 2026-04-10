import { MarkdownView, TFile } from "obsidian";
import type RemarginPlugin from "@/main";

/**
 * Open a file in the current leaf and optionally scroll to a specific line.
 *
 * - Reuses the currently-active leaf (`getLeaf(false)`) so repeated clicks
 *   do not keep spawning new panes.
 * - `line` is expected to be **1-indexed** (as stored by the remargin parser).
 *   Obsidian's editor API is 0-indexed, so we subtract one before calling
 *   `setCursor`/`scrollIntoView`.
 * - If the path does not resolve to a `TFile` (e.g. the file was deleted or
 *   renamed), logs an error and returns without throwing.
 * - If the opened view is not a `MarkdownView` (e.g. PDF, image), the file is
 *   still opened but the scroll step is skipped.
 */
export async function openFileAtLine(
  plugin: RemarginPlugin,
  filePath: string,
  line?: number
): Promise<void> {
  const file = plugin.app.vault.getAbstractFileByPath(filePath);
  if (!(file instanceof TFile)) {
    console.error(`remargin: file not found in vault: ${filePath}`);
    return;
  }

  const leaf = plugin.app.workspace.getLeaf(false);
  await leaf.openFile(file);

  if (line && line > 0 && leaf.view instanceof MarkdownView && leaf.view.editor) {
    // remargin line numbers are 1-indexed; Obsidian's editor is 0-indexed.
    const pos = { line: line - 1, ch: 0 };
    leaf.view.editor.setCursor(pos);
    leaf.view.editor.scrollIntoView({ from: pos, to: pos }, true);
  }
}
