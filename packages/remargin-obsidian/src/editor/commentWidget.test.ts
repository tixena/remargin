import { strict as assert } from "node:assert";
import { afterEach, beforeEach, describe, it } from "node:test";
import type { EditorView, WidgetType } from "@codemirror/view";
import type RemarginPlugin from "../main.ts";
import { type ParsedBlock, parseRemarginBlocks } from "../parser/parseRemarginBlocks.ts";
import { CollapseState } from "../state/collapseState.ts";
import { DEFAULT_SETTINGS } from "../types.ts";
import {
  __setCreateRootForTests,
  buildDecorations,
  commentWidgetPlugin,
  RemarginWidget,
} from "./commentWidget.ts";

/**
 * The CM6 `EditorView` and `ViewPlugin` machinery require a real DOM
 * to construct. The `node --test` harness here doesn't provide one
 * (no happy-dom installed), so we mock the surface area the
 * production code actually touches: `view.state.doc.toString()`,
 * `view.state.field(editorInfoField, false)`, and the
 * `view.dom.closest(".markdown-source-view")` chain used by the
 * Source-Mode-vs-Live-Preview detector.
 *
 * This is exactly the trade-off the ticket's "Mocks authorized" rule
 * permits: when a DOM API has no headless equivalent, mock it and
 * keep the AC closable.
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

function makeView(opts: {
  doc: string;
  livePreview: boolean;
  /** Overrides the source-path returned by editorInfoField. */
  sourcePath?: string;
  /** When true, the `.markdown-source-view` ancestor is missing entirely. */
  noSourceViewAncestor?: boolean;
}): MockEditorView {
  let ancestor: MockHostElement | null = null;
  if (!opts.noSourceViewAncestor) {
    const classes = new Set<string>(
      opts.livePreview ? ["markdown-source-view", "is-live-preview"] : []
    );
    ancestor = {
      classes,
      classList: { contains: (name: string) => classes.has(name) },
    };
  }

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

interface MockPlugin {
  settings: { editorWidgets: boolean };
  collapseState: CollapseState;
  focusComment: (id: string, file: string) => void;
  __focusCalls: Array<[string, string]>;
}

function makePlugin(editorWidgets: boolean): MockPlugin {
  const focusCalls: Array<[string, string]> = [];
  const plugin: MockPlugin = {
    settings: { ...DEFAULT_SETTINGS, editorWidgets },
    collapseState: new CollapseState(),
    focusComment(id, file) {
      focusCalls.push([id, file]);
    },
    __focusCalls: focusCalls,
  };
  return plugin;
}

const VALID_BLOCK = [
  "```remargin",
  "---",
  "id: c1",
  "author: alice",
  "author_type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "hello widget",
  "```",
].join("\n");

const INVALID_BLOCK_NO_ID = [
  "```remargin",
  "---",
  "author: alice",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "no id here",
  "```",
].join("\n");

interface MockHost {
  className: string;
  dataset: Record<string, string>;
  __remarginRoot?: { unmount: () => void; render: (element: unknown) => void };
}

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
  __setCreateRootForTests(null);
});

/**
 * `Decoration.none` is the expected return when the plugin should
 * emit nothing. Asserting via `.size === 0` covers both
 * `Decoration.none` and any future "empty RangeSet" returned by the
 * builder, since they share that contract.
 */
function assertNoDecorations(view: MockEditorView, plugin: MockPlugin) {
  const decorations = buildDecorations(
    view as unknown as EditorView,
    plugin as unknown as RemarginPlugin
  );
  assert.equal(decorations.size, 0, "expected zero decorations");
}

