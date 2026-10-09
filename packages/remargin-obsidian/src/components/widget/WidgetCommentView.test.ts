/** Tests for the single-comment widget: collapsed and expanded markup, click and toggle wiring. */

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
    el: undefined,
    id: "abc",
    line: 12,
    reactions: {},
    remargin_kind: [],
    reply_to: undefined,
    signature: undefined,
    sl: undefined,
    thread: undefined,
    to: [],
    ts: new Date("2026-04-25T12:00:00-04:00"),
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
 * Finds the `onClick` handler on the root `remargin-widget-comment` div of the element tree
 * `WidgetCommentView` returns; react-dom/server cannot dispatch DOM events.
 */
function findRootOnClick(element: ReactElement): ((event: unknown) => void) | undefined {
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
  // Called as a plain function: the returned element tree exposes the props without a DOM.
  const tree = WidgetCommentView({
    sourcePath: "notes/test.md",
    ...props,
  });
  return tree as ReactElement;
}

describe("WidgetCommentView", () => {
  it("renders header but no markdown body when collapsed", () => {
    const html = render(fixture(), true);
    assert.match(html, /<div[^>]*class="[^"]*bg-slate-500[^"]*"[^>]*>abc<\/div>/);
    assert.ok(
      !html.includes("remargin-markdown-content"),
      `expected no MarkdownContent body, got: ${html}`
    );
  });

  it("renders header and markdown body when expanded", () => {
    const html = render(fixture(), false);
    assert.match(html, /<div[^>]*class="[^"]*bg-slate-500[^"]*"[^>]*>abc<\/div>/);
    assert.ok(
      html.includes("remargin-markdown-content"),
      `expected MarkdownContent body, got: ${html}`
    );
  });

  it("shows the edited label and kind chips under the body when expanded", () => {
    const html = render(
      fixture({ edited_at: new Date(), remargin_kind: ["decision-done"] }),
      false
    );
    const body = html.indexOf("remargin-markdown-content");
    const tags = html.indexOf("remargin-widget-comment__tags");
    assert.ok(body > -1 && tags > body, html);
    assert.ok(html.includes('aria-label="Edited'), html);
    assert.ok(html.includes('aria-label="Kind: decision-done"'), html);
  });

  it("renders no tag row for an unedited comment without kinds, or when collapsed", () => {
    assert.ok(!render(fixture(), false).includes("remargin-widget-comment__tags"));
    const collapsed = render(fixture({ edited_at: new Date(), remargin_kind: ["x"] }), true);
    assert.ok(!collapsed.includes("remargin-widget-comment__tags"), collapsed);
  });

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
    handler({});
    assert.deepStrictEqual(calls, [["abc", "notes/test.md"]]);
  });

  // The click is stopped at the toggle and never reaches `onClick`.
  it("CollapseToggle click invokes onToggle without firing onClick", () => {
    const onClickCalls: Array<[string, string]> = [];
    const onToggleCalls: Array<true> = [];
    const tree = buildElement({
      comment: fixture({ id: "abc" }),
      collapsed: true,
      onClick: (id, file) => onClickCalls.push([id, file]),
      onToggle: () => onToggleCalls.push(true),
    });

    // In the collapsed state the toggle is the only `<button>` with an `onClick`.
    const toggleHandler = findToggleHandler(tree);
    assert.ok(toggleHandler, "expected CollapseToggle to render an onClick handler");

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
