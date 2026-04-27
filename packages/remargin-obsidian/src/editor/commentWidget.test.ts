import { strict as assert } from "node:assert";
import { afterEach, beforeEach, describe, it } from "node:test";
import { type EditorState, StateField } from "@codemirror/state";
import type { WidgetType } from "@codemirror/view";
import { editorInfoField, editorLivePreviewField } from "obsidian";
import { WidgetCommentView } from "../components/widget/WidgetCommentView.tsx";
import { WidgetProviders } from "../components/widget/WidgetProviders.tsx";
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
 * The CM6 `EditorView` machinery requires a real DOM to construct, and
 * the `node --test` harness here doesn't provide one (no happy-dom
 * installed). The post-rem-3dra host is a `StateField`, so tests no
 * longer need a fake `view.dom.closest(...)` chain — they just need a
 * minimal `EditorState` shape exposing the two surfaces production
 * code touches: `state.doc.toString()` and `state.field(field, false)`.
 *
 * The `field()` impl is a per-test record keyed on the sentinel field
 * objects exported from the `obsidian` test stub
 * (`editorLivePreviewField`, `editorInfoField`). This is exactly the
 * trade-off the ticket's "Mocks authorized" rule permits.
 */
interface MockEditorState {
  doc: { toString(): string };
  field<T>(field: unknown, required: false): T | undefined;
}

interface MakeStateOpts {
  doc: string;
  /**
   * Value to return for `state.field(editorLivePreviewField, false)`.
   * `undefined` simulates the field being absent (the `try`/`catch`
   * fallback returns `false`).
   */
  livePreview: boolean | undefined;
  /** Overrides the source-path returned by `editorInfoField`. */
  sourcePath?: string;
}

function makeState(opts: MakeStateOpts): MockEditorState {
  return {
    doc: { toString: () => opts.doc },
    field<T>(field: unknown, _required: false): T | undefined {
      if (field === editorLivePreviewField) {
        return opts.livePreview as unknown as T | undefined;
      }
      if (field === editorInfoField) {
        if (opts.sourcePath === undefined) return undefined;
        return { file: { path: opts.sourcePath } } as unknown as T;
      }
      return undefined;
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

const VALID_BLOCK_2 = [
  "```remargin",
  "---",
  "id: c2",
  "author: bob",
  "author_type: human",
  "ts: 2026-04-25T13:00:00-04:00",
  "---",
  "second widget",
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
function assertNoDecorations(state: MockEditorState, plugin: MockPlugin) {
  const decorations = buildDecorations(
    state as unknown as EditorState,
    plugin as unknown as RemarginPlugin
  );
  assert.equal(decorations.size, 0, "expected zero decorations");
}

describe("commentWidget buildDecorations", () => {
  // AC: build() returns Decoration.none when editorWidgets === false.
  it("test #1: setting off → Decoration.none", () => {
    const plugin = makePlugin(false);
    const state = makeState({ doc: VALID_BLOCK, livePreview: true });
    assertNoDecorations(state, plugin);
  });

  // AC: build() returns Decoration.none when in Source Mode.
  it("test #2: source mode (livePreview field === false) → Decoration.none", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: false });
    assertNoDecorations(state, plugin);
  });

  // AC: isLivePreviewState reads editorLivePreviewField. The helper
  // is module-private; we exercise it transitively through
  // buildDecorations, which is the only caller in production. The
  // three sub-tests cover the truth-table the ticket spelled out.
  it("test #2a: livePreview field === true → buildDecorations gates open", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: true });
    const decorations = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 1, "live preview true must produce a decoration");
  });

  it("test #2b: livePreview field === false → buildDecorations gates closed", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: false });
    const decorations = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 0, "live preview false must NOT produce a decoration");
  });

  it("test #2c: livePreview field absent → defaults to false (no throw, no decorations)", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: undefined });
    const decorations = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 0, "absent field must default to false → 0 decorations");
  });

  // AC: Live Preview + valid block → Decoration.replace with block:true,
  // inclusive:false.
  it("test #3: live preview + valid block → 1 replace decoration with block/inclusive flags", () => {
    const plugin = makePlugin(true);
    const state = makeState({
      doc: VALID_BLOCK,
      livePreview: true,
      sourcePath: "notes/test.md",
    });
    const decorations = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 1, "exactly one decoration expected");

    // Walk the produced RangeSet to fish out the spec.
    const collected: Array<{ from: number; to: number; spec: unknown }> = [];
    decorations.between(0, state.doc.toString().length, (from, to, value) => {
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
    const state = makeState({ doc, livePreview: true });
    const decorations = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 1);
  });
});

