/**
 * End-to-end tests of the editor widgets: a real plugin, collapse state, focus bus,
 * post-processor and CM6 build, so a click or collapse toggle in one surface reaches the
 * matching subscriber. `obsidian`, `createRoot` and `EditorState` are the mocked parts.
 */

import { strict as assert } from "node:assert";
import { afterEach, beforeEach, describe, it } from "node:test";
import type { EditorState } from "@codemirror/state";
import { editorInfoField, editorLivePreviewField } from "obsidian";
import RemarginPlugin, { type RemarginFocusDetail } from "../main.ts";
import { CollapseState } from "../state/collapseState.ts";
import { DEFAULT_SETTINGS } from "../types.ts";
import {
  __setCreateRootForTests as __setCommentWidgetCreateRoot,
  buildDecorations,
  type RemarginWidget,
} from "./commentWidget.ts";
import {
  __setCreateRootForTests as __setReadingModeCreateRoot,
  remarginPostProcessor,
} from "./readingModeProcessor.ts";

// The full-fence form: CM6 sees the on-disk markdown, so the parser needs the fences.
const VALID_BLOCK_C1 = [
  "```remargin",
  "---",
  "id: c1",
  "author: alice",
  "type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "first comment",
  "```",
].join("\n");

// The bare form `code.textContent` returns in reading mode, after the renderer strips the fences.
const VALID_BLOCK_C1_INNER = [
  "---",
  "id: c1",
  "author: alice",
  "type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "first comment",
].join("\n");

const VALID_BLOCK_C2 = [
  "```remargin",
  "---",
  "id: c2",
  "author: bob",
  "type: human",
  "ts: 2026-04-25T12:01:00-04:00",
  "---",
  "second comment",
  "```",
].join("\n");

const INVALID_BLOCK_NO_ID = [
  "```remargin",
  "---",
  "author: alice",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "missing id",
  "```",
].join("\n");

const INVALID_BLOCK_NO_ID_INNER = [
  "---",
  "author: alice",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "missing id",
].join("\n");

/**
 * Build the smallest `App` shape `RemarginPlugin.onload` and the
 * downstream foundation pieces actually touch. We never call `onload`
 * in these tests — we instantiate the plugin and populate just the
 * pretty-print foundation (settings, collapseState, focusEvents) by
 * hand. That keeps the e2e harness independent of update-probe and
 * workspace-event side-effects.
 */
function makeApp(): unknown {
  return {
    vault: {
      adapter: { basePath: "/tmp/test-vault" },
      getAbstractFileByPath: () => null,
      cachedRead: async () => "",
      on: () => ({}),
      offref: () => undefined,
    },
    workspace: {
      getActiveViewOfType: () => null,
      getLeavesOfType: () => [],
      getRightLeaf: () => null,
      getLeftLeaf: () => null,
      on: () => ({}),
      off: () => undefined,
      offref: () => undefined,
      onLayoutReady: () => undefined,
      revealLeaf: () => undefined,
    },
  };
}

function makeManifest(): unknown {
  return { version: "0.0.0-test", id: "remargin", name: "Remargin" };
}

/**
 * Stand up a real `RemarginPlugin` instance with the foundation
 * pieces populated, but skip `onload`. `editorWidgets` is set
 * explicitly per-test.
 */
function makePlugin(editorWidgets: boolean): RemarginPlugin {
  const plugin = new RemarginPlugin(makeApp() as never, makeManifest() as never);
  plugin.settings = { ...DEFAULT_SETTINGS, editorWidgets };
  plugin.collapseState = new CollapseState();
  plugin.focusEvents = new EventTarget();
  return plugin;
}

/**
 * Mock for the `<pre>` element produced by Obsidian's markdown
 * renderer. Tracks `replaceWith` so test #1 / #3 / #8 can assert the
 * raw fence stayed in place when the post-processor decided to skip.
 */
interface MockPreElement {
  replaced: boolean;
  replacement: unknown;
  replaceWith(node: unknown): void;
}

/** Mock for the `<code class="language-remargin">` element. */
interface MockCodeElement {
  textContent: string;
  parentElement: MockPreElement;
}

/** A stand-in for the widget's host element. */
interface MockHost {
  className: string;
  dataset: Record<string, string>;
  /** Mutable: the post-processor hides the host through it and `render()` un-hides it. */
  style: Record<string, string>;
  __remarginRoot?: { unmount: () => void; render: (element: unknown) => void };
}

