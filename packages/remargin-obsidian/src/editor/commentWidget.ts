import { type EditorState, RangeSetBuilder, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, WidgetType } from "@codemirror/view";
import { editorInfoField, editorLivePreviewField } from "obsidian";
import { createElement } from "react";
import { createRoot as defaultCreateRoot, type Root } from "react-dom/client";
import { WidgetCommentView } from "@/components/widget/WidgetCommentView";
import type { Comment } from "@/generated";
import type RemarginPlugin from "@/main";
import { type ParsedBlock, parseRemarginBlocks } from "@/parser/parseRemarginBlocks";

/**
 * Test seam for `react-dom/client`'s `createRoot`. Production code uses
 * the default React 19 implementation; unit tests swap it for a mock so
 * `toDOM` / `destroy` lifecycle assertions can run without a real DOM.
 * Mirrors the pattern in `readingModeProcessor.ts`.
 */
let createRootImpl: typeof defaultCreateRoot = defaultCreateRoot;
export function __setCreateRootForTests(impl: typeof defaultCreateRoot | null): void {
  createRootImpl = impl ?? defaultCreateRoot;
}

/**
 * Resolve the editor's mode from the `editorLivePreviewField` StateField
 * exported by Obsidian. It is the public, contract-stable signal for
 * Live-Preview-vs-Source-Mode and — critically — is readable from
 * inside another StateField, which the older host-DOM ancestor lookup
 * was not.
 *
 * The wider host change (state-field-based decorations) is forced by
 * CM6's rule that block decorations must come from a state field, not
 * a per-view plugin; this helper is the matching state-side mode probe.
 */
