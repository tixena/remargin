/** Reading-mode comment widgets: a post-processor that replaces remargin code blocks. */

import {
  type MarkdownPostProcessor,
  type MarkdownPostProcessorContext,
  MarkdownRenderChild,
  type TAbstractFile,
  TFile,
} from "obsidian";
import { createElement } from "react";
import { createRoot as defaultCreateRoot, type Root } from "react-dom/client";
import { WidgetCommentThread } from "@/components/widget/WidgetCommentThread";
import { WidgetProviders } from "@/components/widget/WidgetProviders";
import type { Comment } from "@/generated";
import { buildThreadTree, type ThreadNode, walkThread } from "@/lib/threadTree";
import type RemarginPlugin from "@/main";
import { type ParsedBlock, parseRemarginBlocks } from "@/parser/parseRemarginBlocks";

/** Test seam for `createRoot`; module-private. */
let createRootImpl: typeof defaultCreateRoot = defaultCreateRoot;

/**
 * Re-wrap the stripped inner content of a `<pre><code class="language-remargin">`
 * block with synthesized fences before delegating to the document-level
 * `parseRemarginBlocks`. Obsidian's markdown renderer hands us only the
 * inner content (no `` ``` `` markers), but the parser is a fence-aware
 * state machine that requires them to enter the YAML/Content states.
 *
 * Trade-off: the synthesized text is NOT byte-equal to the source on disk,
 * so `block.startOffset` / `endOffset` are offsets in the synthesized
 * string. The post-processor doesn't read those offsets — it only checks
 * `valid` and `comment.id` — so this is sound.
 */
export function parseFromInnerContent(inner: string): ReturnType<typeof parseRemarginBlocks> {
  const wrapped = `\`\`\`remargin\n${inner.replace(/\n*$/, "")}\n\`\`\`\n`;
  return parseRemarginBlocks(wrapped);
}

/** Test-only seam: swaps the `createRoot` factory so the lifecycle runs without a DOM. */
export function __setCreateRootForTests(impl: typeof defaultCreateRoot | null): void {
  createRootImpl = impl ?? defaultCreateRoot;
}

// Defer past Obsidian's incremental render pass: double rAF lands after
// the next paint; setTimeout is the headless/test fallback.
type DeferFn = (cb: () => void) => void;
const defaultDefer: DeferFn = (cb) => {
  if (typeof requestAnimationFrame === "function") {
    requestAnimationFrame(() => requestAnimationFrame(cb));
  } else {
    setTimeout(cb, 0);
  }
};
let deferCollapse: DeferFn = defaultDefer;

export function __setDeferCollapseForTests(fn: DeferFn | null): void {
  deferCollapse = fn ?? defaultDefer;
}

/**
 * Reading-mode markdown post-processor that swaps each well-formed
 * `<pre><code class="language-remargin">…</code></pre>` block for the
 * shared `WidgetCommentThread` React tree.
 *
 * The structural replacement (host swap, `addChild`) happens
 * synchronously per block. Cross-block thread nesting is resolved
 * asynchronously inside `ReadingModeCommentChild.onload` by reading
 * the full source via `vault.cachedRead` and rebuilding the document
 * thread tree — replies whose parent is in the same file render
 * NOTHING (the parent's host renders them nested), and root blocks
 * render their full subtree.
 */
export function remarginPostProcessor(plugin: RemarginPlugin): MarkdownPostProcessor {
  return (el: HTMLElement, ctx: MarkdownPostProcessorContext): void => {
    if (!plugin.settings.editorWidgets) return;

    const codes = el.querySelectorAll<HTMLElement>("pre > code.language-remargin");
    for (const code of Array.from(codes)) {
      const pre = code.parentElement;
      if (!pre) continue;

      const parsed = parseFromInnerContent(code.textContent ?? "");
      // Exactly one valid parsed block with an id; anything else falls through to the raw `<pre>`.
      if (parsed.length !== 1) continue;
      const block = parsed[0];
      if (!block.valid || !block.comment.id) continue;

      const host = document.createElement("div");
      // `remargin-container` is what scopes the Tailwind utilities to this widget's subtree.
      host.className = "remargin-reading-host remargin-container";
      host.dataset.remarginId = block.comment.id;
      // visibility, not display:none — a zero-height section stalls
      // Obsidian's incremental reading-mode renderer.
      host.style.visibility = "hidden";
      pre.replaceWith(host);

      ctx.addChild(new ReadingModeCommentChild(host, block, ctx.sourcePath, plugin));
    }
  };
}

/**
 * `MarkdownRenderChild` that owns the React root mounted into a
 * reading-mode host element.
 *
 * On load it renders a leaf-only thread immediately, then asynchronously
 * fetches the full source via `vault.cachedRead` to rebuild the
 * document-scope thread tree. Once resolved:
 *   - non-orphan reply (parent in this doc) → unmount + hide host so
 *     the parent's host owns the rendering.
 *   - root or orphan reply → render the full subtree under our host.
 *
 * Subscribes to the plugin-wide collapse store so widget chevron flips
 * trigger a re-render. `onunload` tears down the subscription, the
 * vault listener, and the React root.
 */
