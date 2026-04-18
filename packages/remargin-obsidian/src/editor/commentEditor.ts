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
        // Kept as a fallback for environments where the window-capture
        // interceptor below is bypassed (e.g. synthetic events dispatched
        // directly on the contentDOM). The interceptor handles the common
        // case of a real user keystroke.
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

  // Window-capture keydown interceptor. Runs in the capture phase at the
  // topmost propagation target, so it beats Obsidian's document-capture
  // hotkey dispatcher (which otherwise swallows Mod-Enter for the
  // global "Toggle ..." bindings). We gate on `view.dom.contains(target)`
  // so keystrokes outside the composer stay untouched.
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

  // Tear down the window listener when CM6 destroys the view. `destroy` is
  // called by consumers in their unmount path; piggybacking on it keeps the
  // lifetime of the listener tied to the view without an extra API.
  const originalDestroy = view.destroy.bind(view);
  view.destroy = () => {
    window.removeEventListener("keydown", onKeyDownCapture, true);
    originalDestroy();
  };

  view.focus();
  return view;
}
