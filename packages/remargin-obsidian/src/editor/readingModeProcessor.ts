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

/**
 * Indirection for `createRoot` so unit tests can swap it for a mock
 * without monkey-patching the imported binding. Production code uses
 * the default React 19 implementation; the test seam below is the only
 * non-test caller. Keep this private to the module — exposing it would
 * be an architectural smell.
 */
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
 * `valid` and `comment.id` — so this is sound. If a future caller needs
 * source-mapping, file a follow-up to extract a `parseRemarginInner` API
 * (Path B from rem-hghw).
 */
export function parseFromInnerContent(inner: string): ReturnType<typeof parseRemarginBlocks> {
  const wrapped = `\`\`\`remargin\n${inner.replace(/\n*$/, "")}\n\`\`\`\n`;
  return parseRemarginBlocks(wrapped);
}

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
      // Skip when the fence wasn't a single, well-formed comment block.
      // We need exactly one valid parsed block with an id — anything else
      // (zero, multiple, or invalid) falls through to the raw `<pre>`.
      if (parsed.length !== 1) continue;
      const block = parsed[0];
      if (!block.valid || !block.comment.id) continue;

      const host = document.createElement("div");
      // `remargin-container` makes Tailwind utilities scoped via
      // tailwind.config.ts's `important: ".remargin-container"` apply
      // to this widget's subtree (tooltips and all).
      host.className = "remargin-reading-host remargin-container";
      host.dataset.remarginId = block.comment.id;
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
  /** Resolved subtree once `cachedRead` returns. Null until then. */
  private subtree: ThreadNode | null = null;
  /**
   * True when our id is a non-orphan reply: parent exists in the doc,
   * so this host yields rendering to the parent.
   */
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
    // Initial paint with a leaf-only thread so the user sees something
    // before the async tree resolves.
    this.subtreeIds = new Set([this.parsed.comment.id ?? ""]);
    this.render();
    this.unsubscribeCollapse = this.plugin.collapseState.subscribe((id) => {
      if (this.subtreeIds.has(id)) this.render();
    });
    void this.loadTree();
    // Refresh the tree if the source file mutates while we're mounted
    // (e.g. another pane edits it). Reading-mode re-render handles most
    // updates already, but the listener catches background edits.
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
    if (!(file instanceof TFile)) return;
    const generation = (this.loadGeneration += 1);
    let text: string;
    try {
      text = await this.plugin.app.vault.cachedRead(file);
    } catch {
      // Best-effort: a read failure leaves the leaf-only render in
      // place, which matches the pre-cross-block behaviour.
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
      // Reply whose parent IS in the doc: yield to the parent's host.
      this.suppressed = true;
      this.subtree = null;
      this.subtreeIds = new Set();
      // Hide so the empty host doesn't reserve vertical space.
      (this.containerEl as HTMLElement).style.display = "none";
      this.root.unmount();
      this.root = null;
      return;
    }

    const node = nodeById.get(id);
    if (!node) return;
    this.subtree = node;
    this.suppressed = false;
    this.subtreeIds = collectIds(node);
    this.render();
  }

  private render(): void {
    if (!this.root) return;
    if (this.suppressed) return;
    const id = this.parsed.comment.id;
    if (!id) return;
    const node: ThreadNode = this.subtree ?? {
      // The widget expects a full `Comment`. The parser returns
      // `Partial<Comment>` because malformed blocks may miss fields —
      // but the post-processor filters to `valid && id`, so the cast
      // is sound. Header/body components handle missing optionals.
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
        })
      )
    );
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
