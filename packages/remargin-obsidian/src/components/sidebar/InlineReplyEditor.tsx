import type { EditorView } from "@codemirror/view";
import { Send, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RecipientPicker } from "@/components/sidebar/RecipientPicker";
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
  // Parent author, resolved lazily so we can pre-select and lock the
  // chip when the reply composer opens. The CLI enforces the
  // parent-in-to invariant server-side (rem-kja); the lock is purely
  // decorative.
  const [parentAuthor, setParentAuthor] = useState<string | null>(null);
  const [to, setTo] = useState<string[]>([]);
  const containerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const submitRef = useRef<() => void>(noop);
  const closeRef = useRef<() => void>(noop);

  const locked = useMemo(() => (parentAuthor ? [parentAuthor] : []), [parentAuthor]);

  const handleSubmit = useCallback(async () => {
    const content = viewRef.current?.state.doc.toString().trim() ?? "";
    if (!content || submitting) return;
    setSubmitting(true);
    try {
      await backend.comment(file, content, {
        replyTo,
        autoAck: true,
        sandbox: true,
        to,
      });
      onSubmitted();
    } catch {
      // no-op
    } finally {
      setSubmitting(false);
    }
  }, [backend, file, replyTo, submitting, onSubmitted, to]);

  submitRef.current = () => void handleSubmit();
  closeRef.current = onClose;

  // Fetch the parent author once per (file, replyTo) pair so we can
  // pre-populate the chip. If the fetch fails (unlikely — the comment
  // was just displayed), we silently fall back to no pre-selection;
  // the CLI will still insert `to: [<parent_author>]` on submit.
  useEffect(() => {
    let cancelled = false;
    backend
      .comments(file)
      .then((comments) => {
        if (cancelled) return;
        const parent = comments.find((c) => c.id === replyTo);
        if (!parent) return;
        setParentAuthor(parent.author);
        setTo((prev) => (prev.includes(parent.author) ? prev : [parent.author, ...prev]));
      })
      .catch((err: unknown) => {
        console.error("InlineReplyEditor: failed to resolve parent author:", err);
      });
    return () => {
      cancelled = true;
    };
  }, [backend, file, replyTo]);

  useEffect(() => {
    if (editorRef.current && !viewRef.current) {
      viewRef.current = createCommentEditor({
        parent: editorRef.current,
        placeholder: "Write a reply...",
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
        <span className="text-[10px] text-text-faint">Replying to {replyTo}</span>
        <Button variant="ghost" size="sm" className="h-5 w-5 p-0 text-text-faint" onClick={onClose}>
          <X className="w-3 h-3" />
        </Button>
      </div>
      <RecipientPicker selected={to} onChange={setTo} locked={locked} />
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
