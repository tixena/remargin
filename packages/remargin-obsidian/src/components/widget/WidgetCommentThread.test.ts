import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Participant, RemarginBackend } from "../../backend/index.ts";
import type { Comment } from "../../generated/types.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { __resetParticipantsCacheForTests } from "../../hooks/useParticipants.ts";
import { PluginContext } from "../../hooks/usePlugin.ts";
import type { ThreadNode } from "../../lib/threadTree.ts";
import type RemarginPlugin from "../../main.ts";
import { CollapseState } from "../../state/collapseState.ts";
import { DEFAULT_SETTINGS } from "../../types.ts";
import { setSubtreeCollapsed, WidgetCommentThread } from "./WidgetCommentThread.tsx";

const pluginStub = { settings: DEFAULT_SETTINGS } as unknown as RemarginPlugin;
const backendStub = {
  registryShow: (): Promise<Participant[]> => Promise.resolve([]),
} as unknown as RemarginBackend;

function comment(overrides: Partial<Comment> = {}): Comment {
  return {
    ack: [],
    attachments: [],
    author: "alice",
    author_type: "human",
    checksum: "",
    content: "hello",
    edited_at: undefined,
    el: undefined,
    id: "root",
    line: 0,
    reactions: {},
    remargin_kind: [],
    reply_to: undefined,
    signature: undefined,
    sl: undefined,
    thread: undefined,
    to: [],
    ts: "2026-04-25T12:00:00-04:00",
    ...overrides,
  };
}

function leaf(id: string, replyTo?: string): ThreadNode {
  return { comment: comment({ id, reply_to: replyTo }), replies: [] };
}

function renderThread(props: {
  root: ThreadNode;
  collapseState: CollapseState;
  isRoot?: boolean;
}): string {
  __resetParticipantsCacheForTests();
  return renderToStaticMarkup(
    createElement(
      PluginContext.Provider,
      { value: pluginStub },
      createElement(
        BackendContext.Provider,
        { value: backendStub },
        createElement(WidgetCommentThread, {
          root: props.root,
          sourcePath: "notes/test.md",
          me: null,
          collapseState: props.collapseState,
          onClick: () => undefined,
          isRoot: props.isRoot,
        })
      )
    )
  );
}

describe("WidgetCommentThread isRoot wiring", () => {
  it("isRoot=true → WidgetRootToolbar appears in the rendered tree", () => {
    const html = renderThread({
      root: leaf("root"),
      collapseState: new CollapseState(),
      isRoot: true,
    });
    // Both buttons must show their aria-labels even when collapsed
    // (the toolbar lives in the header which is always rendered).
    assert.ok(
      html.includes("Expand all replies in this thread"),
      `expected expand button aria-label, got: ${html}`
    );
    assert.ok(
      html.includes("Collapse all replies in this thread"),
      `expected collapse button aria-label, got: ${html}`
    );
  });

  it("isRoot omitted (default false) → WidgetRootToolbar does NOT render", () => {
    const html = renderThread({
      root: leaf("root"),
      collapseState: new CollapseState(),
    });
    assert.ok(
      !html.includes("Expand all replies in this thread"),
      `nested call must NOT render the toolbar, got: ${html}`
    );
    assert.ok(
      !html.includes("Collapse all replies in this thread"),
      `nested call must NOT render the toolbar, got: ${html}`
    );
  });

  it("root with zero replies AND isRoot=true → toolbar still renders", () => {
    const html = renderThread({
      root: leaf("solo"),
      collapseState: new CollapseState(),
      isRoot: true,
    });
    assert.ok(html.includes("Expand all replies in this thread"));
    assert.ok(html.includes("Collapse all replies in this thread"));
  });

  it("nested replies inside a root subtree do NOT render the toolbar", () => {
    // Build root with two replies; expand the root so the recursive
    // nested calls actually render. Toolbars must appear EXACTLY once
    // (only on the outer root row), not three times.
    const collapseState = new CollapseState();
    collapseState.setExpanded("root");
    const rootNode: ThreadNode = {
      comment: comment({ id: "root" }),
      replies: [leaf("r1", "root"), leaf("r2", "root")],
    };
    const html = renderThread({ root: rootNode, collapseState, isRoot: true });
    const expandMatches = html.match(/Expand all replies in this thread/g) ?? [];
    const collapseMatches = html.match(/Collapse all replies in this thread/g) ?? [];
    // `aria-label` and `title` both carry the string, so each rendered
    // button contributes 2 matches. One toolbar = 2 buttons = 4 matches.
    assert.equal(
      expandMatches.length,
      2,
      `exactly one expand toolbar; got ${expandMatches.length}`
    );
    assert.equal(
      collapseMatches.length,
      2,
      `exactly one collapse toolbar; got ${collapseMatches.length}`
    );
  });

  // The toolbar handlers delegate to `setSubtreeCollapsed`; testing
  // that helper directly asserts the wiring without needing to
  // intercept React event handlers.
  it("setSubtreeCollapsed(false) calls setMany with every subtree id and collapsed=false", () => {
    const collapseState = new CollapseState();
    const calls: Array<{ ids: readonly string[]; collapsed: boolean }> = [];
    const original = collapseState.setMany.bind(collapseState);
    collapseState.setMany = (ids, collapsed) => {
      calls.push({ ids, collapsed });
      original(ids, collapsed);
    };
    const rootNode: ThreadNode = {
      comment: comment({ id: "root" }),
      replies: [
        {
          comment: comment({ id: "r1", reply_to: "root" }),
          replies: [leaf("r1a", "r1")],
        },
        leaf("r2", "root"),
      ],
    };
    setSubtreeCollapsed(rootNode, collapseState, false);
    assert.equal(calls.length, 1, "expand must call setMany exactly once");
    assert.deepStrictEqual(
      [...calls[0].ids].sort(),
      ["r1", "r1a", "r2", "root"],
      "expand must pass root + every descendant id"
    );
    assert.equal(calls[0].collapsed, false);
    for (const id of ["root", "r1", "r1a", "r2"]) {
      assert.equal(collapseState.isCollapsed(id), false, `id ${id} must be expanded`);
    }
  });

  it("setSubtreeCollapsed(true) is a HARD RESET — overwrites previously-expanded descendants", () => {
    const collapseState = new CollapseState();
    collapseState.setExpanded("r1");
    assert.equal(collapseState.isCollapsed("r1"), false, "precondition: r1 expanded");
    const rootNode: ThreadNode = {
      comment: comment({ id: "root" }),
      replies: [leaf("r1", "root"), leaf("r2", "root")],
    };
    setSubtreeCollapsed(rootNode, collapseState, true);
    assert.equal(
      collapseState.isCollapsed("r1"),
      true,
      "collapse-all must overwrite previously-expanded descendants"
    );
    // Once collapse-all ran, has(r1) is true: the auto-expand priming
    // branch will not re-flip it on next mount (user choice persists).
    assert.equal(collapseState.has("r1"), true);
    assert.equal(collapseState.isCollapsed("root"), true);
    assert.equal(collapseState.isCollapsed("r2"), true);
  });
});
