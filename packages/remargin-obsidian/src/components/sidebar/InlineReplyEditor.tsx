import { Send, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useBackend } from "@/hooks/useBackend";

interface InlineReplyEditorProps {
  file: string;
  replyTo: string;
  onClose: () => void;
  onSubmitted: () => void;
}

export function InlineReplyEditor({ file, replyTo, onClose, onSubmitted }: InlineReplyEditorProps) {
  const backend = useBackend();
  const [content, setContent] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const handleSubmit = useCallback(async () => {
    if (!content.trim() || submitting) return;
    setSubmitting(true);
    try {
      await backend.comment(file, content.trim(), {
        replyTo,
        autoAck: true,
      });
      setContent("");
      onSubmitted();
    } catch {
      // no-op
    } finally {
      setSubmitting(false);
    }
  }, [backend, file, replyTo, content, submitting, onSubmitted]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        handleSubmit();
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
        <span className="text-[10px] text-text-faint">Replying to {replyTo}</span>
        <Button variant="ghost" size="sm" className="h-5 w-5 p-0 text-text-faint" onClick={onClose}>
          <X className="w-3 h-3" />
        </Button>
      </div>
      <textarea
        ref={textareaRef}
        value={content}
        onChange={(e) => setContent(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Write a reply..."
        className="w-full min-h-[60px] p-2 text-xs font-mono bg-bg-primary border border-bg-border rounded-sm text-text-normal placeholder:text-text-faint resize-y focus:outline-none focus:ring-1 focus:ring-accent"
      />
      <div className="flex items-center justify-between">
        <span className="text-[9px] text-text-faint">Ctrl+Enter to send</span>
        <Button
          size="sm"
          className="h-6 px-2 text-[10px] bg-accent text-white hover:bg-accent-hover"
          disabled={!content.trim() || submitting}
          onClick={handleSubmit}
        >
          <Send className="w-3 h-3 mr-1" />
          {submitting ? "Sending..." : "Send"}
        </Button>
      </div>
    </div>
  );
}
