/** Live Preview comment widgets: block decorations that replace remargin fences. */

import { type EditorState, RangeSetBuilder, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";
import { editorInfoField, editorLivePreviewField } from "obsidian";
import { createElement } from "react";
import { createRoot as defaultCreateRoot, type Root } from "react-dom/client";
import { WidgetCommentThread } from "@/components/widget/WidgetCommentThread";
import { WidgetProviders } from "@/components/widget/WidgetProviders";
import type { Comment } from "@/generated";
import { buildThreadTree, type ThreadNode } from "@/lib/threadTree";
import type RemarginPlugin from "@/main";
import { parseRemarginBlocks } from "@/parser/parseRemarginBlocks";

/** Test seam for `createRoot`, so lifecycle assertions can run without a real DOM. */
let createRootImpl: typeof defaultCreateRoot = defaultCreateRoot;
export function __setCreateRootForTests(impl: typeof defaultCreateRoot | null): void {
  createRootImpl = impl ?? defaultCreateRoot;
}

/**
 * Resolve the editor's mode from the `editorLivePreviewField` StateField
 * exported by Obsidian: the public signal for Live Preview versus Source
 * Mode, and one that is readable from inside another StateField.
 */
function isLivePreviewState(state: EditorState): boolean {
  try {
    return state.field(editorLivePreviewField, false) ?? false;
  } catch {
    return false;
  }
}

/**
 * Resolve the source-file path the widget should pass to the click
 * bridge. CM6 itself has no notion of "the file"; Obsidian exposes the
 * surrounding context through the `editorInfoField` StateField. When
 * the field is absent (e.g. an editor stood up outside Obsidian for
 * tests), fall back to an empty string — the click handler still
 * fires, just without a file context.
 */
function resolveSourcePath(state: EditorState): string {
  try {
    const info = state.field(editorInfoField, false);
    return info?.file?.path ?? "";
  } catch {
    return "";
  }
}

/**
 * Cheap, non-cryptographic hash for the parsed block's raw text. Used
 * by `WidgetType.eq` to detect "same id, but content changed" (e.g. a
 * reaction added, an edit applied) so CM6 will tear the widget down
 * and rebuild it. Cryptographic strength is unnecessary — collisions
 * just cause an extra rebuild, never a correctness bug.
 */
function hashRaw(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i += 1) {
    h = (h * 31 + s.charCodeAt(i)) | 0;
  }
  return h;
}

/**
 * CM6 widget that mounts the shared `WidgetCommentThread` React tree in
 * place of a remargin fenced block while in Live Preview.
 *
 * Each block decoration corresponds to one *root* in the document's
 * thread forest: the parsed block is either an actual root (no
 * `reply_to`) or an orphan reply whose parent is missing from the
 * document. Replies whose parent IS in the document do not produce a
 * decoration — the parent's widget renders them nested.
 *
 *  - `ignoreEvent()` returns `true` so typing/selection inside or
 *    adjacent to the replaced range is not eaten by the widget. The
 *    widget's own click is wired through React, not CM6's event path.
 *  - `eq()` compares id + collapsed state + raw-text hash, so a comment
 *    edited in place is rebuilt even though its offsets did not move.
 *  - `toDOM()` mounts a React root and `destroy()` unmounts it, giving
 *    the React subtree a real lifecycle.
 */
export class RemarginWidget extends WidgetType {
  private readonly id: string;
  /** Hash of every descendant's raw text so `eq` rebuilds when ANY descendant changes. */
  private readonly subtreeHash: number;
  /**
   * Captured at construction, not read live inside `eq`: two widgets reading the same current
   * state would compare equal and CM6 would skip the rebuild a toggle needs.
   */
  private readonly collapsedAtBuildTime: boolean;

  constructor(
    private readonly threadNode: ThreadNode,
    private readonly plugin: RemarginPlugin,
    private readonly sourcePath: string
  ) {
    super();
    // "" when a caller did not filter: an unfocused widget beats a throw inside CM6's pipeline.
    this.id = threadNode.comment.id ?? "";
    this.subtreeHash = hashSubtree(threadNode);
    this.collapsedAtBuildTime = plugin.collapseState.isCollapsed(this.id);
  }

