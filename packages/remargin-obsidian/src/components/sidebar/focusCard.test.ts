import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { focusCardInRoot, HIGHLIGHT_DURATION_MS } from "./focusCard.ts";

/**
 * Build a minimum fake-DOM root with one card matching the supplied
 * `commentId`. Records every scroll + classList call so tests can
 * assert ordering, payload, and timer-driven cleanup.
 */
function makeRootWithCard(commentId: string) {
  const events: string[] = [];
  const card = {
    scrollIntoView(options?: ScrollIntoViewOptions | boolean) {
      events.push(`scroll:${JSON.stringify(options)}`);
    },
    classList: {
      add(name: string) {
        events.push(`add:${name}`);
      },
      remove(name: string) {
        events.push(`remove:${name}`);
      },
    },
  };
  const root = {
    querySelector(selector: string) {
      // Only return the card if the selector targets its id; mirrors
      // the real-DOM behaviour and lets tests for "wrong id" pass a
      // root that returns null.
      const expected = `[data-comment-id="${commentId}"]`;
      return selector === expected ? card : null;
    },
  };
  return { root, card, events };
}

describe("focusCardInRoot", () => {
  // Underpins T36 AC #9: with a card mounted, the helper scrolls,
  // applies the highlight class, and schedules its removal.
  it("scrolls the matching card and applies remargin-highlight", () => {
    const { root, events } = makeRootWithCard("abc");
    const timeouts: Array<{ handler: () => void; ms: number }> = [];
    const fakeSetTimeout = (handler: () => void, ms: number) => {
      timeouts.push({ handler, ms });
      return 0;
    };
    const found = focusCardInRoot(root, "abc", fakeSetTimeout);
    assert.equal(found, true, "expected the card to be found");
    assert.deepStrictEqual(events, [
      `scroll:${JSON.stringify({ block: "center", behavior: "smooth" })}`,
      "add:remargin-highlight",
    ]);
    assert.equal(timeouts.length, 1, "expected one scheduled timeout");
    assert.equal(timeouts[0].ms, HIGHLIGHT_DURATION_MS);
  });

  // Underpins T36 AC #9: after the timeout fires, the highlight class
  // is removed.
  it("removes remargin-highlight after the timeout fires", () => {
    const { root, events } = makeRootWithCard("abc");
    const timeouts: Array<{ handler: () => void; ms: number }> = [];
    const fakeSetTimeout = (handler: () => void, ms: number) => {
      timeouts.push({ handler, ms });
      return 0;
    };
    focusCardInRoot(root, "abc", fakeSetTimeout);
    timeouts[0].handler();
    assert.deepStrictEqual(events, [
      `scroll:${JSON.stringify({ block: "center", behavior: "smooth" })}`,
      "add:remargin-highlight",
      "remove:remargin-highlight",
    ]);
  });

  // Underpins T36 AC #11 (silent no-op when no subscriber/match): when
  // the root has no matching card, the call returns false and does
  // not throw or emit side effects.
  it("returns false and emits no side effects when no card matches", () => {
    let timeoutCalls = 0;
    const root = { querySelector: () => null };
    const found = focusCardInRoot(root, "missing", () => {
      timeoutCalls += 1;
      return 0;
    });
    assert.equal(found, false);
    assert.equal(timeoutCalls, 0, "expected no setTimeout call");
  });
});
