import { strict as assert } from "node:assert";
import { afterEach, beforeEach, describe, it } from "node:test";
import { type MarkdownPostProcessorContext, MarkdownRenderChild } from "obsidian";
import { WidgetCommentView } from "../components/widget/WidgetCommentView.tsx";
import { WidgetProviders } from "../components/widget/WidgetProviders.tsx";
import type RemarginPlugin from "../main.ts";
import { CollapseState } from "../state/collapseState.ts";
import { DEFAULT_SETTINGS } from "../types.ts";
import {
  __setCreateRootForTests,
  ReadingModeCommentChild,
  remarginPostProcessor,
} from "./readingModeProcessor.ts";

/**
 * Mock for the `<pre>` element produced by Obsidian's markdown renderer.
 * Tracks whether `replaceWith` was called and what it was called with,
 * which is the structural side-effect tests #2/#5 assert on.
 */
interface MockPreElement {
  replaced: boolean;
  replacement: unknown;
  replaceWith(node: unknown): void;
}

/**
 * Mock for the `<code class="language-remargin">` element. `parentElement`
 * is the matching `<pre>` so the post-processor's `code.parentElement`
 * walk works the same way as in the real DOM.
 */
interface MockCodeElement {
  textContent: string;
  parentElement: MockPreElement;
}

interface MockHost {
  className: string;
  dataset: Record<string, string>;
  // Placeholder so tests that need to dispatch a click can attach a
  // listener after the post-processor mounts the React root.
  __clickHandlers: Array<(event: unknown) => void>;
  addEventListener(event: string, handler: (event: unknown) => void): void;
  click(): void;
}

/**
 * Build a stand-in host element. The post-processor calls
 * `document.createElement("div")` to make this — we override the
 * `document` global for the test so we can inject the mock and capture
 * mutations.
 */
function makeHost(): MockHost {
  const host: MockHost = {
    className: "",
    dataset: {},
    __clickHandlers: [],
    addEventListener(event, handler) {
      if (event === "click") this.__clickHandlers.push(handler);
    },
    click() {
      for (const h of this.__clickHandlers) h({});
    },
  };
  return host;
}

function makePre(): MockPreElement {
  const pre: MockPreElement = {
    replaced: false,
    replacement: null,
    replaceWith(node) {
      this.replaced = true;
      this.replacement = node;
    },
  };
  return pre;
}

function makeCode(textContent: string): MockCodeElement {
  const pre = makePre();
  const code: MockCodeElement = { textContent, parentElement: pre };
  return code;
}

/**
 * Build a fake `el` whose `querySelectorAll` returns the given codes.
 * The post-processor only uses `querySelectorAll` on its `el` argument,
 * so this is the entire surface area we need to mock.
 */
function makeEl(codes: MockCodeElement[]): HTMLElement {
  return {
    querySelectorAll(_selector: string) {
      return codes;
    },
  } as unknown as HTMLElement;
}

interface MockCtx extends MarkdownPostProcessorContext {
  __children: unknown[];
}

function makeCtx(sourcePath = "notes/test.md"): MockCtx {
  const ctx = {
    sourcePath,
    docId: "doc-1",
    frontmatter: undefined,
    __children: [] as unknown[],
    addChild(child: unknown) {
      this.__children.push(child);
    },
    getSectionInfo: () => null,
  };
  return ctx as unknown as MockCtx;
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

/** A well-formed remargin block as it appears inside a `<pre><code>`. */
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
  "no id here",
  "```",
].join("\n");

const TWO_BLOCKS_IN_ONE_FENCE = `${VALID_BLOCK}\n${VALID_BLOCK_2}`;

/**
 * Override the `document` global so the post-processor's
 * `document.createElement("div")` call returns a controllable mock.
 * Restored in `afterEach`.
 */
let originalDocument: typeof globalThis.document | undefined;
const createdHosts: MockHost[] = [];