function isLivePreviewState(state: EditorState): boolean {
  try {
    return state.field(editorLivePreviewField, /* require */ false) ?? false;
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
    const info = state.field(editorInfoField, /* require */ false);
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
 * CM6 widget that mounts the shared `WidgetCommentView` React tree in
 * place of a remargin fenced block while in Live Preview.
 *
 * Critical fixes vs. the v1 attempt (commit 25a612a, reverted in
 * 67ef39d):
 *
 *  - `ignoreEvent()` returns `true` so typing/selection inside or
 *    adjacent to the replaced range is not eaten by the widget. The
 *    widget's own click is wired through React, not CM6's event path.
 *  - `eq()` compares id + collapsed state + raw-text hash. v1 only
 *    compared offsets, which yielded "stale data because the offset
 *    didn't move" misses whenever a comment was edited in place.
 *  - `toDOM()` mounts a React root and `destroy()` unmounts it — the
 *    React subtree gets a real lifecycle, not the leaky bare-DOM
 *    swap v1 used.
 */
export class RemarginWidget extends WidgetType {
  /** Cached id (always present — caller filters for `block.valid`). */
  private readonly id: string;
  /** Cached body hash so `eq` doesn't recompute on every comparison. */
  private readonly contentHash: number;
  /**
   * Collapsed state captured at *construction* time. Snapshotting here
   * (rather than reading `plugin.collapseState.isCollapsed` inside
   * `eq`) is the contract that lets the next `build()` produce a
   * widget that `eq`-differs from the previous one for that id —
   * otherwise both widgets would read the same current state and CM6
   * would skip the rebuild we explicitly want.
   */
  private readonly collapsedAtBuildTime: boolean;

  constructor(
    private readonly parsed: ParsedBlock,
    private readonly plugin: RemarginPlugin,
    private readonly sourcePath: string
  ) {
    super();
    // The build path filters for `parsed.valid && parsed.comment.id`;
    // the non-null assertion is sound but we still default to "" if
    // some future caller forgets — better to render an unfocused
    // widget than to throw inside CM6's decoration pipeline.
    this.id = parsed.comment.id ?? "";
    this.contentHash = hashRaw(parsed.raw);
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
      this.contentHash === other.contentHash
    );
  }

  toDOM(): HTMLElement {
    const host = document.createElement("div");
    host.className = "remargin-widget-host";
    host.dataset.remarginId = this.id;
    const root = createRootImpl(host);
    // Stash the root on the host so `destroy(dom)` can find it without
    // an external map. The cast is intentional — CM6's WidgetType API
    // hands the same DOM node back to `destroy`.
    (host as HTMLElement & { __remarginRoot?: Root }).__remarginRoot = root;
    root.render(
      createElement(WidgetCommentView, {
        // The build path filters for `parsed.valid` so the cast to a
        // full `Comment` is sound; missing optional fields default
        // gracefully inside the header/body components.
        comment: this.parsed.comment as Comment,
        sourcePath: this.sourcePath,
        // Use the snapshot from construction time. CM6 only calls
        // `toDOM` when this widget is being mounted for the first
        // time (or after `eq` returned false → rebuild), so the
        // snapshot is the right value to render against.
        collapsed: this.collapsedAtBuildTime,
        onToggle: () => this.plugin.collapseState.toggle(this.id),
        onClick: (cid, file) => {
          this.plugin.focusComment(cid, file);
        },
      })
    );
    return host;
  }

  destroy(dom: HTMLElement): void {
    const root = (dom as HTMLElement & { __remarginRoot?: Root }).__remarginRoot;
    root?.unmount();
  }

  ignoreEvent(): boolean {
    // True means "let CM6 handle this event normally" — i.e. don't
    // swallow keystrokes/selection inside the widget. The widget's
    // own click bridge is wired at the React layer; this flag is
    // about the surrounding editor's caret handling.
    return true;
  }
}

/**
 * Build the decoration set for the current state. Skipped (returns
 * `Decoration.none`) when the feature toggle is off OR the editor is
 * in Source Mode — same fall-through to the raw fence as the
 * reading-mode widget (T37).
 */
export function buildDecorations(state: EditorState, plugin: RemarginPlugin): DecorationSet {
  if (!plugin.settings.editorWidgets) return Decoration.none;
  if (!isLivePreviewState(state)) return Decoration.none;

  const text = state.doc.toString();
  const blocks = parseRemarginBlocks(text);
  const sourcePath = resolveSourcePath(state);
  const builder = new RangeSetBuilder<Decoration>();
  for (const block of blocks) {
    if (!block.valid || !block.comment.id) continue;
    const widget = new RemarginWidget(block, plugin, sourcePath);
    builder.add(
      block.startOffset,
      block.endOffset,
      // `block: true` makes the widget take a full block in CM6's
      // layout (the line is replaced wholesale, not inlined).
      // `inclusive: false` keeps the caret distinct from the inside
      // of the widget — necessary so arrowing across the fence
      // boundary lands cleanly on either side.
      Decoration.replace({ widget, block: true, inclusive: false })
    );
  }
  return builder.finish();
}

/**
 * CM6 StateField factory. Returns a fresh StateField per
 * `RemarginPlugin` instance so the widget has a stable plugin
 * reference for collapse-state and focus-bridge calls.
 *
 * MUST be a StateField: block decorations are forbidden from
 * per-view plugin instances by CM6 — the runtime check throws
 * `RangeError: Block decorations may not be specified via plugins`
 * (see ticket rem-3dra). Block decorations alter document layout
 * (line heights), which CM6 cannot reflow within a single
 * view-level transaction.
 */
export function commentWidgetPlugin(plugin: RemarginPlugin) {
  return StateField.define<DecorationSet>({
    create(state) {
      return buildDecorations(state, plugin);
    },
    update(decorations, tr) {
      // Critical perf fix vs. v1: rebuild ONLY when the document
      // changed. Selection-only and viewport-only updates never
      // produce widget content changes, so they should not touch
      // the decoration set — just remap existing ranges through
      // the (empty) change set so offsets stay coherent.
      if (tr.docChanged) {
        return buildDecorations(tr.state, plugin);
      }
      return decorations.map(tr.changes);
    },
    provide: (f) => EditorView.decorations.from(f),
  });
}