export class ReadingModeCommentChild extends MarkdownRenderChild {
  private root: Root | null = null;
  private unsubscribeCollapse: (() => void) | null = null;
  private unsubscribeVault: (() => void) | null = null;
  private subtree: ThreadNode | null = null;
  /** True for a reply whose parent is in the doc: the parent's host renders it. */
  private suppressed = false;
  /** Ids covered by the current subtree — used to filter collapse notifications. */
  private subtreeIds = new Set<string>();
  /** Bumped on each `loadTree` call so stale resolutions are ignored. */
  private loadGeneration = 0;

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
    this.subtreeIds = new Set([this.parsed.comment.id ?? ""]);
    // Paint now (hidden) to reserve the section's height; loadTree reveals.
    this.render();
    this.unsubscribeCollapse = this.plugin.collapseState.subscribe((id) => {
      if (this.subtreeIds.has(id)) this.render();
    });
    void this.loadTree();
    // Catches background edits of the source file, e.g. from another pane.
    const file = this.plugin.app.vault.getAbstractFileByPath(this.sourcePath);
    if (file instanceof TFile) {
      const handler = (modified: TAbstractFile) => {
        if (modified.path === this.sourcePath) void this.loadTree();
      };
      const ref = this.plugin.app.vault.on("modify", handler);
      this.unsubscribeVault = () => this.plugin.app.vault.offref(ref);
    }
  }

  onunload(): void {
    this.unsubscribeCollapse?.();
    this.unsubscribeCollapse = null;
    this.unsubscribeVault?.();
    this.unsubscribeVault = null;
    this.root?.unmount();
    this.root = null;
  }

  private async loadTree(): Promise<void> {
    const id = this.parsed.comment.id;
    if (!id) return;
    const file = this.plugin.app.vault.getAbstractFileByPath(this.sourcePath);
    if (!(file instanceof TFile)) {
      // The path resolved to no vault file: with no document tree to suppress by, render the leaf.
      console.warn("[remargin] loadTree: no TFile for", this.sourcePath, "— rendering leaf only");
      this.reveal();
      return;
    }
    const generation = (this.loadGeneration += 1);
    let text: string;
    try {
      text = await this.plugin.app.vault.cachedRead(file);
    } catch (err) {
      // The read failed: render the leaf so the host does not stay hidden.
      console.warn("[remargin] loadTree: cachedRead failed for", this.sourcePath, err);
      this.reveal();
      return;
    }
    // Bail if we were unloaded or a newer load superseded this one.
    if (this.root === null) return;
    if (generation !== this.loadGeneration) return;

    const blocks = parseRemarginBlocks(text);
    const validComments: Comment[] = blocks
      .filter((b) => b.valid && b.comment.id)
      .map((b) => b.comment as Comment);
    const trees = buildThreadTree(validComments);
    const nodeById = new Map<string, ThreadNode>();
    for (const node of trees) collectNodes(node, nodeById);
    const rootIds = new Set(trees.map((n) => n.comment.id));

    if (!rootIds.has(id)) {
      this.suppressed = true;
      this.subtree = null;
      this.subtreeIds = new Set();
      // Collapse after the render pass: display:none mid-render leaves a zero-height section that
      // stalls Obsidian's incremental renderer.
      this.collapse();
      return;
    }

    const node = nodeById.get(id);
    if (!node) return;
    this.subtree = node;
    this.suppressed = false;
    this.subtreeIds = collectIds(node);
    this.render();
    this.reveal();
  }

  private render(): void {
    if (!this.root) return;
    if (this.suppressed) return;
    const id = this.parsed.comment.id;
    if (!id) return;
    const node: ThreadNode = this.subtree ?? {
      // The post-processor filters to `valid && id`, so the cast from `Partial<Comment>` is sound.
      comment: this.parsed.comment as Comment,
      replies: [],
    };
    const me = this.plugin.currentIdentity ?? null;
    this.root.render(
      createElement(
        WidgetProviders,
        { plugin: this.plugin, portalContainer: this.containerEl },
        createElement(WidgetCommentThread, {
          root: node,
          sourcePath: this.sourcePath,
          me,
          collapseState: this.plugin.collapseState,
          onClick: (cid, file) => {
            this.plugin.focusComment(cid, file);
          },
          isRoot: true,
        })
      )
    );
  }

  private reveal(): void {
    const el = this.containerEl as HTMLElement;
    el.style.display = "";
    el.style.visibility = "";
  }

  private collapse(): void {
    deferCollapse(() => {
      // A newer loadTree may have reclaimed this host as a root, or we
      // may have unloaded — either way, leave it alone.
      if (!this.suppressed || this.root === null) return;
      (this.containerEl as HTMLElement).style.display = "none";
      this.root.unmount();
      this.root = null;
    });
  }
}

function collectNodes(node: ThreadNode, into: Map<string, ThreadNode>): void {
  into.set(node.comment.id, node);
  for (const reply of node.replies) {
    collectNodes(reply, into);
  }
}

function collectIds(node: ThreadNode): Set<string> {
  const ids = new Set<string>();
  for (const c of walkThread(node)) ids.add(c.id);
  return ids;
}