describe("commentWidget buildDecorations", () => {
  // AC: build() returns Decoration.none when editorWidgets === false.
  it("test #1: setting off → Decoration.none", () => {
    const plugin = makePlugin(false);
    const view = makeView({ doc: VALID_BLOCK, livePreview: true });
    assertNoDecorations(view, plugin);
  });

  // AC: build() returns Decoration.none when in Source Mode.
  it("test #2: source mode (no is-live-preview class) → Decoration.none", () => {
    const plugin = makePlugin(true);
    const view = makeView({ doc: VALID_BLOCK, livePreview: false });
    assertNoDecorations(view, plugin);
  });

  // Defensive companion: the .markdown-source-view ancestor is missing
  // outright (e.g. an unrooted editor) — still bails out cleanly.
  it("test #2b: missing .markdown-source-view ancestor → Decoration.none", () => {
    const plugin = makePlugin(true);
    const view = makeView({ doc: VALID_BLOCK, livePreview: false, noSourceViewAncestor: true });
    assertNoDecorations(view, plugin);
  });

  // AC: Live Preview + valid block → Decoration.replace with block:true,
  // inclusive:false.
  it("test #3: live preview + valid block → 1 replace decoration with block/inclusive flags", () => {
    const plugin = makePlugin(true);
    const view = makeView({ doc: VALID_BLOCK, livePreview: true, sourcePath: "notes/test.md" });
    const decorations = buildDecorations(
      view as unknown as EditorView,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 1, "exactly one decoration expected");

    // Walk the produced RangeSet to fish out the spec.
    const collected: Array<{ from: number; to: number; spec: unknown }> = [];
    decorations.between(0, view.state.doc.toString().length, (from, to, value) => {
      collected.push({ from, to, spec: (value as { spec: unknown }).spec });
    });
    assert.equal(collected.length, 1);
    const spec = collected[0].spec as {
      widget: WidgetType;
      block: boolean;
      inclusive: boolean;
    };
    assert.ok(spec.widget instanceof RemarginWidget, "widget is a RemarginWidget");
    assert.equal(spec.block, true, "block: true");
    assert.equal(spec.inclusive, false, "inclusive: false");
    // Range matches the parser's startOffset/endOffset for the single block.
    const parsed = parseRemarginBlocks(VALID_BLOCK)[0];
    assert.equal(collected[0].from, parsed.startOffset);
    assert.equal(collected[0].to, parsed.endOffset);
  });

  // AC: Malformed blocks emit no decoration; valid ones still do.
  it("test #4: 1 valid + 1 malformed → exactly 1 decoration (the valid one)", () => {
    const plugin = makePlugin(true);
    const doc = `${VALID_BLOCK}\n${INVALID_BLOCK_NO_ID}`;
    const view = makeView({ doc, livePreview: true });
    const decorations = buildDecorations(
      view as unknown as EditorView,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 1);
  });
});

describe("commentWidgetPlugin update lifecycle", () => {
  // AC: update() rebuilds ONLY on docChanged. Viewport / selection
  // updates do NOT rebuild.
  it("test #5: viewport-only update → build() not called", () => {
    const plugin = makePlugin(true);
    const view = makeView({ doc: VALID_BLOCK, livePreview: true });
    const extension = commentWidgetPlugin(plugin as unknown as RemarginPlugin);

    // CM6's ViewPlugin spec exposes the inner class via `.create(view)`
    // which is the same code path CM6 itself uses to instantiate the
    // plugin. We can drive `update` on the resulting instance without
    // a real EditorView because our mock view satisfies every
    // `state.doc.toString()` and `dom.closest(...)` access the build
    // path performs.
    const instance = (
      extension as unknown as { create: (v: unknown) => { update: (u: unknown) => void } }
    ).create(view);

    let buildCalls = 0;
    const originalDecorations = (instance as unknown as { decorations: unknown }).decorations;
    Object.defineProperty(instance, "decorations", {
      configurable: true,
      get() {
        return originalDecorations;
      },
      set(_value: unknown) {
        buildCalls += 1;
      },
    });

    // Simulate a viewport-only update — docChanged is false.
    instance.update({ docChanged: false, viewportChanged: true, view });
    assert.equal(buildCalls, 0, "viewport-only update must NOT rebuild");
  });

  it("test #6: docChanged update → build() called once", () => {
    const plugin = makePlugin(true);
    const view = makeView({ doc: VALID_BLOCK, livePreview: true });
    const extension = commentWidgetPlugin(plugin as unknown as RemarginPlugin);
    const instance = (
      extension as unknown as { create: (v: unknown) => { update: (u: unknown) => void } }
    ).create(view);

    let buildCalls = 0;
    const originalDecorations = (instance as unknown as { decorations: unknown }).decorations;
    Object.defineProperty(instance, "decorations", {
      configurable: true,
      get() {
        return originalDecorations;
      },
      set(_value: unknown) {
        buildCalls += 1;
      },
    });

    instance.update({ docChanged: true, viewportChanged: false, view });
    assert.equal(buildCalls, 1, "docChanged update rebuilds once");
  });
});

