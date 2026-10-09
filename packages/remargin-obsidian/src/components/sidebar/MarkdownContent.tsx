/** Renders comment markdown through Obsidian's own renderer. */

import { MarkdownRenderer } from "obsidian";
import { useEffect, useRef } from "react";
import { usePlugin } from "@/hooks/usePlugin";
import { cn } from "@/lib/utils";

/** Props for {@link MarkdownContent}. */
interface MarkdownContentProps {
  content: string;
  sourcePath: string;
  className?: string;
}

export function MarkdownContent({ content, sourcePath, className }: MarkdownContentProps) {
  const ref = useRef<HTMLDivElement>(null);
  const plugin = usePlugin();

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.empty();
    MarkdownRenderer.render(plugin.app, content, el, sourcePath, plugin);
    return () => {
      el.empty();
    };
  }, [content, sourcePath, plugin]);

  return <div ref={ref} className={cn("remargin-markdown-content", className)} />;
}
