/** Tests for the reading-mode post-processor and the render child it mounts per comment block. */

import { strict as assert } from "node:assert";
import { afterEach, beforeEach, describe, it } from "node:test";
import { type MarkdownPostProcessorContext, MarkdownRenderChild, TFile } from "obsidian";
import { WidgetCommentThread } from "../components/widget/WidgetCommentThread.tsx";
import { WidgetProviders } from "../components/widget/WidgetProviders.tsx";
import type RemarginPlugin from "../main.ts";
import { CollapseState } from "../state/collapseState.ts";
import { DEFAULT_SETTINGS } from "../types.ts";
import {
  __setCreateRootForTests,
  __setDeferCollapseForTests,
  parseFromInnerContent,
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

/** A stand-in for the widget's host element. */
interface MockHost {
  className: string;
  dataset: Record<string, string>;
  /** Mutable: the post-processor hides the host through it and `render()` un-hides it. */
  style: Record<string, string>;
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
    style: {},
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

/** Mock post-processor context that records the children added to it. */
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

/** The slice of the plugin the reading-mode code touches, with an in-memory vault. */
interface MockPlugin {
  settings: { editorWidgets: boolean };
  collapseState: CollapseState;
  focusComment: (id: string, file: string) => void;
  __focusCalls: Array<[string, string]>;
  app: {
    vault: {
      getAbstractFileByPath: (path: string) => unknown;
      cachedRead: (file: unknown) => Promise<string>;
      on: (name: string, cb: unknown) => unknown;
      offref: (ref: unknown) => void;
    };
  };
  __vaultFiles: Map<string, string>;
}

/**
 * Build a `MockPlugin` whose `app.vault` mimics the surface
 * `ReadingModeCommentChild.loadTree` consults: `getAbstractFileByPath`
 * returns a `TFile`-shaped object iff the path is registered via
 * `__vaultFiles`, and `cachedRead` returns its registered contents.
 * Tests that don't care about cross-block tree resolution can leave
 * `__vaultFiles` empty — `loadTree` short-circuits when the path is
 * not registered, falling back to the leaf-only first paint.
 */
function makePlugin(editorWidgets: boolean): MockPlugin {
  const focusCalls: Array<[string, string]> = [];
  const vaultFiles = new Map<string, string>();
  const plugin: MockPlugin = {
    settings: { ...DEFAULT_SETTINGS, editorWidgets },
    collapseState: new CollapseState(),
    focusComment(id, file) {
      focusCalls.push([id, file]);
    },
    __focusCalls: focusCalls,
    __vaultFiles: vaultFiles,
    app: {
      vault: {
        getAbstractFileByPath: (path: string) => {
          if (!vaultFiles.has(path)) return null;
          return Object.assign(new TFile(), { path });
        },
        cachedRead: async (file: unknown) => {
          const path = (file as { path?: string }).path ?? "";
          return vaultFiles.get(path) ?? "";
        },
        on: () => ({}),
        offref: () => undefined,
      },
    },
  };
  return plugin;
}

// As `code.textContent` returns it in reading mode: the renderer has stripped the outer fences.
const VALID_BLOCK = [
  "---",
  "id: c1",
  "author: alice",
  "type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "hello widget",
].join("\n");

const VALID_BLOCK_2 = [
  "---",
  "id: c2",
  "author: bob",
  "type: human",
  "ts: 2026-04-25T12:01:00-04:00",
  "---",
  "second comment",
].join("\n");

const INVALID_BLOCK_NO_ID = [
  "---",
  "author: alice",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "no id here",
].join("\n");

// Wrapped in the synthesized fence this yields TWO complete blocks, which exercises the
// `parsed.length !== 1` guard. A malformed body, not expected in real usage.
const TWO_BLOCKS_IN_ONE_FENCE = [
  "---",
  "id: c1",
  "author: alice",
  "type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "hello widget",
  "```",
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

// `document` is replaced so the post-processor's `createElement("div")` returns a mock.
let originalDocument: typeof globalThis.document | undefined;
const createdHosts: MockHost[] = [];

beforeEach(() => {
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

describe("parseFromInnerContent", () => {
  it("test #2-helper: bare YAML+content (no fences) → exactly one valid block", () => {
    const inner = [
      "---",
      "id: abc",
      "author: x",
      "type: human",
      "ts: 2026-01-01T00:00:00Z",
      "---",
      "body",
    ].join("\n");

    const parsed = parseFromInnerContent(inner);

    assert.equal(parsed.length, 1, "helper must return exactly one block");
    assert.equal(parsed[0].valid, true, "block must be valid");
    assert.equal(parsed[0].comment.id, "abc", "block id must round-trip from YAML");
  });
});

describe("remarginPostProcessor", () => {
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
    // `remargin-container` is what scopes the Tailwind utilities inside the widget.
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

  // A zero-height (display:none) section stalls Obsidian's incremental reading-mode renderer.
  it("test #5b: valid block → host pre-hidden via visibility, not display:none", () => {
    const plugin = makePlugin(true);
    const code = makeCode(VALID_BLOCK);
    const el = makeEl([code]);
    const ctx = makeCtx();
    const processor = remarginPostProcessor(plugin as unknown as RemarginPlugin);

    processor(el, ctx);

    const host = createdHosts[0];
    assert.equal(host.style.visibility, "hidden", "host must be pre-hidden via visibility");
    assert.notEqual(host.style.display, "none", "host must NOT be collapsed via display:none");
  });

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
  it("test #6: onload mounts a root and subscribes; onunload unsubscribes and unmounts", async () => {
    const plugin = makePlugin(true);

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

    const parsed = parseFromInnerContent(VALID_BLOCK)[0];
    assert.ok(parsed?.valid, "test fixture must be a valid block");

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

  it("test #7: collapseState toggle re-renders only the matching id", async () => {
    const plugin = makePlugin(true);
    const parsedA = parseFromInnerContent(VALID_BLOCK)[0];
    const parsedB = parseFromInnerContent(VALID_BLOCK_2)[0];

    // `__setCreateRootForTests` is one global, so the factory hands out a tracked root per call.
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

  it("test #8: widget onClick prop forwards to plugin.focusComment(id, sourcePath)", async () => {
    const plugin = makePlugin(true);
    const parsed = parseFromInnerContent(VALID_BLOCK)[0];

    let capturedOnClick: ((id: string, file: string) => void) | undefined;
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
          capturedOnClick = child.props.onClick;
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
      // The first render waits on `loadTree`, so microtasks are drained before asserting.
      await new Promise((r) => setTimeout(r, 0));

      assert.ok(capturedOnClick, "expected the rendered widget to receive an onClick prop");
      capturedOnClick("c1", "notes/test.md");
      assert.deepStrictEqual(plugin.__focusCalls, [["c1", "notes/test.md"]]);

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // Without the wrapper the mount throws: "useBackend must be used within a BackendContext.Provider".
  it("test #9: render wraps the thread block in WidgetProviders with plugin + host", async () => {
    const plugin = makePlugin(true);
    const parsed = parseFromInnerContent(VALID_BLOCK)[0];

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
      await new Promise((r) => setTimeout(r, 0));

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
        WidgetCommentThread,
        "wrapper child must be WidgetCommentThread (no intermediate block <div>)"
      );

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  it("test #10: parent block renders nested reply once cachedRead resolves", async () => {
    const plugin = makePlugin(true);
    plugin.__vaultFiles.set(
      "notes/thread.md",
      [
        "```remargin",
        "---",
        "id: c1",
        "author: alice",
        "type: human",
        "ts: 2026-04-25T12:00:00-04:00",
        "---",
        "hello widget",
        "```",
        "",
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
        "",
      ].join("\n")
    );

    const captured: Array<{ id: string; replyIds: string[] }> = [];
    __setCreateRootForTests(((_el: unknown) => ({
      render: (element: unknown) => {
        const wrapper = element as {
          props?: {
            children?: {
              props?: { root?: { comment: { id: string }; replies: Array<unknown> } };
            };
          };
        };
        const root = wrapper.props?.children?.props?.root;
        if (root) {
          captured.push({
            id: root.comment.id,
            replyIds: root.replies.map((r) => (r as { comment: { id: string } }).comment.id),
          });
        }
      },
      unmount: () => {
        /* test-only no-op */
      },
    })) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    try {
      const parsed = parseFromInnerContent(VALID_BLOCK)[0];
      const child = new ReadingModeCommentChild(
        makeHost() as unknown as HTMLElement,
        parsed,
        "notes/thread.md",
        plugin as unknown as RemarginPlugin
      );
      child.onload();

      await new Promise((r) => setTimeout(r, 0));

      // The first render is the leaf-only first paint; the second has the replies populated.
      const last = captured[captured.length - 1];
      assert.ok(last, "expected at least one render after cachedRead resolved");
      assert.equal(last.id, "c1", "post-resolve render keeps the parent root");
      assert.deepStrictEqual(last.replyIds, ["c2"], "reply nests under parent post-resolve");

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // The parent's host owns the reply, and the reply's own host is hidden so it reserves no space.
  it("test #11: reply block whose parent is in the doc is suppressed (host hidden, root unmounted)", async () => {
    const plugin = makePlugin(true);
    plugin.__vaultFiles.set(
      "notes/thread.md",
      [
        "```remargin",
        "---",
        "id: c1",
        "author: alice",
        "type: human",
        "ts: 2026-04-25T12:00:00-04:00",
        "---",
        "hello widget",
        "```",
        "",
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
        "",
      ].join("\n")
    );

    let unmountCalls = 0;
    __setCreateRootForTests(((_el: unknown) => ({
      render: () => {
        /* test-only no-op */
      },
      unmount: () => {
        unmountCalls += 1;
      },
    })) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    __setDeferCollapseForTests((cb) => cb());

    try {
      const c2Inner = [
        "---",
        "id: c2",
        "author: bob",
        "type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply-to: c1",
        "---",
        "second comment",
      ].join("\n");
      const parsed = parseFromInnerContent(c2Inner)[0];
      assert.ok(parsed?.valid, "fixture must be a valid block");

      const host = makeHost();
      const child = new ReadingModeCommentChild(
        host as unknown as HTMLElement,
        parsed,
        "notes/thread.md",
        plugin as unknown as RemarginPlugin
      );
      child.onload();

      await new Promise((r) => setTimeout(r, 0));

      assert.equal(
        (host as unknown as { style: { display?: string } }).style.display,
        "none",
        "reply host must be hidden once cross-block parent is detected"
      );
      assert.equal(unmountCalls, 1, "reply host's React root must be unmounted");

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
      __setDeferCollapseForTests(null);
    }
  });

  it("test #12: orphan reply (parent missing from doc) renders as a leaf root", async () => {
    const plugin = makePlugin(true);
    plugin.__vaultFiles.set(
      "notes/orphan.md",
      [
        "```remargin",
        "---",
        "id: c2",
        "author: bob",
        "type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply-to: c1",
        "---",
        "orphan reply",
        "```",
        "",
      ].join("\n")
    );

    const captured: Array<{ id: string; replyIds: string[] }> = [];
    __setCreateRootForTests(((_el: unknown) => ({
      render: (element: unknown) => {
        const wrapper = element as {
          props?: {
            children?: {
              props?: { root?: { comment: { id: string }; replies: Array<unknown> } };
            };
          };
        };
        const root = wrapper.props?.children?.props?.root;
        if (root) {
          captured.push({
            id: root.comment.id,
            replyIds: root.replies.map((r) => (r as { comment: { id: string } }).comment.id),
          });
        }
      },
      unmount: () => {
        /* test-only no-op */
      },
    })) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    try {
      const c2Inner = [
        "---",
        "id: c2",
        "author: bob",
        "type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply-to: c1",
        "---",
        "orphan reply",
      ].join("\n");
      const parsed = parseFromInnerContent(c2Inner)[0];
      const child = new ReadingModeCommentChild(
        makeHost() as unknown as HTMLElement,
        parsed,
        "notes/orphan.md",
        plugin as unknown as RemarginPlugin
      );
      child.onload();
      await new Promise((r) => setTimeout(r, 0));

      const last = captured[captured.length - 1];
      assert.ok(last, "orphan reply must still render");
      assert.equal(last.id, "c2", "orphan reply is its own root");
      assert.deepStrictEqual(last.replyIds, [], "orphan reply has no nested children");

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // Collapsing to display:none mid-render stalls Obsidian's incremental renderer.
  it("test #13: suppressed reply defers its collapse past the render pass", async () => {
    const plugin = makePlugin(true);
    plugin.__vaultFiles.set(
      "notes/thread.md",
      [
        "```remargin",
        "---",
        "id: c1",
        "author: alice",
        "type: human",
        "ts: 2026-04-25T12:00:00-04:00",
        "---",
        "hello widget",
        "```",
        "",
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
        "",
      ].join("\n")
    );

    // The deferred collapse is captured, not run, to compare host state during and after the pass.
    let deferred: (() => void) | null = null;
    __setDeferCollapseForTests((cb) => {
      deferred = cb;
    });

    let unmountCalls = 0;
    __setCreateRootForTests(((_el: unknown) => ({
      render: () => {
        /* test-only no-op */
      },
      unmount: () => {
        unmountCalls += 1;
      },
    })) as unknown as Parameters<typeof __setCreateRootForTests>[0]);

    try {
      const c2Inner = [
        "---",
        "id: c2",
        "author: bob",
        "type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply-to: c1",
        "---",
        "second comment",
      ].join("\n");
      const parsed = parseFromInnerContent(c2Inner)[0];
      const host = makeHost();
      const child = new ReadingModeCommentChild(
        host as unknown as HTMLElement,
        parsed,
        "notes/thread.md",
        plugin as unknown as RemarginPlugin
      );
      child.onload();
      await new Promise((r) => setTimeout(r, 0));

      assert.ok(deferred, "a collapse must be scheduled");
      assert.notEqual(host.style.display, "none", "must NOT collapse mid-render");
      assert.equal(unmountCalls, 0, "root must not be unmounted before the deferred pass");

      (deferred as unknown as () => void)();
      assert.equal(host.style.display, "none", "collapses after the render pass");
      assert.equal(unmountCalls, 1, "root unmounted after the deferred pass");

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
      __setDeferCollapseForTests(null);
    }
  });
});