describe("RemarginWidget", () => {
  function block(text = VALID_BLOCK): ParsedBlock {
    const result = parseRemarginBlocks(text)[0];
    assert.ok(result?.valid, "test fixture must be valid");
    return result;
  }

  // AC: eq() is true when id + collapsed + content all match.
  it("test #7: eq() returns true for same id + same collapsed + same content", () => {
    const plugin = makePlugin(true);
    const a = new RemarginWidget(block(), plugin as unknown as RemarginPlugin, "f.md");
    const b = new RemarginWidget(block(), plugin as unknown as RemarginPlugin, "f.md");
    assert.equal(a.eq(b), true);
  });

  // AC: eq() is false when collapsed state differs (proves toggling
  // forces a re-render).
  it("test #8: eq() returns false when collapsed state differs", () => {
    const pluginA = makePlugin(true); // c1 collapsed by default (true)
    const pluginB = makePlugin(true);
    pluginB.collapseState.toggle("c1"); // c1 now expanded
    const a = new RemarginWidget(block(), pluginA as unknown as RemarginPlugin, "f.md");
    const b = new RemarginWidget(block(), pluginB as unknown as RemarginPlugin, "f.md");
    assert.equal(a.eq(b), false);
  });

  // AC: eq() is false when content (raw text) differs for the same id.
  it("test #9: eq() returns false when raw content differs for same id", () => {
    const plugin = makePlugin(true);
    const original = block();
    const edited: ParsedBlock = {
      ...original,
      raw: `${original.raw}\nedited line`,
    };
    const a = new RemarginWidget(original, plugin as unknown as RemarginPlugin, "f.md");
    const b = new RemarginWidget(edited, plugin as unknown as RemarginPlugin, "f.md");
    assert.equal(a.eq(b), false);
  });

  // AC: ignoreEvent() returns true.
  it("test #10: ignoreEvent() returns true (does not eat keystrokes)", () => {
    const plugin = makePlugin(true);
    const widget = new RemarginWidget(block(), plugin as unknown as RemarginPlugin, "f.md");
    assert.equal(widget.ignoreEvent(), true);
  });

  // AC: toDOM() mounts a React root via createRoot; destroy() unmounts.
  it("test #11: toDOM mounts a root; destroy unmounts it", () => {
    const plugin = makePlugin(true);

    let renderCalls = 0;
    let unmountCalls = 0;
    let createRootCalls = 0;
    __setCreateRootForTests(((_el: unknown) => {
      createRootCalls += 1;
      return {
        render: () => {
          renderCalls += 1;
        },
        unmount: () => {
          unmountCalls += 1;
        },
      };
    }) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    const widget = new RemarginWidget(block(), plugin as unknown as RemarginPlugin, "f.md");
    const dom = widget.toDOM();
    assert.equal(createRootCalls, 1);
    assert.equal(renderCalls, 1, "render must run once on mount");
    assert.equal((dom as MockHost).className, "remargin-widget-host");
    assert.equal((dom as MockHost).dataset.remarginId, "c1");

    widget.destroy(dom);
    assert.equal(unmountCalls, 1, "unmount must run once on destroy");
  });

  // AC: Click on the host fires plugin.focusComment(id, sourcePath).
  it("test #12: widget onClick prop forwards to plugin.focusComment", () => {
    const plugin = makePlugin(true);

    let captured: ((id: string, file: string) => void) | undefined;
    __setCreateRootForTests(((_el: unknown) => ({
      render: (element: unknown) => {
        const node = element as { props?: { onClick?: (id: string, file: string) => void } };
        if (typeof node.props?.onClick === "function") captured = node.props.onClick;
      },
      unmount: () => {
        /* test-only no-op */
      },
    })) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    const widget = new RemarginWidget(block(), plugin as unknown as RemarginPlugin, "notes/x.md");
    widget.toDOM();
    assert.ok(captured, "expected an onClick prop on the rendered widget");
    captured("c1", "notes/x.md");
    assert.deepStrictEqual(plugin.__focusCalls, [["c1", "notes/x.md"]]);
  });

  // AC: Toggling collapse forces the next build() to produce a widget
  // that eq-differs from the previous one for that id.
  it("test #13: collapse toggle makes the next-built widget !eq the previous", () => {
    const plugin = makePlugin(true);
    const view = makeView({ doc: VALID_BLOCK, livePreview: true });
    const before = buildDecorations(
      view as unknown as EditorView,
      plugin as unknown as RemarginPlugin
    );
    plugin.collapseState.toggle("c1");
    const after = buildDecorations(
      view as unknown as EditorView,
      plugin as unknown as RemarginPlugin
    );

    function pickWidget(set: ReturnType<typeof buildDecorations>): WidgetType {
      let found: WidgetType | null = null;
      set.between(0, view.state.doc.toString().length, (_f, _t, value) => {
        if (found) return;
        found = (value as { spec: { widget: WidgetType } }).spec.widget;
      });
      assert.ok(found);
      return found;
    }
    const widgetBefore = pickWidget(before);
    const widgetAfter = pickWidget(after);
    assert.equal(
      widgetBefore.eq(widgetAfter),
      false,
      "post-toggle widget must differ from pre-toggle widget"
    );
  });
});

describe("commentWidget defaults", () => {
  // AC: settings.editorWidgets default remains false.
  it("DEFAULT_SETTINGS.editorWidgets is false", () => {
    assert.equal(DEFAULT_SETTINGS.editorWidgets, false);
  });
});
