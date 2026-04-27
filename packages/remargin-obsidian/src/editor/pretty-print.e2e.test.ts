import { strict as assert } from "node:assert";
import { afterEach, beforeEach, describe, it } from "node:test";
import type { EditorView } from "@codemirror/view";
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

/**
 * End-to-end coverage for the pretty-print stack (T39 / rem-fyj8.4).
 *
 * Unlike the per-file tests for `readingModeProcessor` and
 * `commentWidget`, the scenarios below exercise the *combined* wiring:
 * a real `RemarginPlugin` instance, a real `CollapseState`, the real
 * `focusEvents` `EventTarget`, the real post-processor, and the real
 * CM6 build path — all mounted against the same plugin so a click in
 * one surface ends up on `plugin.focusEvents` for any subscriber to
 * see, and a collapse toggle in one surface invalidates the next CM6
 * `build()` for the matching id.
 *
 * Mocking strategy (per the T39 ticket's "Mocks authorized" table):
 *
 *  - `obsidian` is replaced module-wide by `test-obsidian-stub.mjs`
 *    via the package's test loader. No real Obsidian runtime.
 *  - `MarkdownRenderer.render` is the obsidian stub's no-op.
 *  - `react-dom/client` is replaced via the per-module
 *    `__setCreateRootForTests` seams already present on the
 *    production code; we install fakes that capture the rendered
 *    React element so we can pull the `onClick` prop and invoke it
 *    directly. This is the same pattern T37 + T38 use — the test
 *    stack has no happy-dom dependency, so we cannot mount a real
 *    React tree.
 *  - `@codemirror/view`'s `EditorView` is mocked at the surface area
 *    `buildDecorations` actually consumes (`view.state.doc.toString`,
 *    `view.dom.closest`, `view.state.field`). Same mock pattern that
 *    `commentWidget.test.ts` lands; reused here unchanged so the e2e
 *    layer stays consistent with the unit layer.
 *
 * Trade-off documented in the T38 close-out note: until happy-dom (or
 * another DOM polyfill) is added to this package's devDependencies,
 * a "real" `EditorView.create` cannot be instantiated headlessly. The
 * surface-area mock covers every method `buildDecorations` and the
 * `ViewPlugin.create` path actually call, so the production code is
 * exercised on its real path while the runtime DOM is faked.
 */

const VALID_BLOCK_C1 = [
  "```remargin",
  "---",
  "id: c1",
  "author: alice",
  "author_type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "first comment",
  "```",
].join("\n");

