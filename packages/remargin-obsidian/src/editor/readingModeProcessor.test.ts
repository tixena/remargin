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

/**
 * A well-formed remargin block as it appears inside `<pre><code>` in
 * reading mode — i.e. AFTER markdown rendering has stripped the outer
 * `` ``` `` fences. This is exactly the shape `code.textContent` returns
 * in production. The post-processor delegates to `parseFromInnerContent`,
 * which re-wraps before parsing.
 */
const VALID_BLOCK = [
  "---",
  "id: c1",
  "author: alice",
  "author_type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "hello widget",
].join("\n");

const VALID_BLOCK_2 = [
  "---",
  "id: c2",
  "author: bob",
  "author_type: human",
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

/**
 * A fixture whose synthesized-fence wrap (see `parseFromInnerContent`)
 * yields TWO complete blocks, exercising the post-processor's
 * `parsed.length !== 1` guard. The shape: bare YAML+content for block
 * one, then a literal closing fence `` ``` `` (which the wrapper's outer
 * `` ```remargin `` opener will close on), then a second `` ```remargin ``
 * fence introducing block two. The wrapper appends its own closing
 * fence after this body, but block two has already self-closed before
 * that.
 *
 * In practice this is a malformed `<pre><code>` body and isn't expected
 * in real usage; the test is here to lock in the guard's behaviour.
 */
const TWO_BLOCKS_IN_ONE_FENCE = [
  "---",
  "id: c1",
  "author: alice",
  "author_type: human",
  "ts: 2026-04-25T12:00:00-04:00",
  "---",
  "hello widget",
  "```",
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

describe("parseFromInnerContent", () => {
  // AC (rem-hghw): the helper accepts the bare YAML+content shape that
  // `<pre><code class="language-remargin">…</code></pre>` exposes via
  // `code.textContent` (markdown rendering strips the outer fences) and
  // returns exactly one valid parsed block. This is the regression that
  // kept reading-mode pretty widgets from rendering.
  it("test #2-helper: bare YAML+content (no fences) → exactly one valid block", () => {
    const inner = [
      "---",
      "id: abc",
      "author: x",
      "author_type: human",
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

    // Build the parsed block by running the helper that re-wraps
    // bare YAML+content with synthesized fences (matching the production
    // call site in `remarginPostProcessor`). If the parser's output
    // shape changes incompatibly, this test should break loudly rather
    // than silently miscoerce.
    const parsed = parseFromInnerContent(VALID_BLOCK)[0];
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
    // VALID_BLOCK / VALID_BLOCK_2 are bare YAML+content (matching the
    // shape Obsidian's renderer hands the post-processor); use the
    // helper so they pass through the same fence-synthesis path as
    // production. See note on test #6.
    const parsedA = parseFromInnerContent(VALID_BLOCK)[0];
    const parsedB = parseFromInnerContent(VALID_BLOCK_2)[0];

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
    const parsed = parseFromInnerContent(VALID_BLOCK)[0];

    // Capture the React element the child renders so we can fish out
    // the wired `onClick` prop and invoke it directly. The render tree
    // is `WidgetProviders > WidgetCommentThread` — the thread component
    // carries the `onClick` prop directly.
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

      assert.ok(capturedOnClick, "expected the rendered widget to receive an onClick prop");
      capturedOnClick("c1", "notes/test.md");
      assert.deepStrictEqual(plugin.__focusCalls, [["c1", "notes/test.md"]]);

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // AC (rem-ob35): the rendered React element must be a `WidgetProviders`
  // wrapping the thread block (a <div> containing the toolbar + thread),
  // and `WidgetProviders` must receive the plugin + the host element as
  // its portal container. Without the wrapper, mounting crashes with
  // "useBackend must be used within a BackendContext.Provider" — so this
  // assertion guards the runtime fix.
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

  // AC: when the doc contains a reply whose parent is the block being
  // rendered, the parent's host re-renders with the reply nested as
  // `root.replies`. Mirrors the Live Preview cross-block tree behaviour.
  it("test #10: parent block renders nested reply once cachedRead resolves", async () => {
    const plugin = makePlugin(true);
    plugin.__vaultFiles.set(
      "notes/thread.md",
      [
        "```remargin",
        "---",
        "id: c1",
        "author: alice",
        "author_type: human",
        "ts: 2026-04-25T12:00:00-04:00",
        "---",
        "hello widget",
        "```",
        "",
        "```remargin",
        "---",
        "id: c2",
        "author: bob",
        "author_type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply_to: c1",
        "---",
        "second comment",
        "```",
        "",
      ].join("\n")
    );

    // The render call captures the latest WidgetCommentThread root prop
    // so we can assert post-resolution shape (replies populated).
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

      // Drain microtasks so the awaited cachedRead resolves and
      // `loadTree` runs its post-await render.
      await new Promise((r) => setTimeout(r, 0));

      // First render is the leaf-only first paint; the second is the
      // post-resolve render with replies populated.
      const last = captured[captured.length - 1];
      assert.ok(last, "expected at least one render after cachedRead resolved");
      assert.equal(last.id, "c1", "post-resolve render keeps the parent root");
      assert.deepStrictEqual(last.replyIds, ["c2"], "reply nests under parent post-resolve");

      child.onunload();
    } finally {
      __setCreateRootForTests(null);
    }
  });

  // AC: a block whose comment is a reply with parent in the same doc
  // renders NOTHING — the parent's host owns it. The host is hidden so
  // it doesn't reserve vertical space.
  it("test #11: reply block whose parent is in the doc is suppressed (host hidden, root unmounted)", async () => {
    const plugin = makePlugin(true);
    plugin.__vaultFiles.set(
      "notes/thread.md",
      [
        "```remargin",
        "---",
        "id: c1",
        "author: alice",
        "author_type: human",
        "ts: 2026-04-25T12:00:00-04:00",
        "---",
        "hello widget",
        "```",
        "",
        "```remargin",
        "---",
        "id: c2",
        "author: bob",
        "author_type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply_to: c1",
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

    try {
      // Build the parsed block for c2 so the child believes it's
      // rendering the reply chunk.
      const c2Inner = [
        "---",
        "id: c2",
        "author: bob",
        "author_type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply_to: c1",
        "---",
        "second comment",
      ].join("\n");
      const parsed = parseFromInnerContent(c2Inner)[0];
      assert.ok(parsed?.valid, "fixture must be a valid block");

      const host = makeHost();
      // The container's `style` shape is the surface `loadTree` mutates
      // when it suppresses the host. The default mock host doesn't have
      // one — splice it in so the assertion can read it.
      (host as unknown as { style: Record<string, string> }).style = {};
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
    }
  });

  // AC: an orphan reply (parent NOT present in the doc) is promoted to
  // a top-level root by buildThreadTree, so the reading-mode host
  // renders it as a leaf rather than suppressing it.
  it("test #12: orphan reply (parent missing from doc) renders as a leaf root", async () => {
    const plugin = makePlugin(true);
    // Doc contains ONLY the reply — its parent (c1) is missing.
    plugin.__vaultFiles.set(
      "notes/orphan.md",
      [
        "```remargin",
        "---",
        "id: c2",
        "author: bob",
        "author_type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply_to: c1",
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
        "author_type: human",
        "ts: 2026-04-25T12:01:00-04:00",
        "reply_to: c1",
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
});
