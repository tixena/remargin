import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { ackAffordanceFor, ackStateFor } from "./ack-state.ts";

describe("ackStateFor", () => {
  it("returns 'unacked' when the ack list is empty", () => {
    assert.strictEqual(ackStateFor([], "eduardo"), "unacked");
  });

  it("returns 'me-acked' when the identity is present in the ack list", () => {
    assert.strictEqual(ackStateFor(["eduardo"], "eduardo"), "me-acked");
  });

  it("returns 'others-acked' when someone else acked but I haven't", () => {
    assert.strictEqual(ackStateFor(["alice"], "eduardo"), "others-acked");
  });

  it("returns 'me-acked' when I am among several ackers", () => {
    assert.strictEqual(ackStateFor(["alice", "eduardo", "bob"], "eduardo"), "me-acked");
  });

  it("falls back to 'others-acked' when identity is undefined but list is non-empty", () => {
    assert.strictEqual(ackStateFor(["alice"], undefined), "others-acked");
    assert.strictEqual(ackStateFor(["alice"], null), "others-acked");
  });

  it("returns 'unacked' when both the list and identity are empty", () => {
    assert.strictEqual(ackStateFor([], undefined), "unacked");
  });
});

/**
 * The ack affordance on a comment card depends on BOTH whether the
 * viewer has acked and whether the viewer is the comment's author. The
 * helper collapses that two-axis decision into a single value the card
 * consumes verbatim.
 */
describe("ackAffordanceFor", () => {
  // --- Viewer is the author ------------------------------------------

  it("author is me, empty roster → label + Ack kebab", () => {
    assert.deepStrictEqual(ackAffordanceFor("eduardo", [], "eduardo"), {
      kind: "label",
      kebab: "ack",
    });
  });

  it("author is me, others acked but not me → label + Ack kebab", () => {
    // Pill stays a label for own-comments (rule 1); kebab offers Ack
    // because viewer has not acked yet.
    assert.deepStrictEqual(ackAffordanceFor("eduardo", ["alice"], "eduardo"), {
      kind: "label",
      kebab: "ack",
    });
  });

  it("author is me, I acked → label + Unack kebab", () => {
    assert.deepStrictEqual(ackAffordanceFor("eduardo", ["eduardo"], "eduardo"), {
      kind: "label",
      kebab: "unack",
    });
  });

  it("author is me, I and others acked → label + Unack kebab", () => {
    assert.deepStrictEqual(
      ackAffordanceFor("eduardo", ["alice", "eduardo", "bob"], "eduardo"),
      { kind: "label", kebab: "unack" }
    );
  });

  // --- Viewer is NOT the author --------------------------------------

  it("author is someone else, empty roster → interactive button, no kebab item", () => {
    assert.deepStrictEqual(ackAffordanceFor("alice", [], "eduardo"), {
      kind: "button",
      kebab: "none",
    });
  });

  it("author is someone else, I acked → label + Unack kebab", () => {
    assert.deepStrictEqual(ackAffordanceFor("alice", ["eduardo"], "eduardo"), {
      kind: "label",
      kebab: "unack",
    });
  });

  it("author is someone else, only others acked → interactive button, no kebab item", () => {
    assert.deepStrictEqual(ackAffordanceFor("alice", ["bob"], "eduardo"), {
      kind: "button",
      kebab: "none",
    });
  });

  // --- Edge: me is null/undefined (identity not resolved yet) --------

  it("unknown viewer, empty roster → interactive button, no kebab item", () => {
    assert.deepStrictEqual(ackAffordanceFor("alice", [], null), {
      kind: "button",
      kebab: "none",
    });
    assert.deepStrictEqual(ackAffordanceFor("alice", [], undefined), {
      kind: "button",
      kebab: "none",
    });
  });

  it("unknown viewer never counts as 'author is me', even when strings coincide", () => {
    // `me=null` means identity hasn't resolved yet. The author field
    // happens to be an empty string here (pathological), but the
    // helper must not treat `null === ""` as authorship.
    assert.deepStrictEqual(ackAffordanceFor("", ["alice"], null), {
      kind: "button",
      kebab: "none",
    });
  });
});
