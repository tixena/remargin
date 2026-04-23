import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { collectKinds, matchesKindFilter, pruneKindFilter } from "./kindFilter.ts";

describe("collectKinds", () => {
  it("returns an empty list when no items carry kinds", () => {
    assert.deepEqual(collectKinds([]), []);
    assert.deepEqual(collectKinds([{ remargin_kind: [] }]), []);
  });

  // remargin_kind is optional on the wire (the Rust struct emits it as
  // Option<Vec<String>> with skip_serializing_if). Pre-field comments
  // and any payload that simply lacks the key must not trip collectKinds.
  it("tolerates items where remargin_kind is absent (undefined)", () => {
    assert.deepEqual(collectKinds([{}, {}]), []);
    assert.deepEqual(collectKinds([{ remargin_kind: undefined }]), []);
    const items: Array<{ remargin_kind?: string[] }> = [
      {},
      { remargin_kind: ["question"] },
      { remargin_kind: undefined },
      { remargin_kind: ["action-item"] },
    ];
    assert.deepEqual(collectKinds(items), ["action-item", "question"]);
  });

  it("de-duplicates across items", () => {
    const items = [
      { remargin_kind: ["question"] },
      { remargin_kind: ["question", "action-item"] },
      { remargin_kind: ["action-item"] },
    ];
    assert.deepEqual(collectKinds(items), ["action-item", "question"]);
  });

  it("sorts case-insensitively but preserves stored casing", () => {
    const items = [{ remargin_kind: ["Bug"] }, { remargin_kind: ["action"] }];
    assert.deepEqual(collectKinds(items), ["action", "Bug"]);
  });

  it("skips empty kind strings defensively", () => {
    const items = [{ remargin_kind: ["", "question"] }];
    assert.deepEqual(collectKinds(items), ["question"]);
  });
});

describe("matchesKindFilter", () => {
  it("matches every comment when the filter is empty", () => {
    assert.strictEqual(matchesKindFilter([], []), true);
    assert.strictEqual(matchesKindFilter(["question"], []), true);
  });

  it("excludes comments with no kinds when the filter is non-empty", () => {
    assert.strictEqual(matchesKindFilter([], ["question"]), false);
  });

  // A comment with no `remargin_kind` field at all (undefined on the
  // wire — pre-field comments serialize with the key absent) must
  // follow the same rules as an explicit empty array.
  it("treats undefined kinds the same as an empty array", () => {
    assert.strictEqual(matchesKindFilter(undefined, []), true);
    assert.strictEqual(matchesKindFilter(undefined, ["question"]), false);
  });

  it("matches with OR semantics when the filter has multiple values", () => {
    assert.strictEqual(matchesKindFilter(["question"], ["question", "bug"]), true);
    assert.strictEqual(matchesKindFilter(["bug"], ["question", "bug"]), true);
    assert.strictEqual(matchesKindFilter(["other"], ["question", "bug"]), false);
  });

  it("matches when the comment carries at least one selected kind", () => {
    assert.strictEqual(matchesKindFilter(["question", "other"], ["question"]), true);
  });
});

describe("pruneKindFilter", () => {
  it("returns the original filter reference when everything is still available", () => {
    const filter = ["question", "bug"];
    const pruned = pruneKindFilter(filter, ["question", "bug", "action-item"]);
    assert.strictEqual(pruned, filter);
  });

  it("drops selections that disappeared from the visible set", () => {
    const filter = ["question", "bug"];
    const pruned = pruneKindFilter(filter, ["question"]);
    assert.deepEqual(pruned, ["question"]);
  });

  it("returns an empty array when nothing is still available", () => {
    assert.deepEqual(pruneKindFilter(["question", "bug"], []), []);
  });

  it("returns the original reference when the filter is already empty", () => {
    const filter: string[] = [];
    const pruned = pruneKindFilter(filter, ["question"]);
    assert.strictEqual(pruned, filter);
  });
});
