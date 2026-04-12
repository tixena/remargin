import { MarkdownView, TFile } from "obsidian";
import type RemarginPlugin from "@/main";

/**
 * Open a file in the current leaf and optionally scroll to a specific line.
 *
 * - Targets the last-known markdown leaf (via `getLastMarkdownView()`) so
 *   clicking a sidebar comment navigates the editor, not the sidebar pane.
 *   Falls back to any open markdown leaf, then to `getLeaf(false)`.
 * - `line` is expected to be **1-indexed** (as stored by the remargin parser).
 *   Obsidian's editor API is 0-indexed, so we subtract one before calling
 *   `setCursor`/`scrollIntoView`.
 * - Waits one animation frame + 50 ms after opening the file so Obsidian's
 *   vault watcher and editor buffer refresh complete before scrolling.
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

  await leaf.openFile(file);

  if (line && line > 0) {
    // 2. Wait for Obsidian's vault watcher + editor buffer refresh.
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => setTimeout(resolve, 50));
    });

    // 3. If the view is in reading mode, switch to source (live preview) so
    //    the editor API is available for cursor placement and scrolling.
    if (leaf.view instanceof MarkdownView) {
      const state = leaf.view.getState();
      if (state.mode === "preview") {
        await leaf.view.setState({ ...state, mode: "source" }, { history: false });
        // Give Obsidian a tick to finish the mode switch.
        await new Promise<void>((r) => setTimeout(r, 50));
      }
    }

    // 4. Scroll after the buffer has settled.
    if (leaf.view instanceof MarkdownView && leaf.view.editor) {
      const pos = { line: line - 1, ch: 0 };
      leaf.view.editor.setCursor(pos);
      leaf.view.editor.scrollIntoView({ from: pos, to: pos }, true);
    }
  }
}