function makePre(): MockPreElement {
  return {
    replaced: false,
    replacement: null,
    replaceWith(node) {
      this.replaced = true;
      this.replacement = node;
    },
  };
}

function makeCode(textContent: string): MockCodeElement {
  return { textContent, parentElement: makePre() };
}

function makeEl(codes: MockCodeElement[]): HTMLElement {
  return {
    querySelectorAll(_selector: string) {
      return codes;
    },
  } as unknown as HTMLElement;
}

/** Mock post-processor context that records the children added to it. */
interface MockCtx {
  sourcePath: string;
  __children: unknown[];
  addChild(child: unknown): void;
}

function makeCtx(sourcePath = "notes/test.md"): MockCtx {
  return {
    sourcePath,
    __children: [],
    addChild(child) {
      this.__children.push(child);
    },
  };
}

/**
 * Mock `EditorState` matching the surface area `buildDecorations` and
 * `commentWidgetPlugin`'s `StateField` create/update path actually
 * consume. Identical shape to the one in `commentWidget.test.ts` so
 * the e2e stays in lock-step with the unit-level test.
 *
 * The two `state.field` reads the production code performs are keyed
 * on `editorLivePreviewField` (mode probe) and `editorInfoField`
 * (source-path resolver). Both sentinel objects come from the
 * `obsidian` test stub.
 */
interface MockEditorState {
  doc: { toString(): string };
  field<T>(field: unknown, required: false): T | undefined;
}

function makeEditorState(opts: {
  doc: string;
  livePreview: boolean;
  sourcePath?: string;
}): MockEditorState {
  return {
    doc: { toString: () => opts.doc },
    field<T>(field: unknown, _required: false): T | undefined {
      if (field === editorLivePreviewField) {
        return opts.livePreview as unknown as T;
      }
      if (field === editorInfoField) {
        if (opts.sourcePath === undefined) return undefined;
        return { file: { path: opts.sourcePath } } as unknown as T;
      }
      return undefined;
    },
  };
}

// `globalThis.document` is replaced so both surfaces' `createElement("div")` return mocks.
let originalDocument: typeof globalThis.document | undefined;
const createdHosts: MockHost[] = [];

beforeEach(() => {
  originalDocument = (globalThis as { document?: typeof globalThis.document }).document;
  createdHosts.length = 0;
  (globalThis as { document?: unknown }).document = {
    createElement: (_tag: string) => {
      const host: MockHost = { className: "", dataset: {}, style: {} };
      createdHosts.push(host);
      return host;
    },
  };
});

afterEach(() => {
  if (originalDocument === undefined) {
    delete (globalThis as { document?: unknown }).document;
  } else {
    (globalThis as { document?: unknown }).document = originalDocument;
  }
  __setReadingModeCreateRoot(null);
  __setCommentWidgetCreateRoot(null);
});

/**
 * Pull the `onClick` prop from the wrapped widget tree. Both editor
 * surfaces render `<WidgetProviders>` directly wrapping a single
 * `<WidgetCommentThread>`; the thread component carries the `onClick`
 * prop. Descend one level (provider → child) to reach it.
 */
function findInnerOnClick(element: unknown): ((id: string, file: string) => void) | undefined {
  const wrapper = element as {
    props?: {
      children?: { props?: { onClick?: (id: string, file: string) => void } };
    };
  };
  const child = wrapper.props?.children;
  if (typeof child?.props?.onClick === "function") {
    return child.props.onClick;
  }
  return undefined;
}

/**
 * Helper: install a fake `createRoot` for the reading-mode side that
 * captures every rendered React element's `onClick` prop. Returns the
 * captured-callback array (newest at the end).
 */
function captureReadingModeOnClicks(): Array<(id: string, file: string) => void> {
  const captured: Array<(id: string, file: string) => void> = [];
  __setReadingModeCreateRoot(((_el: unknown) => ({
    render(element: unknown) {
      const onClick = findInnerOnClick(element);
      if (onClick) captured.push(onClick);
    },
    unmount() {
      /* test-only no-op */
    },
  })) as unknown as Parameters<typeof __setReadingModeCreateRoot>[0]);
  return captured;
}

/**
 * Helper: install a fake `createRoot` for the CM6 widget side that
 * captures every rendered React element's `onClick` prop.
 */
