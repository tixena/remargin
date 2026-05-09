import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { ReactElement } from "react";
import { WidgetThreadActions } from "./WidgetThreadActions.tsx";

interface ButtonProps {
  type?: string;
  "aria-label"?: string;
  title?: string;
  className?: string;
  onClick?: (event: { stopPropagation: () => void }) => void;
}

function buttons(): ReactElement[] {
  const tree = WidgetThreadActions({
    onExpandAll: () => undefined,
    onCollapseAll: () => undefined,
  }) as ReactElement;
  // The component returns <div>{button}{button}</div>; props.children is
  // a 2-element array of ReactElements.
  const children = (tree.props as { children: ReactElement[] }).children;
  assert.equal(children.length, 2, "expected exactly two action buttons");
  return children;
}

function buildWith(handlers: {
  onExpandAll: () => void;
  onCollapseAll: () => void;
}): ReactElement[] {
  const tree = WidgetThreadActions(handlers) as ReactElement;
  const children = (tree.props as { children: ReactElement[] }).children;
  return children;
}

describe("WidgetThreadActions", () => {
  it("renders exactly two buttons with type, aria-label, and title attributes", () => {
    const [expand, collapse] = buttons();
    const expandProps = expand.props as ButtonProps;
    const collapseProps = collapse.props as ButtonProps;
    assert.equal(expandProps.type, "button");
    assert.equal(collapseProps.type, "button");
    assert.equal(expandProps["aria-label"], "Expand all replies in this thread");
    assert.equal(collapseProps["aria-label"], "Collapse all replies in this thread");
    assert.equal(expandProps.title, "Expand all replies in this thread");
    assert.equal(collapseProps.title, "Collapse all replies in this thread");
  });

  it("first button click invokes onExpandAll exactly once", () => {
    let expandCalls = 0;
    let collapseCalls = 0;
    const [expand] = buildWith({
      onExpandAll: () => {
        expandCalls += 1;
      },
      onCollapseAll: () => {
        collapseCalls += 1;
      },
    });
    const onClick = (expand.props as ButtonProps).onClick;
    assert.ok(onClick, "expand button must have an onClick handler");
    onClick({ stopPropagation: () => undefined });
    assert.equal(expandCalls, 1);
    assert.equal(collapseCalls, 0);
  });

  it("second button click invokes onCollapseAll exactly once", () => {
    let expandCalls = 0;
    let collapseCalls = 0;
    const [, collapse] = buildWith({
      onExpandAll: () => {
        expandCalls += 1;
      },
      onCollapseAll: () => {
        collapseCalls += 1;
      },
    });
    const onClick = (collapse.props as ButtonProps).onClick;
    assert.ok(onClick, "collapse button must have an onClick handler");
    onClick({ stopPropagation: () => undefined });
    assert.equal(collapseCalls, 1);
    assert.equal(expandCalls, 0);
  });

  it("each button calls event.stopPropagation so the outer widget click does NOT bubble", () => {
    const [expand, collapse] = buildWith({
      onExpandAll: () => undefined,
      onCollapseAll: () => undefined,
    });
    let stopped = 0;
    const event = {
      stopPropagation: () => {
        stopped += 1;
      },
    };
    (expand.props as ButtonProps).onClick?.(event);
    (collapse.props as ButtonProps).onClick?.(event);
    assert.equal(stopped, 2, "expected stopPropagation called once per button click");
  });

  it("buttons carry the `remargin-widget-thread-actions__btn` class for CSS hooks", () => {
    const [expand, collapse] = buttons();
    assert.equal((expand.props as ButtonProps).className, "remargin-widget-thread-actions__btn");
    assert.equal((collapse.props as ButtonProps).className, "remargin-widget-thread-actions__btn");
  });
});
