import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Participant, RemarginBackend } from "../../backend/index.ts";
import type { Comment } from "../../generated/types.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { __resetParticipantsCacheForTests } from "../../hooks/useParticipants.ts";
import { PluginContext } from "../../hooks/usePlugin.ts";
import type RemarginPlugin from "../../main.ts";
import { DEFAULT_SETTINGS } from "../../types.ts";
import { WidgetCommentView } from "./WidgetCommentView.tsx";

// Same minimal stand-ins as CommentHeader.test.ts — useParticipants
// only reads `plugin.settings` and calls `backend.registryShow`.
const pluginStub = { settings: DEFAULT_SETTINGS } as unknown as RemarginPlugin;
const backendStub = {
  registryShow: (): Promise<Participant[]> => Promise.resolve([]),
} as unknown as RemarginBackend;

function fixture(overrides: Partial<Comment> = {}): Comment {
  return {
    ack: [],
    attachments: [],
    author: "alice",
    author_type: "human",
    checksum: "",
    content: "hello widget",
    edited_at: undefined,
    id: "abc",
    line: 12,
    reactions: {},
    remargin_kind: [],
    reply_to: undefined,
    signature: undefined,
    thread: undefined,
    to: [],
    ts: "2026-04-25T12:00:00-04:00",
    ...overrides,
  };
}

function render(
  comment: Comment,
  collapsed: boolean,
  onClick: (id: string, file: string) => void = noop,
  onToggle: () => void = noop
): string {
  __resetParticipantsCacheForTests();
  return renderToStaticMarkup(
    createElement(
      PluginContext.Provider,
      { value: pluginStub },
      createElement(
        BackendContext.Provider,
        { value: backendStub },
        createElement(WidgetCommentView, {
          comment,
          sourcePath: "notes/test.md",
          collapsed,
          onToggle,
          onClick,
        })
      )
    )
  );
}

const noop = () => {
  /* test-only no-op handler */
};

/**
 * Walk the React element tree returned by `WidgetCommentView` and find
 * the `onClick` handler attached to the root `remargin-widget-comment`
 * div. Used to drive tests #3 and #4 since react-dom/server cannot
 * dispatch DOM events.
 */
function findRootOnClick(element: ReactElement): ((event: unknown) => void) | undefined {
  // The component returns a single <div className="remargin-widget-comment">.
  // We invoke the function component directly so we can introspect props.
  const props = element.props as Record<string, unknown>;
  const onClick = props.onClick;
  return typeof onClick === "function" ? (onClick as (event: unknown) => void) : undefined;
}

function buildElement(props: {
  comment: Comment;
  collapsed: boolean;
  onClick: (id: string, file: string) => void;
  onToggle: () => void;
}): ReactElement {
  // Call the component as a plain function — function components are
  // pure during this kind of inspection. Returns the same element tree
  // React would otherwise reconcile, which lets us reach `onClick` /
  // toggle props without standing up a DOM.
  const tree = WidgetCommentView({
    sourcePath: "notes/test.md",
    ...props,
  });
  return tree as ReactElement;
}

describe("WidgetCommentView", () => {
  // Test #1 (T36 spec): collapsed=true renders header but NOT body.
  it("renders header but no markdown body when collapsed", () => {
    const html = render(fixture(), true);
    // Header always present (id badge from CommentHeader).
    assert.match(html, /<div[^>]*class="[^"]*bg-slate-500[^"]*"[^>]*>abc<\/div>/);
    // MarkdownContent renders a div with `remargin-markdown-content`
    // — must NOT appear when collapsed.
    assert.ok(
      !html.includes("remargin-markdown-content"),
      `expected no MarkdownContent body, got: ${html}`
    );
  });

  // Test #2: collapsed=false renders header AND body.
  it("renders header and markdown body when expanded", () => {
    const html = render(fixture(), false);
    assert.match(html, /<div[^>]*class="[^"]*bg-slate-500[^"]*"[^>]*>abc<\/div>/);
    assert.ok(
      html.includes("remargin-markdown-content"),
      `expected MarkdownContent body, got: ${html}`
    );
  });

  // Test #3: clicking the widget root invokes onClick(commentId, sourcePath).
  it("widget-root click invokes onClick with comment id and source path", () => {
    const calls: Array<[string, string]> = [];
    const onClick = (id: string, file: string) => {
      calls.push([id, file]);
    };
    const tree = buildElement({
      comment: fixture({ id: "abc" }),
      collapsed: true,
      onClick,
      onToggle: noop,
    });
    const handler = findRootOnClick(tree);
    assert.ok(handler, "expected a root onClick handler");
    // Synthetic-event shape doesn't matter — the handler ignores the
    // event argument and forwards (id, file) directly.
    handler({});
    assert.deepStrictEqual(calls, [["abc", "notes/test.md"]]);
  });

  // Test #4: clicking the CollapseToggle invokes onToggle and does NOT
  // bubble to onClick (event stopped at the toggle).
  it("CollapseToggle click invokes onToggle without firing onClick", () => {
    const onClickCalls: Array<[string, string]> = [];
    const onToggleCalls: Array<true> = [];
    const tree = buildElement({
      comment: fixture({ id: "abc" }),
      collapsed: true,
      onClick: (id, file) => onClickCalls.push([id, file]),
      onToggle: () => onToggleCalls.push(true),
    });

    // The root child is the header wrapper; CollapseToggle is the first
    // grandchild. Walk the tree until we find a button with a real
    // `onClick` (the toggle is the only `<button>` rendered in
    // collapsed state since the body is hidden).
    const toggleHandler = findToggleHandler(tree);
    assert.ok(toggleHandler, "expected CollapseToggle to render an onClick handler");

    // Drive the handler with a stopPropagation-aware mock event so the
    // toggle's `event.stopPropagation()` call has something to invoke.
    let stopped = false;
    toggleHandler({
      stopPropagation: () => {
        stopped = true;
      },
    });
    assert.equal(stopped, true, "expected toggle to call event.stopPropagation");
    assert.deepStrictEqual(onToggleCalls, [true], "onToggle must fire once");
    assert.deepStrictEqual(onClickCalls, [], "onClick must NOT fire when toggle is clicked");
  });
});

/**
 * Locate the CollapseToggle's onClick by recursively walking the React
 * element tree returned by `WidgetCommentView`. Returns the first
 * `<button>` with an `onClick` prop — the toggle is the only such
 * button in the collapsed-state render tree.
 */
function findToggleHandler(element: unknown): ((event: unknown) => void) | undefined {
  if (!element || typeof element !== "object") return undefined;
  const node = element as ReactElement & {
    type?: unknown;
    props?: { onClick?: unknown; children?: unknown };
  };
  // We need to step inside CollapseToggle: it's a function component
  // whose returned element IS the button. Call the function with its
  // resolved props to descend.
  if (typeof node.type === "function") {
    const rendered = (node.type as (props: unknown) => ReactElement)(node.props);
    return findToggleHandler(rendered);
  }
  if (node.type === "button" && typeof node.props?.onClick === "function") {
    return node.props.onClick as (event: unknown) => void;
  }
  const children = node.props?.children;
  if (Array.isArray(children)) {
    for (const child of children) {
      const found = findToggleHandler(child);
      if (found) return found;
    }
  } else if (children) {
    return findToggleHandler(children);
  }
  return undefined;
}