function captureCm6WidgetOnClicks(): Array<(id: string, file: string) => void> {
  const captured: Array<(id: string, file: string) => void> = [];
  __setCommentWidgetCreateRoot(((_el: unknown) => ({
    render(element: unknown) {
      const onClick = findInnerOnClick(element);
      if (onClick) captured.push(onClick);
    },
    unmount() {
      /* test-only no-op */
    },
  })) as unknown as Parameters<typeof __setCommentWidgetCreateRoot>[0]);
  return captured;
}

/** Subscribe to the plugin's focus bus and return the captured details. */
function captureFocusEvents(plugin: RemarginPlugin): RemarginFocusDetail[] {
  const captured: RemarginFocusDetail[] = [];
  plugin.focusEvents.addEventListener("remargin:focus", (event) => {
    const detail = (event as CustomEvent<RemarginFocusDetail>).detail;
    if (detail) captured.push(detail);
  });
  return captured;
}

describe("pretty-print end-to-end", () => {
  it("scenario 1: editorWidgets=true -> reading-mode widget replaces <pre>", () => {
    const plugin = makePlugin(true);
    captureReadingModeOnClicks();
    const code = makeCode(VALID_BLOCK_C1_INNER);
    const el = makeEl([code]);
    const ctx = makeCtx();

    const processor = remarginPostProcessor(plugin);
    processor(el, ctx as never);

    assert.equal(code.parentElement.replaced, true, "<pre> was replaced");
    assert.equal(createdHosts.length, 1, "exactly one reading-mode host element");
    const classes = createdHosts[0].className.split(/\s+/);
    assert.ok(
      classes.includes("remargin-reading-host"),
      `expected host className to include remargin-reading-host, got: "${createdHosts[0].className}"`
    );
    assert.ok(
      classes.includes("remargin-container"),
      `expected host className to include remargin-container, got: "${createdHosts[0].className}"`
    );
    assert.equal(createdHosts[0].dataset.remarginId, "c1");
    assert.equal(ctx.__children.length, 1, "ctx.addChild fired once");
  });

  it("scenario 2: editorWidgets=true + Live Preview -> CM6 builds 1 decoration", () => {
    const plugin = makePlugin(true);
    const state = makeEditorState({
      doc: VALID_BLOCK_C1,
      livePreview: true,
      sourcePath: "notes/test.md",
    });

    const decorations = buildDecorations(state as unknown as EditorState, plugin);
    assert.equal(decorations.size, 1, "exactly one decoration");
  });

  it("scenario 3: editorWidgets=false -> reading-mode no-op AND CM6 emits no decorations", () => {
    const plugin = makePlugin(false);
    const code = makeCode(VALID_BLOCK_C1_INNER);
    const el = makeEl([code]);
    const ctx = makeCtx();

    const processor = remarginPostProcessor(plugin);
    processor(el, ctx as never);
    assert.equal(code.parentElement.replaced, false, "<pre> stays untouched");
    assert.equal(ctx.__children.length, 0, "ctx.addChild not called");

    const state = makeEditorState({ doc: VALID_BLOCK_C1, livePreview: true });
    const decorations = buildDecorations(state as unknown as EditorState, plugin);
    assert.equal(decorations.size, 0, "CM6 emits no decorations when toggle off");
  });

  it("scenario 4: reading-mode click -> remargin:focus fires with (id, file)", () => {
    const plugin = makePlugin(true);
    const captured = captureReadingModeOnClicks();
    const focusDetails = captureFocusEvents(plugin);

    const code = makeCode(VALID_BLOCK_C1_INNER);
    const el = makeEl([code]);
    const ctx = makeCtx("notes/x.md");
    const processor = remarginPostProcessor(plugin);
    processor(el, ctx as never);

    // The stub's MarkdownRenderChild has no lifecycle, so `child.onload()` is driven by hand.
    const child = ctx.__children[0] as { onload: () => void; onunload: () => void };
    child.onload();

    assert.equal(captured.length, 1, "reading-mode rendered exactly one widget");
    captured[0]("c1", "notes/x.md");

    assert.deepStrictEqual(focusDetails, [{ commentId: "c1", file: "notes/x.md" }]);
    child.onunload();
  });

  it("scenario 5: CM6 widget click -> remargin:focus fires with (id, file)", () => {
    const plugin = makePlugin(true);
    const captured = captureCm6WidgetOnClicks();
    const focusDetails = captureFocusEvents(plugin);

    const state = makeEditorState({
      doc: VALID_BLOCK_C1,
      livePreview: true,
      sourcePath: "notes/y.md",
    });
    const decorations = buildDecorations(state as unknown as EditorState, plugin);
    let widget: RemarginWidget | null = null;
    decorations.between(0, state.doc.toString().length, (_f, _t, value) => {
      if (!widget) widget = (value as { spec: { widget: RemarginWidget } }).spec.widget;
    });
    assert.ok(widget, "expected one widget in the decoration set");
    (widget as RemarginWidget).toDOM();

    assert.equal(captured.length, 1, "CM6 widget rendered exactly once");
    captured[0]("c1", "notes/y.md");

    assert.deepStrictEqual(focusDetails, [{ commentId: "c1", file: "notes/y.md" }]);
  });

  // The shared CollapseState is the bridge between the two surfaces.
  it("scenario 6: collapse toggle (any surface) makes next CM6 widget !eq the previous", () => {
    const plugin = makePlugin(true);
    captureReadingModeOnClicks();

    const code = makeCode(VALID_BLOCK_C1_INNER);
    const el = makeEl([code]);
    const ctx = makeCtx();
    remarginPostProcessor(plugin)(el, ctx as never);
    const child = ctx.__children[0] as { onload: () => void; onunload: () => void };
    child.onload();

    const state = makeEditorState({ doc: VALID_BLOCK_C1, livePreview: true });
    const before = buildDecorations(state as unknown as EditorState, plugin);
    let widgetBefore: RemarginWidget | null = null;
    before.between(0, state.doc.toString().length, (_f, _t, value) => {
      if (!widgetBefore) widgetBefore = (value as { spec: { widget: RemarginWidget } }).spec.widget;
    });
    assert.ok(widgetBefore);

    plugin.collapseState.toggle("c1");

    const after = buildDecorations(state as unknown as EditorState, plugin);
    let widgetAfter: RemarginWidget | null = null;
    after.between(0, state.doc.toString().length, (_f, _t, value) => {
      if (!widgetAfter) widgetAfter = (value as { spec: { widget: RemarginWidget } }).spec.widget;
    });
    assert.ok(widgetAfter);

    assert.equal(
      (widgetBefore as RemarginWidget).eq(widgetAfter as RemarginWidget),
      false,
      "post-toggle CM6 widget must !eq the pre-toggle widget"
    );

    child.onunload();
  });

  it("scenario 7: Source Mode -> CM6 emits no decorations", () => {
    const plugin = makePlugin(true);
    const state = makeEditorState({ doc: VALID_BLOCK_C1, livePreview: false });
    const decorations = buildDecorations(state as unknown as EditorState, plugin);
    assert.equal(decorations.size, 0);
  });

  it("scenario 8: malformed block -> reading-mode untouched AND CM6 emits no decoration", () => {
    const plugin = makePlugin(true);

    const code = makeCode(INVALID_BLOCK_NO_ID_INNER);
    const el = makeEl([code]);
    const ctx = makeCtx();
    remarginPostProcessor(plugin)(el, ctx as never);
    assert.equal(code.parentElement.replaced, false, "<pre> stays in place");
    assert.equal(ctx.__children.length, 0, "ctx.addChild not called");

    // A valid block on either side of the malformed one: exactly two decorations must come out.
    const doc = `${VALID_BLOCK_C1}\n${INVALID_BLOCK_NO_ID}\n${VALID_BLOCK_C2}`;
    const state = makeEditorState({ doc, livePreview: true });
    const decorations = buildDecorations(state as unknown as EditorState, plugin);
    assert.equal(decorations.size, 2, "exactly two decorations: c1 and c2");

    const ids: string[] = [];
    decorations.between(0, state.doc.toString().length, (_f, _t, value) => {
      const widget = (value as { spec: { widget: RemarginWidget } }).spec.widget;
      const host = (() => {
        captureCm6WidgetOnClicks();
        return widget.toDOM();
      })();
      const id = (host as unknown as MockHost).dataset.remarginId;
      if (id) ids.push(id);
    });
    assert.deepStrictEqual(ids.sort(), ["c1", "c2"]);
  });
});
