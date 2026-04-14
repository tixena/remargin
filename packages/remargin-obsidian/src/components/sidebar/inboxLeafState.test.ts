import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { Acknowledgment, ExpandedComment } from "../../generated/types.ts";
import { deriveLeafState } from "./inboxLeafState.ts";

function ack(author: string): Acknowledgment {
  return { author, ts: "2026-04-06T12:00:00-04:00" };
}

function fixture(
  overrides: Partial<Pick<ExpandedComment, "to" | "ack">>
): Pick<ExpandedComment, "to" | "ack"> {
  return {
    to: overrides.to ?? [],
    ack: overrides.ack ?? [],
  };
}

describe("deriveLeafState", () => {
  it("broadcast with no acks is directed at everyone (row 1)", () => {
    assert.deepStrictEqual(deriveLeafState(fixture({ to: [], ack: [] }), "alice"), {
      directedAtMe: true,
      ackedByMe: false,
      visual: "me-directed-unacked",
    });
  });

  it("explicit to containing me is directed at me (row 2)", () => {
    assert.deepStrictEqual(deriveLeafState(fixture({ to: ["alice"] }), "alice"), {
      directedAtMe: true,
      ackedByMe: false,
      visual: "me-directed-unacked",
    });
  });

  it("multi-recipient to containing me is directed at me (row 3)", () => {
    assert.deepStrictEqual(deriveLeafState(fixture({ to: ["alice", "bob"] }), "alice"), {
      directedAtMe: true,
      ackedByMe: false,
      visual: "me-directed-unacked",
    });
  });

  it("to without me is neutral (row 4)", () => {
    assert.deepStrictEqual(deriveLeafState(fixture({ to: ["bob"] }), "alice"), {
      directedAtMe: false,
      ackedByMe: false,
      visual: "neutral",
    });
  });

  it("acked by me wins over directed (row 5)", () => {
    const state = deriveLeafState(fixture({ to: ["alice"], ack: [ack("alice")] }), "alice");
    assert.strictEqual(state.visual, "acked-by-me");
    assert.strictEqual(state.directedAtMe, true);
    assert.strictEqual(state.ackedByMe, true);
  });

  it("acked by me wins even on broadcast (row 6)", () => {
    const state = deriveLeafState(fixture({ to: [], ack: [ack("alice")] }), "alice");
    assert.strictEqual(state.visual, "acked-by-me");
    assert.strictEqual(state.ackedByMe, true);
  });

  it("acked by me still dims even when directed at someone else (row 7)", () => {
    const state = deriveLeafState(fixture({ to: ["bob"], ack: [ack("alice")] }), "alice");
    assert.strictEqual(state.visual, "acked-by-me");
    assert.strictEqual(state.directedAtMe, false);
    assert.strictEqual(state.ackedByMe, true);
  });

  it("unknown me (null) renders everything neutral", () => {
    const state = deriveLeafState(fixture({ to: ["alice"], ack: [ack("alice")] }), null);
    assert.deepStrictEqual(state, {
      directedAtMe: false,
      ackedByMe: false,
      visual: "neutral",
    });
  });

  it("unknown me (undefined) renders everything neutral", () => {
    const state = deriveLeafState(fixture({ to: [], ack: [] }), undefined);
    assert.strictEqual(state.visual, "neutral");
  });

  it("ack by someone else is not ackedByMe", () => {
    const state = deriveLeafState(fixture({ to: [], ack: [ack("bob")] }), "alice");
    assert.strictEqual(state.ackedByMe, false);
    assert.strictEqual(state.visual, "me-directed-unacked");
  });
});
