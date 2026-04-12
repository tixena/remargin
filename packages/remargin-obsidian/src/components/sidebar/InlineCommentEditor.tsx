import { Send, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useBackend } from "@/hooks/useBackend";

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
 * Mounted inside the file-named section of the sidebar (NOT as a modal — the
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
  const [content, setContent] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const handleSubmit = useCallback(async () => {
    if (!content.trim() || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await backend.comment(file, content.trim(), {
        afterLine,
        sandbox: true,
      });
      setContent("");
      onSubmitted(afterLine);
    } catch (err) {
      // Leave the draft in the textarea so the user can retry / copy the text.
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }, [backend, file, afterLine, content, submitting, onSubmitted]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        e.stopPropagation();
        e.nativeEvent.stopImmediatePropagation();
        void handleSubmit();
      }
      if (e.key === "Escape") {
        onClose();
      }
    },
    [handleSubmit, onClose]
  );

  return (
    <div className="flex flex-col gap-1.5 px-4 py-2 bg-bg-secondary border-y border-bg-border">
      <div className="flex items-center justify-between">
        <span className="text-[10px] text-text-faint">
          New comment after line {afterLine} in {file.split("/").pop()}
        </span>
        <Button variant="ghost" size="sm" className="h-5 w-5 p-0 text-text-faint" onClick={onClose}>
          <X className="w-3 h-3" />
        </Button>
      </div>
      <textarea
        ref={textareaRef}
        value={content}
        onChange={(e) => setContent(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Add a comment..."
        className="w-full min-h-[60px] p-2 text-xs font-mono bg-bg-primary border border-bg-border rounded-sm text-text-normal placeholder:text-text-faint resize-y focus:outline-none focus:ring-1 focus:ring-accent"
      />
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
          disabled={!content.trim() || submitting}
          onClick={handleSubmit}
        >
          <Send className="w-3 h-3 mr-1" />
          {submitting ? "Sending..." : "Comment"}
        </Button>
      </div>
    </div>
  );
}
