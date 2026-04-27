import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { CollapseState } from "./collapseState.ts";

describe("CollapseState", () => {
  // Test #5 (T36 spec): unknown ids read as collapsed.
  it("reads any unknown id as collapsed by default", () => {
    const state = new CollapseState();
    assert.equal(state.isCollapsed("anything"), true);
    assert.equal(state.isCollapsed("another"), true);
  });

  // Test #6: a single toggle flips the default to expanded.
  it("toggle flips the default-collapsed state to expanded", () => {
    const state = new CollapseState();
    state.toggle("abc");
    assert.equal(state.isCollapsed("abc"), false);
    state.toggle("abc");
    assert.equal(state.isCollapsed("abc"), true);
  });

  // Test #7: subscribe returns an unsubscribe thunk; after unsubscribe,
  // further toggles do not call the listener.
  it("subscribe returns an unsubscribe thunk that detaches the listener", () => {
    const state = new CollapseState();
    const calls: Array<[string, boolean]> = [];
    const listener = (id: string, collapsed: boolean) => {
      calls.push([id, collapsed]);
    };
    const unsubscribe = state.subscribe(listener);
    assert.equal(typeof unsubscribe, "function");

    state.toggle("a");
    assert.deepStrictEqual(calls, [["a", false]]);

    unsubscribe();
    state.toggle("a");
    // No new entry — listener detached.
    assert.deepStrictEqual(calls, [["a", false]]);
  });

  // Test #8: multiple subscribers all receive notifications on toggle.
  it("multiple subscribers all receive notifications on toggle", () => {
    const state = new CollapseState();
    const callsA: Array<[string, boolean]> = [];
    const callsB: Array<[string, boolean]> = [];
    state.subscribe((id, collapsed) => callsA.push([id, collapsed]));
    state.subscribe((id, collapsed) => callsB.push([id, collapsed]));
    state.toggle("xyz");
    assert.deepStrictEqual(callsA, [["xyz", false]]);
    assert.deepStrictEqual(callsB, [["xyz", false]]);
  });
});
