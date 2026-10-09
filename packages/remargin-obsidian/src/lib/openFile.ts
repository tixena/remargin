/** Opens a vault file in the editor and scrolls to a line. */

import { MarkdownView, Notice, normalizePath, TFile } from "obsidian";
import type RemarginPlugin from "@/main";

/**
 * Delay that waits for one animation frame followed by a timeout.
 * Used to let Obsidian finish vault-watcher and editor-buffer work.
 */
function rafDelay(ms: number): Promise<void> {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => setTimeout(resolve, ms));
  });
}

/**
 * Open a file in the current leaf and optionally scroll to a specific line.
 *
 * - Targets the last-known markdown leaf (via `getLastMarkdownView()`) so
 *   clicking a sidebar comment navigates the editor, not the sidebar pane.
 *   Falls back to any open markdown leaf, then to `getLeaf(false)`.
 * - `line` is expected to be **1-indexed** (as stored by the remargin parser).
 *   Obsidian's editor API is 0-indexed, so we subtract one before calling
 *   `setCursor`/`scrollIntoView`.
 * - Detects cross-file navigation (opening a file different from the leaf's
 *   current file) and uses a longer settle delay so the new editor buffer is
 *   fully initialised before scrolling.
 * - Normalises `filePath` with Obsidian's `normalizePath` before the vault
 *   lookup to handle OS path separators and leading `./` that the CLI may
 *   produce on some platforms.
 * - If the path does not resolve to a `TFile` (e.g. the file was deleted or
 *   renamed), shows a Notice, logs an error, and returns without throwing.
 * - If the opened view is not a `MarkdownView` (e.g. PDF, image), the file is
 *   still opened but the scroll step is skipped.
 */
export async function openFileAtLine(
  plugin: RemarginPlugin,
  filePath: string,
  line?: number
): Promise<void> {
  const rel = normalizePath(filePath);
  const file = plugin.app.vault.getAbstractFileByPath(rel);
  if (!(file instanceof TFile)) {
    console.error(`remargin: file not found in vault: ${rel}`);
    new Notice(`Remargin: couldn't open ${rel} (not found in vault)`);
    return;
  }

  const lastView = plugin.getLastMarkdownView();
  let leaf = lastView?.leaf ?? null;
  if (!leaf || !(leaf.view instanceof MarkdownView)) {
    const leaves = plugin.app.workspace.getLeavesOfType("markdown");
    leaf = leaves[0] ?? plugin.app.workspace.getLeaf(false);
  }

  // Read before `openFile`: afterwards the leaf already references the new file.
  const currentPath =
    leaf.view instanceof MarkdownView ? leaf.view.file?.path : undefined;
  const isCrossFile = currentPath !== rel;

  await leaf.openFile(file);
  plugin.app.workspace.revealLeaf(leaf);

  if (line && line > 0) {
    // A cross-file switch needs longer: Obsidian rebuilds the CodeMirror state for the new file.
    const settleMs = isCrossFile ? 200 : 50;
    await rafDelay(settleMs);

    // Re-read from the leaf: `openFile` may have replaced the MarkdownView instance.
    const view = leaf.view instanceof MarkdownView ? leaf.view : null;

    // Reading mode has no editor API, so switch to source mode for cursor placement and scrolling.
    if (view) {
      const state = view.getState();
      if (state.mode === "preview") {
        await view.setState({ ...state, mode: "source" }, { history: false });
        await rafDelay(100);
      }
    }

    const scrollView = leaf.view instanceof MarkdownView ? leaf.view : null;
    if (scrollView?.editor) {
      const pos = { line: line - 1, ch: 0 };
      scrollView.editor.setCursor(pos);
      scrollView.editor.scrollIntoView({ from: pos, to: pos }, true);
    }
  }
}