describe("commentWidgetPlugin StateField update lifecycle", () => {
  // Helper: pull the StateFieldSpec out of the StateField the
  // factory returns. CM6's StateField stores its create/update
  // callbacks on private `createF` / `updateF` slots; we don't
  // reach in there. Instead, drive the field through CM6's public
  // contract — `extension` plugged into a real EditorState — and
  // observe `state.field(field)` before and after a transaction.
  //
  // For unit-test purposes we call create/update via the spec stash
  // exposed on the StateField instance. CM6 doesn't publicize this,
  // but the StateField object also IS the field key, so we can use
  // it both as the lookup key and as the spec carrier through the
  // simple test shim below.
  function specFromField(field: unknown): {
    create: (state: EditorState) => unknown;
    update: (value: unknown, tr: unknown) => unknown;
  } {
    // The StateField class stores create/update on private slots
    // named `createF` / `updateF` (visible on the prototype's
    // closure). Read them off via known property names.
    const f = field as { createF?: unknown; updateF?: unknown };
    return {
      create: f.createF as (state: EditorState) => unknown,
      update: f.updateF as (value: unknown, tr: unknown) => unknown,
    };
  }

  // AC: update() rebuilds ONLY on docChanged. Viewport / selection
  // updates do NOT rebuild — they remap the existing decorations.
  it("test #5: docChanged update → buildDecorations called (re-parse)", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: true });
    const field = commentWidgetPlugin(plugin as unknown as RemarginPlugin);
    const spec = specFromField(field);

    const initial = spec.create(state as unknown as EditorState) as {
      size: number;
    };
    assert.equal(initial.size, 1, "initial create produces 1 decoration");

    // Simulate a docChanged transaction that swaps in a different
    // valid block. The update path re-runs buildDecorations against
    // the new state — we observe the rebuild by counting decorations
    // against the new doc.
    const nextState = makeState({
      doc: `${VALID_BLOCK}\n${VALID_BLOCK_2}`,
      livePreview: true,
    });
    const tr = {
      docChanged: true,
      state: nextState as unknown as EditorState,
      changes: { mapPos: (pos: number) => pos },
    };
    const next = spec.update(initial, tr) as { size: number };
    assert.equal(next.size, 2, "docChanged rebuild produces 2 decorations against the new doc");
  });

  it("test #6: non-docChanged update → existing decorations remapped, NOT rebuilt", () => {
    const plugin = makePlugin(true);
    const field = commentWidgetPlugin(plugin as unknown as RemarginPlugin);
    const spec = specFromField(field);

    // Use a sentinel "previous decorations" object whose only
    // surface is `.map(changes)`. If production goes through the
    // remap branch it will call this spy; if it falls into the
    // rebuild branch it will reach for buildDecorations and IGNORE
    // the sentinel. Returning a distinct sentinel value from the
    // spy lets us assert both that .map was called AND that the
    // production code returned the spy's output verbatim (i.e.
    // didn't re-parse).
    let mapCalls = 0;
    const remapSentinel = Symbol("remapped-decorations");
    const previous = {
      size: 1,
      map: (_changes: unknown) => {
        mapCalls += 1;
        return remapSentinel;
      },
    };

    const tr = {
      docChanged: false,
      // `state` is unused on the non-docChanged branch; pass a
      // throw-on-touch sentinel to make any accidental access
      // crash loudly.
      state: new Proxy(
        {},
        {
          get(_t, prop) {
            throw new Error(
              `tr.state should not be read on non-docChanged branch (got: ${String(prop)})`
            );
          },
        }
      ) as unknown as EditorState,
      changes: { __sentinel: "changes" },
    };
    const next = spec.update(previous, tr);
    assert.equal(mapCalls, 1, "non-docChanged update must call decorations.map exactly once");
    assert.equal(
      next,
      remapSentinel,
      "non-docChanged update must return the .map() result verbatim"
    );
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
    // Host must carry both the structural class AND `remargin-container`
    // so Tailwind utilities scoped via the `important` selector apply
    // inside the widget. See ticket rem-ob35.
    const classes = (dom as MockHost).className.split(/\s+/);
    assert.ok(
      classes.includes("remargin-widget-host"),
      `expected host className to include remargin-widget-host, got: "${(dom as MockHost).className}"`
    );
    assert.ok(
      classes.includes("remargin-container"),
      `expected host className to include remargin-container, got: "${(dom as MockHost).className}"`
    );
    assert.equal((dom as MockHost).dataset.remarginId, "c1");

    widget.destroy(dom);
    assert.equal(unmountCalls, 1, "unmount must run once on destroy");
  });

  // AC: Click on the host fires plugin.focusComment(id, sourcePath).
  it("test #12: widget onClick prop forwards to plugin.focusComment", () => {
    const plugin = makePlugin(true);

    // Wrapper is `WidgetProviders` (added by ticket rem-ob35); the inner
    // child is `WidgetCommentView`, which carries the `onClick` prop.
    let captured: ((id: string, file: string) => void) | undefined;
    __setCreateRootForTests(((_el: unknown) => ({
      render: (element: unknown) => {
        const wrapper = element as {
          props?: { children?: { props?: { onClick?: (id: string, file: string) => void } } };
        };
        const inner = wrapper.props?.children;
        if (typeof inner?.props?.onClick === "function") captured = inner.props.onClick;
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

  // AC (rem-ob35): toDOM must wrap WidgetCommentView in WidgetProviders,
  // passing `plugin: this.plugin` and `portalContainer: host`. Without
  // the wrapper, the React mount throws on first render with
  // "useBackend must be used within a BackendContext.Provider". The
  // wrapper presence is the structural fix this test guards.
  it("test #12a: toDOM wraps WidgetCommentView in WidgetProviders with plugin + host as portal container", () => {
    const plugin = makePlugin(true);

    let capturedElement: unknown;
    let capturedHost: unknown;
    __setCreateRootForTests(((host: unknown) => {
      capturedHost = host;
      return {
        render: (element: unknown) => {
          capturedElement = element;
        },
        unmount: () => {
          /* test-only no-op */
        },
      };
    }) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    const widget = new RemarginWidget(block(), plugin as unknown as RemarginPlugin, "notes/x.md");
    const dom = widget.toDOM();

    const wrapper = capturedElement as {
      type: unknown;
      props: {
        plugin: unknown;
        portalContainer: unknown;
        children: { type: unknown };
      };
    };
    assert.equal(wrapper.type, WidgetProviders, "outer element must be WidgetProviders");
    assert.equal(wrapper.props.plugin, plugin, "plugin prop must be the plugin instance");
    assert.equal(
      wrapper.props.portalContainer,
      capturedHost,
      "portalContainer prop must be the same host the React root mounts into"
    );
    assert.equal(
      wrapper.props.portalContainer,
      dom,
      "portalContainer prop must be the host element toDOM returns"
    );
    assert.equal(
      wrapper.props.children.type,
      WidgetCommentView,
      "wrapper child must be the WidgetCommentView element"
    );
  });

  // AC: Toggling collapse forces the next build() to produce a widget
  // that eq-differs from the previous one for that id.
  it("test #13: collapse toggle makes the next-built widget !eq the previous", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: true });
    const before = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    plugin.collapseState.toggle("c1");
    const after = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );

    function pickWidget(set: ReturnType<typeof buildDecorations>): WidgetType {
      let found: WidgetType | null = null;
      set.between(0, state.doc.toString().length, (_f, _t, value) => {
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

describe("commentWidgetPlugin shape", () => {
  // AC: commentWidgetPlugin returns a StateField extension (NOT a
  // ViewPlugin). The strongest check is `instanceof StateField` —
  // CM6 enforces this at runtime, and a ViewPlugin would never
  // satisfy it (ViewPlugin lives in @codemirror/view and has its
  // own class). We also assert the `extension` getter is present
  // (the public surface used to plug the field into EditorState).
  //
  // Why this matters: block decorations are forbidden from
  // ViewPlugin instances by CM6 (RangeError: "Block decorations
  // may not be specified via plugins" — see ticket rem-3dra). If
  // this test ever fails, the runtime crash returns.
  it("test #14: commentWidgetPlugin returns a CM6 StateField (NOT a ViewPlugin)", () => {
    const plugin = makePlugin(true);
    const field = commentWidgetPlugin(plugin as unknown as RemarginPlugin);
    assert.ok(
      field instanceof StateField,
      "commentWidgetPlugin must return a StateField — block decorations cannot come from a ViewPlugin"
    );
    const candidate = field as unknown as { extension: unknown };
    assert.notEqual(candidate.extension, undefined, "StateField must expose an `extension`");
  });
});

describe("commentWidget defaults", () => {
  // AC: settings.editorWidgets default remains false.
  it("DEFAULT_SETTINGS.editorWidgets is false", () => {
    assert.equal(DEFAULT_SETTINGS.editorWidgets, false);
  });
});
