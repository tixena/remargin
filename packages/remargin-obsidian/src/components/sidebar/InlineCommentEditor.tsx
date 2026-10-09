/** The inline composer for a new comment. */

import type { EditorView } from "@codemirror/view";
import { Send, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { RecipientPicker } from "@/components/sidebar/RecipientPicker";
import { Button } from "@/components/ui/button";
import { createCommentEditor } from "@/editor/commentEditor";
import { useBackend } from "@/hooks/useBackend";

function noop(): void {
  /* intentionally empty */
}

/** Props for {@link InlineCommentEditor}. */
interface InlineCommentEditorProps {
  file: string;
  /** 1-indexed, and already snapped by `snapAfterCommentBlock` to a legal insert point. */
  afterLine: number;
  onClose: () => void;
  /** Receives the 1-indexed line the new comment was inserted at. */
  onSubmitted: (insertedLine: number) => void;
}

/**
 * Inline composer for the sidebar `+` button flow, mounted inside the file-named section of
 * the sidebar, not as a modal. The single Submit action issues
 * `remargin comment --after-line <N> --sandbox` so the comment and the sandbox entry are
 * written in one atomic CLI call.
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
  const [to, setTo] = useState<string[]>([]);
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
        to,
      });
      onSubmitted(afterLine);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }, [backend, file, afterLine, submitting, onSubmitted, to]);

  // Refs, so the CM6 keymap closures always call the latest handleSubmit / onClose.
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
      // The composer can render below the fold of a long sidebar, so it is scrolled to and focused.
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
      <RecipientPicker selected={to} onChange={setTo} />
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