beforeEach(() => {
  // Stash any pre-existing global document so the harness can be
  // composed with future DOM-providing test runners without surprise.
  originalDocument = (globalThis as { document?: typeof globalThis.document }).document;
  createdHosts.length = 0;
  (globalThis as { document?: unknown }).document = {
    createElement: (_tag: string) => {
      const host = makeHost();
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
});

describe("remarginPostProcessor", () => {
  // AC: When `editorWidgets === false`, the post-processor is a no-op
  // (raw <pre> stays).
  it("test #1: setting off → leaves <pre> untouched and skips addChild", () => {
    const plugin = makePlugin(false);
    const code = makeCode(VALID_BLOCK);
    const el = makeEl([code]);
    const ctx = makeCtx();
    const processor = remarginPostProcessor(plugin as unknown as RemarginPlugin);

    processor(el, ctx);

    assert.equal(code.parentElement.replaced, false, "<pre> must stay in place");
    assert.equal(ctx.__children.length, 0, "ctx.addChild must NOT be called");
  });

  // AC: When `editorWidgets === true` and the block is well-formed, the
  // <pre> is replaced with a host element containing the React tree.
  it("test #2: valid block → <pre> replaced; host has data-remargin-id; addChild fires once", () => {
    const plugin = makePlugin(true);
    const code = makeCode(VALID_BLOCK);
    const el = makeEl([code]);
    const ctx = makeCtx();
    const processor = remarginPostProcessor(plugin as unknown as RemarginPlugin);

    processor(el, ctx);

    assert.equal(code.parentElement.replaced, true, "<pre> must be replaced");
    assert.equal(createdHosts.length, 1, "exactly one host element should be created");
    const host = createdHosts[0];
    // Host className must carry both the structural class AND
    // `remargin-container` so Tailwind utilities scoped via
    // tailwind.config.ts's `important: ".remargin-container"` apply
    // inside the widget. See ticket rem-ob35.
    assert.ok(
      host.className.split(/\s+/).includes("remargin-reading-host"),
      `expected host className to include remargin-reading-host, got: "${host.className}"`
    );
    assert.ok(
      host.className.split(/\s+/).includes("remargin-container"),
      `expected host className to include remargin-container, got: "${host.className}"`
    );
    assert.equal(host.dataset.remarginId, "c1");
    assert.equal(code.parentElement.replacement, host, "host is the replacement node");
    assert.equal(ctx.__children.length, 1, "ctx.addChild fired once");
    assert.ok(
      ctx.__children[0] instanceof MarkdownRenderChild,
      "ctx.addChild received a MarkdownRenderChild"
    );
  });

  // AC: When the block is malformed (parser returns valid: false), skip.
  it("test #3: invalid block (missing id) → <pre> untouched, addChild skipped", () => {
    const plugin = makePlugin(true);
    const code = makeCode(INVALID_BLOCK_NO_ID);
    const el = makeEl([code]);
    const ctx = makeCtx();
    const processor = remarginPostProcessor(plugin as unknown as RemarginPlugin);

    processor(el, ctx);

    assert.equal(code.parentElement.replaced, false, "<pre> must stay in place");
    assert.equal(ctx.__children.length, 0, "ctx.addChild must NOT be called");
  });

  // AC: When the parser returns >1 blocks for the fence (length !== 1),
  // skip. (Two `````remargin` fences nested inside one `<pre>` is the
  // structural shape we guard against here.)
  it("test #4: parser returns multiple blocks → <pre> untouched", () => {
    const plugin = makePlugin(true);
    const code = makeCode(TWO_BLOCKS_IN_ONE_FENCE);
    const el = makeEl([code]);
    const ctx = makeCtx();
    const processor = remarginPostProcessor(plugin as unknown as RemarginPlugin);

    processor(el, ctx);

    assert.equal(code.parentElement.replaced, false, "<pre> must stay in place");
    assert.equal(ctx.__children.length, 0, "ctx.addChild must NOT be called");
  });

  // AC: Multiple separate <pre> elements in the same DOM root are each
  // handled independently.
  it("test #5: two separate <pre> elements → both replaced; addChild fires twice", () => {
    const plugin = makePlugin(true);
    const codeA = makeCode(VALID_BLOCK);
    const codeB = makeCode(VALID_BLOCK_2);
    const el = makeEl([codeA, codeB]);
    const ctx = makeCtx();
    const processor = remarginPostProcessor(plugin as unknown as RemarginPlugin);

    processor(el, ctx);

    assert.equal(codeA.parentElement.replaced, true, "first <pre> replaced");
    assert.equal(codeB.parentElement.replaced, true, "second <pre> replaced");
    assert.equal(createdHosts.length, 2);
    assert.equal(createdHosts[0].dataset.remarginId, "c1");
    assert.equal(createdHosts[1].dataset.remarginId, "c2");
    assert.equal(ctx.__children.length, 2, "ctx.addChild fired twice");
  });
});

describe("ReadingModeCommentChild", () => {
  // AC: onload mounts a React root + subscribes to collapseState; onunload
  // unsubscribes and unmounts.
  it("test #6: onload mounts a root and subscribes; onunload unsubscribes and unmounts", async () => {
    const plugin = makePlugin(true);

    // Track every collapseState subscription so we can verify onload
    // registered one and onunload tore it down.
    let listenerRegistered = false;
    let unsubscribed = false;
    const realSubscribe = plugin.collapseState.subscribe.bind(plugin.collapseState);
    plugin.collapseState.subscribe = (listener) => {
      listenerRegistered = true;
      const real = realSubscribe(listener);
      return () => {
        unsubscribed = true;
        real();
      };
    };

    // Build the parsed block by running the real parser. If the
    // parser's output shape changes incompatibly, this test should
    // break loudly rather than silently miscoerce.
    const parsed = (await import("../parser/parseRemarginBlocks.ts")).parseRemarginBlocks(
      VALID_BLOCK
    )[0];
    assert.ok(parsed?.valid, "test fixture must be a valid block");

    // Inject a fake `createRoot` so we don't need a real DOM. The mock
    // tracks render and unmount calls; that's the only behaviour the
    // AC requires us to verify.
    let renderCalls = 0;
    let unmountCalls = 0;
    const fakeRoot = {
      render: () => {
        renderCalls += 1;
      },
      unmount: () => {
        unmountCalls += 1;
      },
    };
    let createRootCalls = 0;
    __setCreateRootForTests(((_el: unknown) => {
      createRootCalls += 1;
      return fakeRoot;
    }) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    try {
      const host = makeHost() as unknown as HTMLElement;
      const child = new ReadingModeCommentChild(
        host,
        parsed,
        "notes/test.md",
        plugin as unknown as RemarginPlugin
      );

      child.onload();
      assert.equal(createRootCalls, 1, "onload must call createRoot once");
      assert.equal(listenerRegistered, true, "onload must subscribe to collapseState");
      assert.equal(renderCalls, 1, "onload must trigger an initial render");

      child.onunload();
      assert.equal(unsubscribed, true, "onunload must call the unsubscribe thunk");
      assert.equal(unmountCalls, 1, "onunload must unmount the React root");
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // AC: Toggling collapse re-renders only the matching child.
  it("test #7: collapseState toggle re-renders only the matching id", async () => {
    const plugin = makePlugin(true);
    const parseRemarginBlocks = (await import("../parser/parseRemarginBlocks.ts"))
      .parseRemarginBlocks;
    const parsedA = parseRemarginBlocks(VALID_BLOCK)[0];
    const parsedB = parseRemarginBlocks(VALID_BLOCK_2)[0];

    // Spy on every render so we can assert per-child render counts.
    // Each child gets its own fake root — `__setCreateRootForTests`
    // is a single global, so we install a factory that hands out a
    // fresh tracked root per call.
    const renderCounts: number[] = [];
    const unmountCounts: number[] = [];
    let nextIndex = -1;
    __setCreateRootForTests(((_el: unknown) => {
      nextIndex += 1;
      const i = nextIndex;
      renderCounts[i] = 0;
      unmountCounts[i] = 0;
      return {
        render: () => {
          renderCounts[i] += 1;
        },
        unmount: () => {
          unmountCounts[i] += 1;
        },
      };
    }) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    try {
      const childA = new ReadingModeCommentChild(
        makeHost() as unknown as HTMLElement,
        parsedA,
        "notes/a.md",
        plugin as unknown as RemarginPlugin
      );
      const childB = new ReadingModeCommentChild(
        makeHost() as unknown as HTMLElement,
        parsedB,
        "notes/b.md",
        plugin as unknown as RemarginPlugin
      );

      childA.onload();
      childB.onload();
      const initialA = renderCounts[0];
      const initialB = renderCounts[1];

      plugin.collapseState.toggle("c1");
      assert.equal(renderCounts[0], initialA + 1, "child A re-renders on its own id");
      assert.equal(renderCounts[1], initialB, "child B does NOT re-render on A's toggle");

      plugin.collapseState.toggle("c2");
      assert.equal(renderCounts[0], initialA + 1, "child A does NOT re-render on B's toggle");
      assert.equal(renderCounts[1], initialB + 1, "child B re-renders on its own id");

      childA.onunload();
      childB.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // AC: Click on the host fires plugin.focusComment(id, sourcePath).
  it("test #8: widget onClick prop forwards to plugin.focusComment(id, sourcePath)", async () => {
    const plugin = makePlugin(true);
    const parsed = (await import("../parser/parseRemarginBlocks.ts")).parseRemarginBlocks(
      VALID_BLOCK
    )[0];

    // Capture the React element the child renders so we can fish out
    // the wired `onClick` prop and invoke it directly. The wrapper is
    // `WidgetProviders` (added by ticket rem-ob35); the child is the
    // `WidgetCommentView` element where `onClick` lives. Descend one
    // level via `props.children` to reach it.
    let capturedOnClick: ((id: string, file: string) => void) | undefined;
    __setCreateRootForTests(((_el: unknown) => ({
      render: (element: unknown) => {
        const wrapper = element as {
          props?: { children?: { props?: { onClick?: (id: string, file: string) => void } } };
        };
        const inner = wrapper.props?.children;
        if (typeof inner?.props?.onClick === "function") {
          capturedOnClick = inner.props.onClick;
        }
      },
      unmount: () => {
        /* test-only no-op */
      },
    })) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    try {
      const child = new ReadingModeCommentChild(
        makeHost() as unknown as HTMLElement,
        parsed,
        "notes/test.md",
        plugin as unknown as RemarginPlugin
      );
      child.onload();

      assert.ok(capturedOnClick, "expected the rendered widget to receive an onClick prop");
      capturedOnClick("c1", "notes/test.md");
      assert.deepStrictEqual(plugin.__focusCalls, [["c1", "notes/test.md"]]);

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // AC (rem-ob35): the rendered React element must be a `WidgetProviders`
  // wrapping a `WidgetCommentView`, and `WidgetProviders` must receive
  // the plugin + the host element as its portal container. Without the
  // wrapper, mounting crashes with "useBackend must be used within a
  // BackendContext.Provider" — so this assertion guards the runtime fix.
  it("test #9: render wraps WidgetCommentView in WidgetProviders with plugin + host", async () => {
    const plugin = makePlugin(true);
    const parsed = (await import("../parser/parseRemarginBlocks.ts")).parseRemarginBlocks(
      VALID_BLOCK
    )[0];

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

    try {
      const host = makeHost() as unknown as HTMLElement;
      const child = new ReadingModeCommentChild(
        host,
        parsed,
        "notes/test.md",
        plugin as unknown as RemarginPlugin
      );
      child.onload();

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
        wrapper.props.children.type,
        WidgetCommentView,
        "wrapper child must be the WidgetCommentView element"
      );

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });
});