const VALID_BLOCK_C2 = [
  "```remargin",
  "---",
  "id: c2",
  "author: bob",
  "author_type: human",
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
    vault: { adapter: { basePath: "/tmp/test-vault" } },
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

interface MockCodeElement {
  textContent: string;
  parentElement: MockPreElement;
}

interface MockHost {
  className: string;
  dataset: Record<string, string>;
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
 * Mock `EditorView` matching the surface area `buildDecorations` and
 * `commentWidgetPlugin`'s `ViewPlugin.create` path actually consume.
 * Identical shape to the one in `commentWidget.test.ts` so the e2e
 * stays in lock-step with the unit-level test.
 */
interface MockHostElement {
  classes: Set<string>;
  classList: { contains(name: string): boolean };
}

interface MockClosestRoot {
  closest(selector: string): MockHostElement | null;
}

interface MockEditorView {
  dom: MockClosestRoot;
  state: {
    doc: { toString(): string };
    field<T>(field: unknown, required: false): T | undefined;
  };
}

function makeEditorView(opts: {
  doc: string;
  livePreview: boolean;
  sourcePath?: string;
}): MockEditorView {
  const classes = new Set<string>(
    opts.livePreview ? ["markdown-source-view", "is-live-preview"] : ["markdown-source-view"]
  );
  const ancestor: MockHostElement = {
    classes,
    classList: { contains: (name: string) => classes.has(name) },
  };
  return {
    dom: {
      closest(selector) {
        if (selector === ".markdown-source-view") return ancestor;
        return null;
      },
    },
    state: {
      doc: { toString: () => opts.doc },
      field<T>(_field: unknown, _required: false): T | undefined {
        if (opts.sourcePath === undefined) return undefined;
        return { file: { path: opts.sourcePath } } as unknown as T;
      },
    },
  };
}

/**
 * Override `globalThis.document` so the post-processor's
 * `document.createElement("div")` and the CM6 widget's
 * `document.createElement("div")` (inside `RemarginWidget.toDOM`)
 * return controllable mocks. Restored in `afterEach`.
 */
let originalDocument: typeof globalThis.document | undefined;
const createdHosts: MockHost[] = [];

beforeEach(() => {
  originalDocument = (globalThis as { document?: typeof globalThis.document }).document;
  createdHosts.length = 0;
  (globalThis as { document?: unknown }).document = {
    createElement: (_tag: string) => {
      const host: MockHost = { className: "", dataset: {} };
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
 * Helper: install a fake `createRoot` for the reading-mode side that
 * captures every rendered React element's `onClick` prop. Returns the
 * captured-callback array (newest at the end).
 */
function captureReadingModeOnClicks(): Array<(id: string, file: string) => void> {
  const captured: Array<(id: string, file: string) => void> = [];
  __setReadingModeCreateRoot(((_el: unknown) => ({
    render(element: unknown) {
      const node = element as { props?: { onClick?: (id: string, file: string) => void } };
      if (typeof node.props?.onClick === "function") captured.push(node.props.onClick);
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
      const node = element as { props?: { onClick?: (id: string, file: string) => void } };
      if (typeof node.props?.onClick === "function") captured.push(node.props.onClick);
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

describe("pretty-print end-to-end (T39 / rem-fyj8.4)", () => {
  // Scenario 1: toggle on -> reading-mode widget renders (post-processor
  // replaces the <pre> and registers a child).
  it("scenario 1: editorWidgets=true -> reading-mode widget replaces <pre>", () => {
    const plugin = makePlugin(true);
    captureReadingModeOnClicks();
    const code = makeCode(VALID_BLOCK_C1);
    const el = makeEl([code]);
    const ctx = makeCtx();

    const processor = remarginPostProcessor(plugin);
    processor(el, ctx as never);

    assert.equal(code.parentElement.replaced, true, "<pre> was replaced");
    assert.equal(createdHosts.length, 1, "exactly one reading-mode host element");
    assert.equal(createdHosts[0].className, "remargin-reading-host");
    assert.equal(createdHosts[0].dataset.remarginId, "c1");
    assert.equal(ctx.__children.length, 1, "ctx.addChild fired once");
  });

  // Scenario 2: toggle on -> CM6 builds exactly one decoration in
  // Live Preview.
  it("scenario 2: editorWidgets=true + Live Preview -> CM6 builds 1 decoration", () => {
    const plugin = makePlugin(true);
    const view = makeEditorView({
      doc: VALID_BLOCK_C1,
      livePreview: true,
      sourcePath: "notes/test.md",
    });

    const decorations = buildDecorations(view as unknown as EditorView, plugin);
    assert.equal(decorations.size, 1, "exactly one decoration");
  });

  // Scenario 3: toggle off -> both surfaces leave the raw fence in
  // place.
  it("scenario 3: editorWidgets=false -> reading-mode no-op AND CM6 emits no decorations", () => {
    const plugin = makePlugin(false);
    const code = makeCode(VALID_BLOCK_C1);
    const el = makeEl([code]);
    const ctx = makeCtx();

    const processor = remarginPostProcessor(plugin);
    processor(el, ctx as never);
    assert.equal(code.parentElement.replaced, false, "<pre> stays untouched");
    assert.equal(ctx.__children.length, 0, "ctx.addChild not called");

    const view = makeEditorView({ doc: VALID_BLOCK_C1, livePreview: true });
    const decorations = buildDecorations(view as unknown as EditorView, plugin);
    assert.equal(decorations.size, 0, "CM6 emits no decorations when toggle off");
  });

  // Scenario 4: clicking the reading-mode widget invokes
  // plugin.focusComment, which fires `remargin:focus` on the plugin's
  // focus bus. This is the bridge contract a sidebar subscriber relies
  // on.
  it("scenario 4: reading-mode click -> remargin:focus fires with (id, file)", () => {
    const plugin = makePlugin(true);
    const captured = captureReadingModeOnClicks();
    const focusDetails = captureFocusEvents(plugin);

    const code = makeCode(VALID_BLOCK_C1);
    const el = makeEl([code]);
    const ctx = makeCtx("notes/x.md");
    const processor = remarginPostProcessor(plugin);
    processor(el, ctx as never);

    // The post-processor calls ctx.addChild(child); we have to drive
    // child.onload() to mount the React root because the obsidian
    // stub's MarkdownRenderChild has no automatic lifecycle. Once
    // mounted, the captured onClick prop wires through to
    // plugin.focusComment.
    const child = ctx.__children[0] as { onload: () => void; onunload: () => void };
    child.onload();

    assert.equal(captured.length, 1, "reading-mode rendered exactly one widget");
    captured[0]("c1", "notes/x.md");

    assert.deepStrictEqual(focusDetails, [{ commentId: "c1", file: "notes/x.md" }]);
    child.onunload();
  });

  // Scenario 5: clicking the CM6 widget invokes plugin.focusComment
  // through the same bridge.
  it("scenario 5: CM6 widget click -> remargin:focus fires with (id, file)", () => {
    const plugin = makePlugin(true);
    const captured = captureCm6WidgetOnClicks();
    const focusDetails = captureFocusEvents(plugin);

    // Build the parsed block and instantiate a real RemarginWidget
    // through the production constructor path. We could equivalently
    // walk the build()-produced RangeSet, but constructing directly
    // gives us a stable handle to call toDOM() on.
    const view = makeEditorView({
      doc: VALID_BLOCK_C1,
      livePreview: true,
      sourcePath: "notes/y.md",
    });
    const decorations = buildDecorations(view as unknown as EditorView, plugin);
    let widget: RemarginWidget | null = null;
    decorations.between(0, view.state.doc.toString().length, (_f, _t, value) => {
      if (!widget) widget = (value as { spec: { widget: RemarginWidget } }).spec.widget;
    });
    assert.ok(widget, "expected one widget in the decoration set");
    (widget as RemarginWidget).toDOM();

    assert.equal(captured.length, 1, "CM6 widget rendered exactly once");
    captured[0]("c1", "notes/y.md");

    assert.deepStrictEqual(focusDetails, [{ commentId: "c1", file: "notes/y.md" }]);
  });

  // Scenario 6: a collapse toggle in reading mode invalidates the next
  // CM6 build for the matching id (eq() returns false) — proving the
  // shared CollapseState is the bridge between the two surfaces.
  it("scenario 6: collapse toggle (any surface) makes next CM6 widget !eq the previous", () => {
    const plugin = makePlugin(true);
    captureReadingModeOnClicks();

    // 1. Mount the reading-mode widget so it has a chance to register
    //    its collapse subscription. (This is the "reading-mode side" of
    //    the bridge; the toggle-flow it owns is symmetric across
    //    surfaces, so the assertion that follows holds whichever
    //    surface initiated.)
    const code = makeCode(VALID_BLOCK_C1);
    const el = makeEl([code]);
    const ctx = makeCtx();
    remarginPostProcessor(plugin)(el, ctx as never);
    const child = ctx.__children[0] as { onload: () => void; onunload: () => void };
    child.onload();

    // 2. Capture the CM6 widget BEFORE the toggle.
    const view = makeEditorView({ doc: VALID_BLOCK_C1, livePreview: true });
    const before = buildDecorations(view as unknown as EditorView, plugin);
    let widgetBefore: RemarginWidget | null = null;
    before.between(0, view.state.doc.toString().length, (_f, _t, value) => {
      if (!widgetBefore) widgetBefore = (value as { spec: { widget: RemarginWidget } }).spec.widget;
    });
    assert.ok(widgetBefore);

    // 3. Toggle through the SHARED plugin.collapseState. Reading-mode
    //    and CM6 both consume this same instance, which is the whole
    //    point of the bridge.
    plugin.collapseState.toggle("c1");

    // 4. Capture the CM6 widget AFTER the toggle.
    const after = buildDecorations(view as unknown as EditorView, plugin);
    let widgetAfter: RemarginWidget | null = null;
    after.between(0, view.state.doc.toString().length, (_f, _t, value) => {
      if (!widgetAfter) widgetAfter = (value as { spec: { widget: RemarginWidget } }).spec.widget;
    });
    assert.ok(widgetAfter);

    // 5. eq() must say "different" — that is what forces CM6 to
    //    destroy + rebuild the DOM for that id, mirroring the toggle.
    assert.equal(
      (widgetBefore as RemarginWidget).eq(widgetAfter as RemarginWidget),
      false,
      "post-toggle CM6 widget must !eq the pre-toggle widget"
    );

    child.onunload();
  });

  // Scenario 7: Source Mode (no `is-live-preview` ancestor class) ->
  // CM6 emits zero decorations regardless of the toggle.
  it("scenario 7: Source Mode -> CM6 emits no decorations", () => {
    const plugin = makePlugin(true);
    const view = makeEditorView({ doc: VALID_BLOCK_C1, livePreview: false });
    const decorations = buildDecorations(view as unknown as EditorView, plugin);
    assert.equal(decorations.size, 0);
  });

  // Scenario 8: a malformed remargin block leaves both surfaces with
  // the raw fence — neither the post-processor nor the CM6 builder
  // emits a widget for it.
  it("scenario 8: malformed block -> reading-mode untouched AND CM6 emits no decoration", () => {
    const plugin = makePlugin(true);

    // Reading-mode side: post-processor must skip the malformed code
    // element entirely.
    const code = makeCode(INVALID_BLOCK_NO_ID);
    const el = makeEl([code]);
    const ctx = makeCtx();
    remarginPostProcessor(plugin)(el, ctx as never);
    assert.equal(code.parentElement.replaced, false, "<pre> stays in place");
    assert.equal(ctx.__children.length, 0, "ctx.addChild not called");

    // CM6 side: build() must yield a decoration set that contains zero
    // entries for the malformed block. To make the surrounding-blocks
    // case explicit, we sandwich a valid block on either side and
    // assert exactly two decorations come out (one per valid block,
    // none for the malformed one in the middle).
    const doc = `${VALID_BLOCK_C1}\n${INVALID_BLOCK_NO_ID}\n${VALID_BLOCK_C2}`;
    const view = makeEditorView({ doc, livePreview: true });
    const decorations = buildDecorations(view as unknown as EditorView, plugin);
    assert.equal(decorations.size, 2, "exactly two decorations: c1 and c2");

    const ids: string[] = [];
    decorations.between(0, view.state.doc.toString().length, (_f, _t, value) => {
      const widget = (value as { spec: { widget: RemarginWidget } }).spec.widget;
      // RemarginWidget exposes its id privately; round-trip via toDOM
      // is the only public hook. We installed the fake createRoot
      // earlier in the global afterEach reset, so reinstall here for
      // the cm6 side.
      const host = (() => {
        captureCm6WidgetOnClicks();
        return widget.toDOM();
      })();
      const id = (host as MockHost).dataset.remarginId;
      if (id) ids.push(id);
    });
    assert.deepStrictEqual(ids.sort(), ["c1", "c2"]);
  });
});
