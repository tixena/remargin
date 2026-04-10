import { useState, useCallback, useRef, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Send } from "lucide-react";
import { useBackend } from "@/hooks/useBackend";

interface PromptSectionProps {
  file?: string;
  onCommentAdded?: () => void;
}

export function PromptSection({ file, onCommentAdded }: PromptSectionProps) {
  const backend = useBackend();
  const [content, setContent] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const handleSubmit = useCallback(async () => {
    if (!content.trim() || !file || submitting) return;
    setSubmitting(true);
    try {
      await backend.comment(file, content.trim());
      setContent("");
      onCommentAdded?.();
    } catch {
      // TODO: error handling
    } finally {
      setSubmitting(false);
    }
  }, [backend, file, content, submitting, onCommentAdded]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        handleSubmit();
      }
    },
    [handleSubmit]
  );

  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <textarea
        ref={textareaRef}
        value={content}
        onChange={(e) => setContent(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={
          file
            ? "Add a comment to this file..."
            : "Open a file to add comments"
        }
        disabled={!file}
        className="w-full min-h-[80px] p-2 text-xs font-mono bg-bg-primary border border-bg-border rounded-sm text-text-normal placeholder:text-text-faint resize-y focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
      />
      <div className="flex items-center justify-between">
        <span className="text-[9px] text-text-faint">
          Ctrl+Enter to send
        </span>
        <Button
          size="sm"
          className="h-6 px-2 text-[10px] bg-accent text-white hover:bg-accent-hover"
          disabled={!content.trim() || !file || submitting}
          onClick={handleSubmit}
        >
          <Send className="w-3 h-3 mr-1" />
          {submitting ? "Sending..." : "Comment"}
        </Button>
      </div>
    </div>
  );
}
