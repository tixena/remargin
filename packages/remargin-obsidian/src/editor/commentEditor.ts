/** The CodeMirror editor used by the inline composers. */

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

/** What {@link createCommentEditor} needs: the mount point, placeholder and callbacks. */
export interface CommentEditorConfig {
  parent: HTMLElement;
  placeholder: string;
  onSubmit: () => void;
  onCancel: () => void;
  onDocLength?: (length: number) => void;
}

/**
 * Create a CodeMirror 6 editor configured for writing comment content.
 *
 * Features:
 * - Markdown syntax highlighting
 * - Obsidian-native theme via CSS custom properties
 * - Mod-Enter to submit, Escape to cancel. CM6's keymap facet registers
 *   handlers on the editor's contentDOM, which fire in the at-target phase
 *   AFTER Obsidian's document-level capture-phase hotkey dispatcher. To
 *   actually win the race we attach a `window`-capture keydown listener
 *   scoped to the editor's DOM — window capture runs before document
 *   capture, so Obsidian never sees the event.
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
        // Fallback for events dispatched straight on the contentDOM, which bypass the window listener.
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

  // Window capture runs before Obsidian's document-capture hotkey dispatcher, which would
  // swallow Mod-Enter. Gated on `view.dom.contains(target)` so other keystrokes pass untouched.
  const onKeyDownCapture = (event: KeyboardEvent) => {
    const target = event.target;
    if (!(target instanceof Node) || !view.dom.contains(target)) return;
    const isModEnter = event.key === "Enter" && (event.ctrlKey || event.metaKey);
    const isEscape = event.key === "Escape";
    if (!isModEnter && !isEscape) return;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
    if (isModEnter) {
      config.onSubmit();
    } else {
      config.onCancel();
    }
  };
  window.addEventListener("keydown", onKeyDownCapture, true);

  // Removed when CM6 destroys the view, which ties the listener's lifetime to it.
  const originalDestroy = view.destroy.bind(view);
  view.destroy = () => {
    window.removeEventListener("keydown", onKeyDownCapture, true);
    originalDestroy();
  };

  view.focus();
  return view;
}
