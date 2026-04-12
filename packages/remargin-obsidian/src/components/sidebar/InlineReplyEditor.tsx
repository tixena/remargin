import type { EditorView } from "@codemirror/view";
import { Send, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { createCommentEditor } from "@/editor/commentEditor";
import { useBackend } from "@/hooks/useBackend";

function noop(): void {
  /* intentionally empty */
}

interface InlineReplyEditorProps {
  file: string;
  replyTo: string;
  onClose: () => void;
  onSubmitted: () => void;
}

export function InlineReplyEditor({ file, replyTo, onClose, onSubmitted }: InlineReplyEditorProps) {
  const backend = useBackend();
  const [submitting, setSubmitting] = useState(false);
  const [hasContent, setHasContent] = useState(false);
  const editorRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const submitRef = useRef<() => void>(noop);
  const closeRef = useRef<() => void>(noop);

  const handleSubmit = useCallback(async () => {
    const content = viewRef.current?.state.doc.toString().trim() ?? "";
    if (!content || submitting) return;
    setSubmitting(true);
    try {
      await backend.comment(file, content, {
        replyTo,
        autoAck: true,
        sandbox: true,
      });
      onSubmitted();
    } catch {
      // no-op
    } finally {
      setSubmitting(false);
    }
  }, [backend, file, replyTo, submitting, onSubmitted]);

  submitRef.current = () => void handleSubmit();
  closeRef.current = onClose;

  useEffect(() => {
    if (editorRef.current && !viewRef.current) {
      viewRef.current = createCommentEditor({
        parent: editorRef.current,
        placeholder: "Write a reply...",
        onSubmit: () => submitRef.current(),
        onCancel: () => closeRef.current(),
        onDocLength: (len) => setHasContent(len > 0),
      });
    }
    return () => {
      viewRef.current?.destroy();
      viewRef.current = null;
    };
  }, []);

  return (
    <div className="flex flex-col gap-1.5 px-4 py-2 bg-bg-secondary border-y border-bg-border">
      <div className="flex items-center justify-between">
        <span className="text-[10px] text-text-faint">Replying to {replyTo}</span>
        <Button variant="ghost" size="sm" className="h-5 w-5 p-0 text-text-faint" onClick={onClose}>
          <X className="w-3 h-3" />
        </Button>
      </div>
      <div ref={editorRef} />
      <div className="flex items-center justify-between">
        <span className="text-[9px] text-text-faint">Ctrl+Enter to send</span>
        <Button
          size="sm"
          className="h-6 px-2 text-[10px] bg-accent text-white hover:bg-accent-hover"
          disabled={!hasContent || submitting}
          onClick={() => void handleSubmit()}
        >
          <Send className="w-3 h-3 mr-1" />
          {submitting ? "Sending..." : "Send"}
        </Button>
      </div>
    </div>
  );
}
