import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { Participant } from "../backend/index.ts";
import { pickerOptions } from "./pickerOptions.ts";

function participant(overrides: Partial<Participant>): Participant {
  return {
    name: overrides.name ?? "alice",
    display_name: overrides.display_name ?? overrides.name ?? "alice",
    type: overrides.type ?? "human",
    status: overrides.status ?? "active",
    pubkeys: overrides.pubkeys ?? 0,
  };
}

describe("pickerOptions", () => {
  it("returns active participants untouched when nothing is selected", () => {
    const input = [
      participant({ name: "alice" }),
      participant({ name: "bob" }),
    ];
    const result = pickerOptions(input, []);
    assert.deepStrictEqual(result.map((p) => p.name), ["alice", "bob"]);
  });

  it("excludes revoked participants", () => {
    const input = [
      participant({ name: "alice", status: "active" }),
      participant({ name: "carol", status: "revoked" }),
      participant({ name: "bob", status: "active" }),
    ];
    const result = pickerOptions(input, []);
    assert.deepStrictEqual(result.map((p) => p.name), ["alice", "bob"]);
  });

  it("excludes already-selected ids", () => {
    const input = [
      participant({ name: "alice" }),
      participant({ name: "bob" }),
      participant({ name: "carol" }),
    ];
    const result = pickerOptions(input, ["bob"]);
    assert.deepStrictEqual(result.map((p) => p.name), ["alice", "carol"]);
  });

  it("returns an empty list when participants is empty", () => {
    assert.deepStrictEqual(pickerOptions([], []), []);
  });

  it("returns an empty list when every active participant is already selected", () => {
    const input = [
      participant({ name: "alice" }),
      participant({ name: "bob" }),
    ];
    assert.deepStrictEqual(pickerOptions(input, ["alice", "bob"]), []);
  });

  it("dedups by id — first occurrence wins", () => {
    const input = [
      participant({ name: "alice", display_name: "Alice One" }),
      participant({ name: "alice", display_name: "Alice Two" }),
      participant({ name: "bob" }),
    ];
    const result = pickerOptions(input, []);
    assert.strictEqual(result.length, 2);
    assert.strictEqual(result[0]?.display_name, "Alice One");
    assert.strictEqual(result[1]?.name, "bob");
  });

  it("preserves input order", () => {
    const input = [
      participant({ name: "zoe" }),
      participant({ name: "alice" }),
      participant({ name: "mark" }),
    ];
    const result = pickerOptions(input, []);
    assert.deepStrictEqual(result.map((p) => p.name), ["zoe", "alice", "mark"]);
  });

  it("combines revoked-filter + selected-filter + dedup in one pass", () => {
    const input = [
      participant({ name: "alice" }),
      participant({ name: "bob", status: "revoked" }),
      participant({ name: "alice" }), // duplicate
      participant({ name: "carol" }),
    ];
    const result = pickerOptions(input, ["carol"]);
    assert.deepStrictEqual(result.map((p) => p.name), ["alice"]);
  });
});