  /**
   * `eq` decides whether CM6 can reuse the existing DOM. Returning
   * `false` forces a `destroy` + `toDOM` cycle, which is what we want
   * whenever the widget's *visible* content could have changed.
   *
   * We compare the snapshot collapsed state, NOT the live value — see
   * `collapsedAtBuildTime` for why.
   */
  eq(other: WidgetType): boolean {
    if (!(other instanceof RemarginWidget)) return false;
    return (
      this.id === other.id &&
      this.collapsedAtBuildTime === other.collapsedAtBuildTime &&
      this.subtreeHash === other.subtreeHash
    );
  }

  toDOM(): HTMLElement {
    const host = document.createElement("div");
    // `remargin-container` is what scopes the Tailwind utilities to this widget's subtree.
    host.className = "remargin-widget-host remargin-container";
    host.dataset.remarginId = this.id;
    const root = createRootImpl(host);
    // Stashed on the host so `destroy(dom)`, handed the same node, can find the root.
    (host as HTMLElement & { __remarginRoot?: Root }).__remarginRoot = root;
    const me = this.plugin.currentIdentity ?? null;
    root.render(
      createElement(
        WidgetProviders,
        { plugin: this.plugin, portalContainer: host },
        createElement(WidgetCommentThread, {
          root: this.threadNode as { comment: Comment; replies: ThreadNode[] },
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
    return host;
  }

  destroy(dom: HTMLElement): void {
    const root = (dom as HTMLElement & { __remarginRoot?: Root }).__remarginRoot;
    root?.unmount();
  }

  ignoreEvent(): boolean {
    // True lets CM6 handle the event normally, so the widget swallows no keystrokes or selection.
    return true;
  }
}

/**
 * Hash a thread node's full subtree raw text so `eq()` rebuilds when
 * any descendant changes (a reply added, an existing reply edited,
 * etc.). Cryptographic strength is unnecessary — collisions just
 * cause an extra rebuild, never a correctness bug.
 */
function hashSubtree(node: ThreadNode): number {
  let h = hashRaw(node.comment.id ?? "");
  for (const reply of node.replies) {
    h = (h * 31 + hashSubtree(reply)) | 0;
  }
  h = (h * 31 + hashRaw(node.comment.content ?? "")) | 0;
  // Ack list affects pending badges; mix length + last ts.
  h = (h * 31 + node.comment.ack.length) | 0;
  return h;
}

/**
 * Build the decoration set for the current state. Skipped (returns
 * `Decoration.none`) when the feature toggle is off OR the editor is
 * in Source Mode, the same fall-through to the raw fence as the
 * reading-mode widget.
 *
 * Document-scope thread building: parse every block in the doc once,
 * build the thread tree, then iterate blocks. For each block:
 *   - Root in tree → emit a decoration that renders the full subtree.
 *   - Reply with parent in this doc → emit a hidden decoration (the
 *     parent's widget nests it).
 *   - Orphan reply (parent missing from doc) → emit a degraded-root
 *     decoration so it stays visible.
 */
export function buildDecorations(state: EditorState, plugin: RemarginPlugin): DecorationSet {
  if (!plugin.settings.editorWidgets) return Decoration.none;
  if (!isLivePreviewState(state)) return Decoration.none;

  const text = state.doc.toString();
  const blocks = parseRemarginBlocks(text);
  const sourcePath = resolveSourcePath(state);
  const builder = new RangeSetBuilder<Decoration>();

  const validComments: Comment[] = blocks
    .filter((b) => b.valid && b.comment.id)
    .map((b) => b.comment as Comment);
  const trees = buildThreadTree(validComments);
  const rootIds = new Set(trees.map((n) => n.comment.id));
  const nodeById = new Map<string, ThreadNode>();
  for (const node of trees) collectNodes(node, nodeById);

  for (const block of blocks) {
    if (!block.valid || !block.comment.id) continue;
    const id = block.comment.id;
    if (!rootIds.has(id)) {
      // A hidden empty widget replaces the range, or CM6 would render the reply's raw fence.
      builder.add(
        block.startOffset,
        block.endOffset,
        Decoration.replace({ widget: new EmptyWidget(), block: true, inclusive: true })
      );
      continue;
    }
    const node = nodeById.get(id);
    if (!node) continue;
    const widget = new RemarginWidget(node, plugin, sourcePath);
    builder.add(
      block.startOffset,
      block.endOffset,
      // `inclusive: true` lets the range absorb its bounding cursor positions; without it CM6 keeps
      // a boundary cursor zone above the widget, which Live Preview draws as an empty dark bar.
      Decoration.replace({ widget, block: true, inclusive: true })
    );
  }
  return builder.finish();
}

/**
 * Zero-height block widget used to suppress the raw YAML source of a
 * reply block whose parent renders it nested. CM6 requires SOMETHING
 * to fill a `Decoration.replace` range; this widget renders a
 * `display: none` div so it consumes no vertical space and shows no
 * content. `eq` always returns true for instances of this class so
 * CM6 can reuse the DOM across rebuilds without churn.
 */
class EmptyWidget extends WidgetType {
  toDOM(): HTMLElement {
    const el = document.createElement("div");
    el.style.display = "none";
    return el;
  }
  ignoreEvent(): boolean {
    return true;
  }
  eq(other: WidgetType): boolean {
    return other instanceof EmptyWidget;
  }
}

function collectNodes(node: ThreadNode, into: Map<string, ThreadNode>): void {
  into.set(node.comment.id, node);
  for (const reply of node.replies) {
    collectNodes(reply, into);
  }
}

/**
 * Dispatched whenever the shared `CollapseState` flips for a comment id. `CollapseState.toggle`
 * produces no CM6 transaction of its own, so the bridge plugin turns it into this effect.
 */
export const collapseEffect = StateEffect.define<{ id: string }>();

/**
 * CM6 StateField factory. Returns a fresh StateField per
 * `RemarginPlugin` instance so the widget has a stable plugin
 * reference for collapse-state and focus-bridge calls.
 *
 * MUST be a StateField: block decorations are forbidden from
 * per-view plugin instances by CM6 — the runtime check throws
 * `RangeError: Block decorations may not be specified via plugins`.
 * Block decorations alter document layout (line heights), which CM6
 * cannot reflow within a single view-level transaction.
 */
export function commentWidgetPlugin(plugin: RemarginPlugin) {
  return StateField.define<DecorationSet>({
    create(state) {
      return buildDecorations(state, plugin);
    },
    update(decorations, tr) {
      // Rebuild only on a document change or a collapse effect; selection and viewport updates just
      // remap the existing ranges.
      if (tr.docChanged) {
        return buildDecorations(tr.state, plugin);
      }
      for (const e of tr.effects) {
        if (e.is(collapseEffect)) {
          return buildDecorations(tr.state, plugin);
        }
      }
      return decorations.map(tr.changes);
    },
    provide: (f) => EditorView.decorations.from(f),
  });
}

/**
 * Companion ViewPlugin that bridges the plugin-wide `CollapseState`
 * store to the CM6 StateField above. On construction it subscribes to
 * the store; every time the store fires (i.e. someone called
 * `collapseState.toggle(id)`) it dispatches a `collapseEffect` carrying
 * the toggled id so the StateField can rebuild. On `destroy` it
 * unsubscribes — without this, every closed editor leaks one listener
 * per registered StateField.
 *
 * This plugin produces NO decorations of its own, so the CM6
 * "block decorations from a per-view plugin" prohibition is not
 * violated. The StateField above remains the sole decoration source.
 */
export function collapseEffectBridge(plugin: RemarginPlugin) {
  return ViewPlugin.fromClass(
    // TS4094 forbids `private` members on an anonymous exported class, so `unsubscribe` is plain.
    class {
      readonly unsubscribe: () => void;

      constructor(view: EditorView) {
        this.unsubscribe = plugin.collapseState.subscribe((id) => {
          view.dispatch({ effects: collapseEffect.of({ id }) });
        });
      }

      destroy(): void {
        this.unsubscribe();
      }
    }
  );
}
