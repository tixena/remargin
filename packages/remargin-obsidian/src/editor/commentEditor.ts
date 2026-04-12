import { markdown } from "@codemirror/lang-markdown";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightSpecialChars,
  keymap,
  placeholder as placeholderExt,
} from "@codemirror/view";
import { commentEditorTheme } from "./commentEditorTheme";

export interface CommentEditorConfig {
  parent: HTMLElement;
  placeholder: string;
  onSubmit: () => void;
  onCancel: () => void;
  /** Called on every document change with the current document length. */
  onDocLength?: (length: number) => void;
}

/**
 * Create a CodeMirror 6 editor configured for writing comment content.
 *
 * Features:
 * - Markdown syntax highlighting
 * - Obsidian-native theme via CSS custom properties
 * - Mod-Enter to submit, Escape to cancel (via CM6 keymap -- bypasses
 *   Obsidian's document-level capture-phase listeners)
 * - Line wrapping, placeholder text, auto-focus
 */
export function createCommentEditor(config: CommentEditorConfig): EditorView {
  const view = new EditorView({
    state: EditorState.create({
      doc: "",
      extensions: [
        highlightSpecialChars(),
        drawSelection(),
        EditorState.allowMultipleSelections.of(true),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        markdown(),
        commentEditorTheme,
        EditorView.lineWrapping,
        placeholderExt(config.placeholder),
        keymap.of([
          {
            key: "Mod-Enter",
            run: () => {
              config.onSubmit();
              return true;
            },
          },
          {
            key: "Escape",
            run: () => {
              config.onCancel();
              return true;
            },
          },
        ]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged && config.onDocLength) {
            config.onDocLength(update.state.doc.length);
          }
        }),
      ],
    }),
    parent: config.parent,
  });

  view.focus();
  return view;
}
