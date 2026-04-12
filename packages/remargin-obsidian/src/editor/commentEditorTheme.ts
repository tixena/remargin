import { EditorView } from "@codemirror/view";

/**
 * Obsidian-native theme for the comment editor. Uses CSS custom properties
 * from Obsidian's theme engine so the editor adapts to light/dark mode and
 * user-installed themes automatically.
 */
export const commentEditorTheme = EditorView.theme({
  "&": {
    backgroundColor: "var(--background-primary)",
    color: "var(--text-normal)",
    fontSize: "12px",
    fontFamily: "var(--font-monospace)",
    minHeight: "60px",
    border: "1px solid var(--background-modifier-border)",
    borderRadius: "4px",
    padding: "8px",
  },
  "&.cm-focused": {
    outline: "none",
    boxShadow: "0 0 0 1px var(--interactive-accent)",
  },
  ".cm-content": {
    caretColor: "var(--text-normal)",
    padding: "0",
  },
  ".cm-placeholder": {
    color: "var(--text-faint)",
  },
  ".cm-line": {
    padding: "0",
  },
  ".cm-cursor": {
    borderLeftColor: "var(--text-normal)",
  },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
    backgroundColor: "var(--text-selection)",
  },
  ".cm-scroller": {
    overflow: "auto",
  },
});
