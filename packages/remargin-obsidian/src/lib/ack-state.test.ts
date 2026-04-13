import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { ackStateFor } from "./ack-state.ts";

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
