/**
 * Tests for the Live Preview comment widget: decoration building, the state field's update
 * path, the widget itself and the collapse-effect bridge.
 */

import { strict as assert } from "node:assert";
import { afterEach, beforeEach, describe, it } from "node:test";
import { type EditorState, StateEffect, StateField } from "@codemirror/state";
import type { WidgetType } from "@codemirror/view";
import { editorInfoField, editorLivePreviewField } from "obsidian";
import { WidgetCommentThread } from "../components/widget/WidgetCommentThread.tsx";
import { WidgetProviders } from "../components/widget/WidgetProviders.tsx";
import type { Comment } from "../generated/types.ts";
import type { ThreadNode } from "../lib/threadTree.ts";
import type RemarginPlugin from "../main.ts";
import { type ParsedBlock, parseRemarginBlocks } from "../parser/parseRemarginBlocks.ts";
import { CollapseState } from "../state/collapseState.ts";
import { DEFAULT_SETTINGS } from "../types.ts";
import {
  __setCreateRootForTests,
  buildDecorations,
  collapseEffect,
  collapseEffectBridge,
  commentWidgetPlugin,
  RemarginWidget,
} from "./commentWidget.ts";

/**
 * The minimal `EditorState` shape production code touches: `state.doc.toString()` and
 * `state.field(field, false)`. A real `EditorView` needs a DOM, which `node --test` does not
 * provide; `field()` is a per-test record keyed on the sentinel fields of the `obsidian` stub.
 */
interface MockEditorState {
  doc: { toString(): string };
  field<T>(field: unknown, required: false): T | undefined;
}

/** Inputs to `makeState`. */
interface MakeStateOpts {
  doc: string;
  /** `undefined` simulates the live-preview field being absent. */
  livePreview: boolean | undefined;
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

/** The slice of the plugin the widget code touches, recording focus calls. */
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
  "type: human",
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
  "type: human",
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

/** A stand-in for the widget's host element. */
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
  it("test #1: setting off → Decoration.none", () => {
    const plugin = makePlugin(false);
    const state = makeState({ doc: VALID_BLOCK, livePreview: true });
    assertNoDecorations(state, plugin);
  });

  it("test #2: source mode (livePreview field === false) → Decoration.none", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: false });
    assertNoDecorations(state, plugin);
  });

  // `isLivePreviewState` is module-private, so it is exercised through `buildDecorations`.
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
    assert.equal(spec.inclusive, true, "inclusive: true");
    const parsed = parseRemarginBlocks(VALID_BLOCK)[0];
    assert.equal(collected[0].from, parsed.startOffset);
    assert.equal(collected[0].to, parsed.endOffset);
  });

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

  // Without the hidden decoration the reply's raw YAML would render below the parent's widget.
  it("test #4b: reply block whose parent is in the doc emits a hidden EmptyWidget decoration", () => {
    const plugin = makePlugin(true);
    const REPLY_TO_C1 = [
      "```remargin",
      "---",
      "id: c2",
      "author: bob",
      "type: human",
      "ts: 2026-04-25T12:01:00-04:00",
      "reply-to: c1",
      "---",
      "second comment",
      "```",
    ].join("\n");
    const doc = `${VALID_BLOCK}\n${REPLY_TO_C1}`;
    const state = makeState({ doc, livePreview: true });
    const decorations = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 2, "two decorations: parent RemarginWidget + reply EmptyWidget");

    const collected: Array<{ kind: "root" | "hidden"; id?: string }> = [];
    decorations.between(0, doc.length, (_from, _to, value) => {
      const widget = (value as { spec: { widget: WidgetType } }).spec.widget;
      const maybeNode = widget as unknown as { threadNode?: ThreadNode };
      if (maybeNode.threadNode) {
        collected.push({ kind: "root", id: maybeNode.threadNode.comment.id });
      } else {
        collected.push({ kind: "hidden" });
      }
    });
    const roots = collected.filter((c) => c.kind === "root");
    const hidden = collected.filter((c) => c.kind === "hidden");
    assert.equal(roots.length, 1, "exactly one root widget");
    assert.equal(roots[0].id, "c1", "the root widget belongs to the parent (c1)");
    assert.equal(hidden.length, 1, "exactly one hidden suppression decoration for the reply");
  });

  it("test #4c: orphan reply (parent absent) emits a decoration as a degraded root", () => {
    const plugin = makePlugin(true);
    const ORPHAN_REPLY = [
      "```remargin",
      "---",
      "id: c2",
      "author: bob",
      "type: human",
      "ts: 2026-04-25T12:01:00-04:00",
      "reply-to: missing-parent",
      "---",
      "orphan reply",
      "```",
    ].join("\n");
    const state = makeState({ doc: ORPHAN_REPLY, livePreview: true });
    const decorations = buildDecorations(
      state as unknown as EditorState,
      plugin as unknown as RemarginPlugin
    );
    assert.equal(decorations.size, 1, "orphan reply still renders as a degraded root");
  });
});

