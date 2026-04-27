import {
  type MarkdownPostProcessor,
  type MarkdownPostProcessorContext,
  MarkdownRenderChild,
} from "obsidian";
import { createElement } from "react";
import { createRoot as defaultCreateRoot, type Root } from "react-dom/client";
import { WidgetCommentView } from "@/components/widget/WidgetCommentView";
import type { Comment } from "@/generated";
import type RemarginPlugin from "@/main";
import { type ParsedBlock, parseRemarginBlocks } from "@/parser/parseRemarginBlocks";

/**
 * Indirection for `createRoot` so unit tests can swap it for a mock
 * without monkey-patching the imported binding. Production code uses
 * the default React 19 implementation; the test seam below is the only
 * non-test caller. Keep this private to the module — exposing it would
 * be an architectural smell.
 */
let createRootImpl: typeof defaultCreateRoot = defaultCreateRoot;

/**
 * Test-only seam: replace the `createRoot` factory so tests can
 * exercise `ReadingModeCommentChild.onload` / `onunload` without a
 * real DOM. NOT exported via the package barrel; only the unit test
 * imports it.
 */
export function __setCreateRootForTests(impl: typeof defaultCreateRoot | null): void {
  createRootImpl = impl ?? defaultCreateRoot;
}

/**
 * Reading-mode markdown post-processor that swaps each well-formed
 * `<pre><code class="language-remargin">…</code></pre>` block for the
 * shared `WidgetCommentView` React tree (T37).
 *
 * Why this shape:
 *
 *  - Returns a fresh post-processor closure for every plugin so the
 *    `editorWidgets` setting is read on every render call. Toggling the
 *    setting at runtime takes effect on the next Obsidian render pass —
 *    no re-registration needed.
 *  - When the setting is off OR the block is malformed (parser returns
 *    not-exactly-one valid block with an `id`), we leave the raw `<pre>`
 *    untouched. This was the user-confirmed behaviour: a malformed
 *    block is not a real comment to remargin anyway, so showing it raw
 *    matches reality.
 *  - The replacement is a plain `<div>` host that gets handed to a
 *    `MarkdownRenderChild` via `ctx.addChild`, so Obsidian can tear the
 *    React subtree down on preview re-render. This is the lifecycle
 *    fix that the v1 attempt (commit 25a612a) missed.
 */
export function remarginPostProcessor(plugin: RemarginPlugin): MarkdownPostProcessor {
  return (el: HTMLElement, ctx: MarkdownPostProcessorContext): void => {
    if (!plugin.settings.editorWidgets) return;

    const codes = el.querySelectorAll<HTMLElement>("pre > code.language-remargin");
    for (const code of Array.from(codes)) {
      const pre = code.parentElement;
      if (!pre) continue;

      const parsed = parseRemarginBlocks(code.textContent ?? "");
      // Skip when the fence wasn't a single, well-formed comment block.
      // We need exactly one valid parsed block with an id — anything else
      // (zero, multiple, or invalid) falls through to the raw `<pre>`.
      if (parsed.length !== 1) continue;
      const block = parsed[0];
      if (!block.valid || !block.comment.id) continue;

      const host = document.createElement("div");
      host.className = "remargin-reading-host";
      host.dataset.remarginId = block.comment.id;
      pre.replaceWith(host);

      ctx.addChild(new ReadingModeCommentChild(host, block, ctx.sourcePath, plugin));
    }
  };
}

/**
 * `MarkdownRenderChild` that owns the React root mounted into a
 * reading-mode host element. `onload` mounts the widget and subscribes
 * to the plugin-wide collapse store so the widget re-renders when the
 * comment's collapsed flag flips. `onunload` tears the subscription and
 * the root down — Obsidian calls this when the preview re-renders or
 * the leaf closes, so leaks are not possible.
 */
export class ReadingModeCommentChild extends MarkdownRenderChild {
  private root: Root | null = null;
  private unsubscribe: (() => void) | null = null;

  constructor(
    el: HTMLElement,
    private readonly parsed: ParsedBlock,
    private readonly sourcePath: string,
    private readonly plugin: RemarginPlugin
  ) {
    super(el);
  }

  onload(): void {
    this.root = createRootImpl(this.containerEl);
    this.render();
    // Re-render only when the toggle is for OUR id. The collapse store
    // notifies every subscriber on every flip, so other comments'
    // toggles must be filtered out here — otherwise every widget on
    // the page would re-render whenever any single one collapsed.
    this.unsubscribe = this.plugin.collapseState.subscribe((id) => {
      if (id === this.parsed.comment.id) this.render();
    });
  }

  onunload(): void {
    this.unsubscribe?.();
    this.unsubscribe = null;
    this.root?.unmount();
    this.root = null;
  }

  private render(): void {
    if (!this.root) return;
    const id = this.parsed.comment.id;
    if (!id) return;
    this.root.render(
      createElement(WidgetCommentView, {
        // The widget expects a full `Comment`. The parser returns
        // `Partial<Comment>` because malformed blocks may miss fields —
        // but at this point we've already filtered to `valid && id`,
        // so the cast is sound. The header/body components handle
        // missing optional fields gracefully (defaults to empty
        // string / array).
        comment: this.parsed.comment as Comment,
        sourcePath: this.sourcePath,
        collapsed: this.plugin.collapseState.isCollapsed(id),
        onToggle: () => this.plugin.collapseState.toggle(id),
        onClick: (cid, file) => {
          this.plugin.focusComment(cid, file);
        },
      })
    );
  }
}
