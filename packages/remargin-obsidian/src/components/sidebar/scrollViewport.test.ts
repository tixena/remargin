import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { findRadixScrollViewport } from "./scrollViewport.ts";

// Minimal element shim that implements just the two members our helper
// touches: `hasAttribute` and `parentElement`. Covers the traversal
// contract without pulling in jsdom.
interface FakeNode {
  name: string;
  attrs?: Record<string, string>;
  parent?: FakeNode | null;
}

function asElement(node: FakeNode): HTMLElement {
  return {
    hasAttribute(name: string): boolean {
      return Boolean(node.attrs?.[name] !== undefined);
    },
    get parentElement(): HTMLElement | null {
      return node.parent ? asElement(node.parent) : null;
    },
  } as unknown as HTMLElement;
}

describe("findRadixScrollViewport", () => {
  it("returns null when the start node is null", () => {
    assert.equal(findRadixScrollViewport(null), null);
  });

  it("returns null when no ancestor carries the viewport attribute", () => {
    const root: FakeNode = { name: "root" };
    const mid: FakeNode = { name: "mid", parent: root };
    const leaf: FakeNode = { name: "leaf", parent: mid };
    assert.equal(findRadixScrollViewport(asElement(leaf)), null);
  });

  it("returns the start node when it itself is the viewport", () => {
    const start: FakeNode = {
      name: "viewport",
      attrs: { "data-radix-scroll-area-viewport": "" },
    };
    const found = findRadixScrollViewport(asElement(start));
    assert.ok(found);
  });

  it("walks up the tree to find the nearest viewport ancestor", () => {
    const viewport: FakeNode = {
      name: "viewport",
      attrs: { "data-radix-scroll-area-viewport": "" },
    };
    const mid: FakeNode = { name: "mid", parent: viewport };
    const leaf: FakeNode = { name: "leaf", parent: mid };
    const found = findRadixScrollViewport(asElement(leaf));
    assert.ok(found, "expected to find a viewport ancestor");
  });

  it("returns the NEAREST viewport when multiple are nested", () => {
    // The outer SidebarShell ScrollArea sits inside the Obsidian
    // workspace, which may itself be a ScrollArea in some layouts.
    // We always want the nearest one so scroll restoration targets
    // the container that actually moved.
    const outerViewport: FakeNode = {
      name: "outer",
      attrs: { "data-radix-scroll-area-viewport": "outer" },
    };
    const innerViewport: FakeNode = {
      name: "inner",
      attrs: { "data-radix-scroll-area-viewport": "inner" },
      parent: outerViewport,
    };
    const leaf: FakeNode = { name: "leaf", parent: innerViewport };
    const found = findRadixScrollViewport(asElement(leaf));
    assert.ok(found);
    assert.equal(found.hasAttribute("data-radix-scroll-area-viewport"), true);
    // Verify we stopped at the inner one (not walked past it).
    // Since our shim returns a fresh proxy each call, we check by
    // walking the parent chain from `found`: its parent should be
    // the outer viewport.
    const parent = found.parentElement;
    assert.ok(parent);
    assert.equal(parent.hasAttribute("data-radix-scroll-area-viewport"), true);
  });
});
