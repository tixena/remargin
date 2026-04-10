import { type App, TFile } from "obsidian";

type Action = () => Promise<void>;

/**
 * Serializes file-mutating actions to prevent race conditions.
 *
 * Before each action: saves the editor buffer to disk.
 * After each action: reloads the file from disk into the editor.
 */
export class ActionQueue {
  private queue: Action[] = [];
  private running = false;

  constructor(private app: App) {}

  /**
   * Enqueue an action that mutates a file via the CLI.
   * The action is wrapped with save-before and reload-after.
   */
  async enqueue(filePath: string, action: () => Promise<void>): Promise<void> {
    return new Promise((resolve, reject) => {
      this.queue.push(async () => {
        try {
          await this.saveFile(filePath);
          await action();
          await this.reloadFile(filePath);
          resolve();
        } catch (err) {
          reject(err);
        }
      });
      this.drain();
    });
  }

  private async drain(): Promise<void> {
    if (this.running) return;
    this.running = true;
    try {
      while (this.queue.length > 0) {
        const next = this.queue.shift()!;
        await next();
      }
    } finally {
      this.running = false;
    }
  }

  private async saveFile(filePath: string): Promise<void> {
    // Force save active editor if it matches the target file
    const leaf = this.app.workspace.activeLeaf;
    if (!leaf) return;
    const view = leaf.view;
    if ("save" in view && typeof view.save === "function") {
      const file = this.app.workspace.getActiveFile();
      if (file && file.path === filePath) {
        await (view as any).save();
      }
    }
  }

  private async reloadFile(filePath: string): Promise<void> {
    const file = this.app.vault.getAbstractFileByPath(filePath);
    if (!(file instanceof TFile)) return;

    // Read fresh content from disk
    const content = await this.app.vault.read(file);

    // Find and update any editor showing this file
    for (const leaf of this.app.workspace.getLeavesOfType("markdown")) {
      const state = leaf.getViewState();
      if (state.state?.file === filePath) {
        const view = leaf.view;
        if ("editor" in view && view.editor) {
          const editor = (view as any).editor;
          const cursor = editor.getCursor();
          await this.app.vault.modify(file, content);
          // Restore cursor (clamped to new doc length)
          try {
            const lineCount = editor.lineCount();
            const line = Math.min(cursor.line, lineCount - 1);
            const ch = Math.min(cursor.ch, editor.getLine(line)?.length ?? 0);
            editor.setCursor({ line, ch });
          } catch {
            // cursor restoration is best-effort
          }
          return;
        }
      }
    }

    // No open editor — just trigger a vault cache refresh
    await this.app.vault.modify(file, content);
  }
}
