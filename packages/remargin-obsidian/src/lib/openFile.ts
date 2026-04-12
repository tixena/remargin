import { MarkdownView, TFile } from "obsidian";
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

  // 1. Target the last-known markdown leaf, not the active (sidebar) leaf.
  const lastView = plugin.getLastMarkdownView();
  let leaf = lastView?.leaf ?? null;
  if (!leaf || !(leaf.view instanceof MarkdownView)) {
    const leaves = plugin.app.workspace.getLeavesOfType("markdown");
    leaf = leaves[0] ?? plugin.app.workspace.getLeaf(false);
  }

  // Detect whether we are switching files (cross-file navigation) so we can
  // use a longer settle delay below. Reading the current file path BEFORE
  // openFile is the only reliable moment -- afterwards the leaf already
  // references the new file.
  const currentPath =
    leaf.view instanceof MarkdownView ? leaf.view.file?.path : undefined;
  const isCrossFile = currentPath !== filePath;

  await leaf.openFile(file);

  if (line && line > 0) {
    // 2. Wait for the editor buffer to initialise. A cross-file switch needs
    //    more time than a same-file scroll because Obsidian tears down the old
    //    CodeMirror state and builds a new one for the target file.
    const settleMs = isCrossFile ? 200 : 50;
    await rafDelay(settleMs);

    // Re-read the view from the leaf -- after openFile the MarkdownView
    // instance may have been replaced (Obsidian can create a new view for the
    // new file).
    const view = leaf.view instanceof MarkdownView ? leaf.view : null;

    // 3. If the view is in reading mode, switch to source (live preview) so
    //    the editor API is available for cursor placement and scrolling.
    if (view) {
      const state = view.getState();
      if (state.mode === "preview") {
        await view.setState({ ...state, mode: "source" }, { history: false });
        // Give Obsidian a tick to finish the mode switch.
        await rafDelay(100);
      }
    }

    // 4. Scroll after the buffer has settled.  Re-read the view one more time
    //    in case the mode switch replaced it.
    const scrollView = leaf.view instanceof MarkdownView ? leaf.view : null;
    if (scrollView?.editor) {
      const pos = { line: line - 1, ch: 0 };
      scrollView.editor.setCursor(pos);
      scrollView.editor.scrollIntoView({ from: pos, to: pos }, true);
    }
  }
}
