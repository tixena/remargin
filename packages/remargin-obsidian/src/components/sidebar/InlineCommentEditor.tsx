import type { EditorView } from "@codemirror/view";
import { Send, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { createCommentEditor } from "@/editor/commentEditor";
import { useBackend } from "@/hooks/useBackend";

function noop(): void {
  /* intentionally empty */
}

interface InlineCommentEditorProps {
  /** Vault-relative path of the file the new comment targets. */
  file: string;
  /**
   * 1-indexed line number where the comment should be inserted. Callers are
   * expected to have already run `snapAfterCommentBlock` so this is always a
   * legal insert point for `remargin comment --after-line`.
   */
  afterLine: number;
  /** Called when the user cancels or after a successful submit. */
  onClose: () => void;
  /**
   * Invoked after the CLI returns successfully. Receives the 1-indexed line
   * the new comment was inserted at, so the caller can scroll the editor to
   * it and fire a sidepanel refresh.
   */
  onSubmitted: (insertedLine: number) => void;
}

/**
 * Inline composer for the sidebar `+` button flow.
 *
 * Mounted inside the file-named section of the sidebar (NOT as a modal --- the
 * "no dialogs pls" rule from prior design feedback). The single Submit action
 * issues `remargin comment --after-line <N> --sandbox` so the comment and the
 * sandbox entry are written in one atomic CLI call.
 */
export function InlineCommentEditor({
  file,
  afterLine,
  onClose,
  onSubmitted,
}: InlineCommentEditorProps) {
  const backend = useBackend();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasContent, setHasContent] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const submitRef = useRef<() => void>(noop);
  const closeRef = useRef<() => void>(noop);

  const handleSubmit = useCallback(async () => {
    const content = viewRef.current?.state.doc.toString().trim() ?? "";
    if (!content || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await backend.comment(file, content, {
        afterLine,
        sandbox: true,
      });
      onSubmitted(afterLine);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }, [backend, file, afterLine, submitting, onSubmitted]);

  // Keep refs in sync so the CM6 keymap closures always call the
  // latest versions of handleSubmit/onClose.
  submitRef.current = () => void handleSubmit();
  closeRef.current = onClose;

  useEffect(() => {
    if (editorRef.current && !viewRef.current) {
      viewRef.current = createCommentEditor({
        parent: editorRef.current,
        placeholder: "Add a comment...",
        onSubmit: () => submitRef.current(),
        onCancel: () => closeRef.current(),
        onDocLength: (len) => setHasContent(len > 0),
      });
      // Bring the composer into view and focus the editor so the user can
      // start typing immediately. Without this the user often doesn't
      // notice anything happened, because the composer renders at the
      // bottom of a long sidebar below the fold.
      containerRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
      viewRef.current.focus();
    }
    return () => {
      viewRef.current?.destroy();
      viewRef.current = null;
    };
  }, []);

  return (
    <div
      ref={containerRef}
      className="flex flex-col gap-1.5 px-4 py-2 bg-bg-secondary border-y border-bg-border"
    >
      <div className="flex items-center justify-between">
        <span className="text-[10px] text-text-faint">
          New comment after line {afterLine} in {file.split("/").pop()}
        </span>
        <Button variant="ghost" size="sm" className="h-5 w-5 p-0 text-text-faint" onClick={onClose}>
          <X className="w-3 h-3" />
        </Button>
      </div>
      <div ref={editorRef} />
      {error && (
        <div className="text-[10px] text-red-400 font-mono whitespace-pre-wrap break-words">
          {error}
        </div>
      )}
      <div className="flex items-center justify-between">
        <span className="text-[9px] text-text-faint">Ctrl+Enter to comment</span>
        <Button
          size="sm"
          className="h-6 px-2 text-[10px] bg-accent text-white hover:bg-accent-hover"
          disabled={!hasContent || submitting}
          onClick={() => void handleSubmit()}
        >
          <Send className="w-3 h-3 mr-1" />
          {submitting ? "Sending..." : "Comment"}
        </Button>
      </div>
    </div>
  );
}
