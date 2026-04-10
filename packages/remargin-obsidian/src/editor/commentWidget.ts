import { RangeSetBuilder } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  type EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import { type ParsedBlock, parseRemarginBlocks } from "@/parser";

class CommentBlockWidget extends WidgetType {
  constructor(
    readonly block: ParsedBlock,
    readonly collapsed: boolean
  ) {
    super();
  }

  toDOM(): HTMLElement {
    const wrapper = document.createElement("div");
    wrapper.className = "remargin-comment-widget";

    const header = document.createElement("div");
    header.className = "remargin-comment-header";

    const badge = document.createElement("span");
    badge.className = `remargin-badge remargin-badge-${
      this.block.comment.author_type === "Agent" ? "agent" : "human"
    }`;
    badge.textContent = this.block.comment.author_type === "Agent" ? "AI" : "H";
    header.appendChild(badge);

    const author = document.createElement("span");
    author.className = "remargin-author";
    author.textContent = this.block.comment.author ?? "unknown";
    header.appendChild(author);

    if (this.block.comment.ack?.length === 0) {
      const pending = document.createElement("span");
      pending.className = "remargin-pending-dot";
      header.appendChild(pending);
    }

    const time = document.createElement("span");
    time.className = "remargin-time";
    time.textContent = formatRelative(this.block.comment.ts);
    header.appendChild(time);

    wrapper.appendChild(header);

    if (!this.collapsed) {
      const content = document.createElement("div");
      content.className = "remargin-comment-content";
      content.textContent = this.block.comment.content?.split("\n")[0] ?? "";
      wrapper.appendChild(content);
    }

    return wrapper;
  }

  eq(other: CommentBlockWidget): boolean {
    return (
      this.block.startOffset === other.block.startOffset &&
      this.block.endOffset === other.block.endOffset &&
      this.collapsed === other.collapsed
    );
  }

  ignoreEvent(): boolean {
    return false;
  }
}

function buildDecorations(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const doc = view.state.doc;
  const text = doc.toString();
  const blocks = parseRemarginBlocks(text);

  for (const block of blocks) {
    if (!block.valid) continue;

    const from = block.startOffset;
    const to = Math.min(block.endOffset, doc.length);

    if (from >= to) continue;

    const widget = new CommentBlockWidget(block, false);
    builder.add(from, to, Decoration.replace({ widget, block: true }));
  }

  return builder.finish();
}

export const commentWidgetPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged) {
        this.decorations = buildDecorations(update.view);
      }
    }
  },
  {
    decorations: (v) => v.decorations,
  }
);

function formatRelative(ts?: string): string {
  if (!ts) return "";
  try {
    const diff = Date.now() - new Date(ts).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "now";
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h`;
    return `${Math.floor(hours / 24)}d`;
  } catch {
    return "";
  }
}