describe("commentWidgetPlugin StateField update lifecycle", () => {
  // CM6 keeps a StateField's create/update callbacks on private slots; this helper reads them.
  function specFromField(field: unknown): {
    create: (state: EditorState) => unknown;
    update: (value: unknown, tr: unknown) => unknown;
  } {
    const f = field as { createF?: unknown; updateF?: unknown };
    return {
      create: f.createF as (state: EditorState) => unknown,
      update: f.updateF as (value: unknown, tr: unknown) => unknown,
    };
  }

  // Viewport and selection updates remap the existing decorations; only a doc change rebuilds.
  it("test #5: docChanged update → buildDecorations called (re-parse)", () => {
    const plugin = makePlugin(true);
    const state = makeState({ doc: VALID_BLOCK, livePreview: true });
    const field = commentWidgetPlugin(plugin as unknown as RemarginPlugin);
    const spec = specFromField(field);

    const initial = spec.create(state as unknown as EditorState) as {
      size: number;
    };
    assert.equal(initial.size, 1, "initial create produces 1 decoration");

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

    // A sentinel whose only surface is `.map(changes)`: the remap branch calls it and returns its
    // output verbatim, while a rebuild would ignore it.
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
      // Unused on the non-docChanged branch, so any access throws.
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
      // A real `Transaction.effects` is always a readonly array, and the update iterates it.
      effects: [] as unknown[],
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

  function node(text = VALID_BLOCK): ThreadNode {
    return { comment: block(text).comment as Comment, replies: [] };
  }

  it("test #7: eq() returns true for same id + same collapsed + same content", () => {
    const plugin = makePlugin(true);
    const a = new RemarginWidget(node(), plugin as unknown as RemarginPlugin, "f.md");
    const b = new RemarginWidget(node(), plugin as unknown as RemarginPlugin, "f.md");
    assert.equal(a.eq(b), true);
  });

  it("test #8: eq() returns false when collapsed state differs", () => {
    const pluginA = makePlugin(true);
    const pluginB = makePlugin(true);
    pluginB.collapseState.toggle("c1");
    const a = new RemarginWidget(node(), pluginA as unknown as RemarginPlugin, "f.md");
    const b = new RemarginWidget(node(), pluginB as unknown as RemarginPlugin, "f.md");
    assert.equal(a.eq(b), false);
  });

  it("test #9: eq() returns false when content differs for same id", () => {
    const plugin = makePlugin(true);
    const original = node();
    const edited: ThreadNode = {
      comment: { ...original.comment, content: `${original.comment.content}\nedited line` },
      replies: [],
    };
    const a = new RemarginWidget(original, plugin as unknown as RemarginPlugin, "f.md");
    const b = new RemarginWidget(edited, plugin as unknown as RemarginPlugin, "f.md");
    assert.equal(a.eq(b), false);
  });

  it("test #10: ignoreEvent() returns true (does not eat keystrokes)", () => {
    const plugin = makePlugin(true);
    const widget = new RemarginWidget(node(), plugin as unknown as RemarginPlugin, "f.md");
    assert.equal(widget.ignoreEvent(), true);
  });

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

    const widget = new RemarginWidget(node(), plugin as unknown as RemarginPlugin, "f.md");
    const dom = widget.toDOM();
    assert.equal(createRootCalls, 1);
    assert.equal(renderCalls, 1, "render must run once on mount");
    // `remargin-container` is what scopes the Tailwind utilities inside the widget.
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

  it("test #12: widget onClick prop forwards to plugin.focusComment", () => {
    const plugin = makePlugin(true);

    let captured: ((id: string, file: string) => void) | undefined;
    __setCreateRootForTests(((_el: unknown) => ({
      render: (element: unknown) => {
        const wrapper = element as {
          props?: {
            children?: {
              props?: { onClick?: (id: string, file: string) => void };
            };
          };
        };
        const child = wrapper.props?.children;
        if (typeof child?.props?.onClick === "function") {
          captured = child.props.onClick;
        }
      },
      unmount: () => {
        /* test-only no-op */
      },
    })) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    const widget = new RemarginWidget(node(), plugin as unknown as RemarginPlugin, "notes/x.md");
    widget.toDOM();
    assert.ok(captured, "expected an onClick prop on the rendered widget thread");
    captured("c1", "notes/x.md");
    assert.deepStrictEqual(plugin.__focusCalls, [["c1", "notes/x.md"]]);
  });

  // Without the wrapper the mount throws: "useBackend must be used within a BackendContext.Provider".
  it("test #12a: toDOM wraps the rendered tree in WidgetProviders with plugin + host as portal container", () => {
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

    const widget = new RemarginWidget(node(), plugin as unknown as RemarginPlugin, "notes/x.md");
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
      WidgetCommentThread,
      "wrapper child must be WidgetCommentThread (no intermediate block <div>)"
    );
  });

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
  // CM6 forbids block decorations from a ViewPlugin ("Block decorations may not be specified via
  // plugins"), so the extension must be a StateField.
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

describe("collapseEffectBridge", () => {
  function makeStubView(): {
    dispatch: (...args: unknown[]) => void;
    __dispatched: unknown[];
  } {
    const dispatched: unknown[] = [];
    return {
      dispatch: (...args: unknown[]) => {
        dispatched.push(args[0]);
      },
      __dispatched: dispatched,
    };
  }

  // `viewPlugin.create(view)` is the call CM6 itself makes to instantiate the wrapped class.
  function instantiateBridge(plugin: unknown, view: unknown): { destroy: () => void } {
    const vp = collapseEffectBridge(plugin as RemarginPlugin);
    return (vp as unknown as { create: (view: unknown) => { destroy: () => void } }).create(view);
  }

  it("test #15: toggle dispatches a collapseEffect carrying the toggled id", () => {
    const plugin = makePlugin(true);
    const view = makeStubView();

    instantiateBridge(plugin, view);

    plugin.collapseState.toggle("c-toggled");

    assert.equal(view.__dispatched.length, 1, "dispatch must be called exactly once per toggle");
    const tr = view.__dispatched[0] as { effects?: unknown };
    const effects = (Array.isArray(tr.effects) ? tr.effects : [tr.effects]) as Array<{
      is: (t: unknown) => boolean;
      value: { id: string };
    }>;
    assert.equal(effects.length, 1, "transaction must carry exactly one effect");
    assert.equal(effects[0].is(collapseEffect), true, "effect must be a collapseEffect");
    assert.equal(effects[0].value.id, "c-toggled", "effect must carry the toggled id");
  });

  it("test #17: destroy() unsubscribes; subsequent toggles do NOT dispatch", () => {
    const plugin = makePlugin(true);
    const view = makeStubView();

    const instance = instantiateBridge(plugin, view);

    plugin.collapseState.toggle("a");
    assert.equal(view.__dispatched.length, 1, "first toggle dispatches");

    instance.destroy();

    plugin.collapseState.toggle("b");
    assert.equal(view.__dispatched.length, 1, "post-destroy toggle must NOT dispatch");
  });
});

describe("commentWidgetPlugin StateField rebuild on collapseEffect", () => {
  function specFromField(field: unknown): {
    create: (state: EditorState) => unknown;
    update: (value: unknown, tr: unknown) => unknown;
  } {
    const f = field as { createF?: unknown; updateF?: unknown };
    return {
      create: f.createF as (state: EditorState) => unknown,
      update: f.updateF as (value: unknown, tr: unknown) => unknown,
    };
  }

  it("test #16: docChanged=false + collapseEffect → rebuild via buildDecorations", () => {
    const plugin = makePlugin(true);
    const field = commentWidgetPlugin(plugin as unknown as RemarginPlugin);
    const spec = specFromField(field);

    const initialState = makeState({ doc: VALID_BLOCK, livePreview: true });
    const initial = spec.create(initialState as unknown as EditorState) as { size: number };
    assert.equal(initial.size, 1, "initial create produces 1 decoration");

    // The state carries two valid blocks: a true rebuild shows as size 2, a remap leaves it at 1.
    const nextState = makeState({
      doc: `${VALID_BLOCK}\n${VALID_BLOCK_2}`,
      livePreview: true,
    });
    const tr = {
      docChanged: false,
      state: nextState as unknown as EditorState,
      changes: { mapPos: (pos: number) => pos },
      effects: [collapseEffect.of({ id: "c1" })],
    };
    const next = spec.update(initial, tr) as { size: number };
    assert.equal(
      next.size,
      2,
      "collapseEffect must trigger a full rebuild (size must reflect the new state's blocks)"
    );
  });

  // A non-collapse effect on a non-docChanged transaction must stay on the remap-only path.
  it("test #16b: docChanged=false + unrelated effect → still remap-only", () => {
    const plugin = makePlugin(true);
    const field = commentWidgetPlugin(plugin as unknown as RemarginPlugin);
    const spec = specFromField(field);

    let mapCalls = 0;
    const remapSentinel = Symbol("remapped-decorations");
    const previous = {
      size: 1,
      map: (_changes: unknown) => {
        mapCalls += 1;
        return remapSentinel;
      },
    };

    const unrelatedEffect = StateEffect.define<number>();

    const tr = {
      docChanged: false,
      state: new Proxy(
        {},
        {
          get(_t, prop) {
            throw new Error(
              `tr.state should not be read on remap-only branch (got: ${String(prop)})`
            );
          },
        }
      ) as unknown as EditorState,
      changes: { __sentinel: "changes" },
      effects: [unrelatedEffect.of(42)],
    };
    const next = spec.update(previous, tr);
    assert.equal(mapCalls, 1, "unrelated effect must take the remap path");
    assert.equal(next, remapSentinel, "remap path returns .map() result verbatim");
  });
});

describe("commentWidget defaults", () => {
  it("DEFAULT_SETTINGS.editorWidgets is false", () => {
    assert.equal(DEFAULT_SETTINGS.editorWidgets, false);
  });
});
