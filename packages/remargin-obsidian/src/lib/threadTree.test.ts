import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { Comment } from "@/generated/types";
import { buildThreadTree, indexById, walkThread } from "./threadTree.ts";

function mkComment(id: string, replyTo?: string): Comment {
  return {
    ack: [],
    attachments: [],
    author: "tester",
    author_type: "human",
    checksum: "",
    content: "",
    edited_at: undefined,
    id,
    line: 0,
    reactions: {},
    remargin_kind: undefined,
    reply_to: replyTo,
    signature: undefined,
    thread: undefined,
    to: [],
    ts: "2026-01-01T00:00:00Z",
  };
}

describe("buildThreadTree", () => {
  it("returns roots (no reply_to) at top level", () => {
    const cs = [mkComment("a"), mkComment("b")];
    const trees = buildThreadTree(cs);
    assert.equal(trees.length, 2);
    assert.equal(trees[0].comment.id, "a");
    assert.equal(trees[1].comment.id, "b");
  });

  it("nests reply under its parent", () => {
    const cs = [mkComment("root"), mkComment("reply", "root")];
    const trees = buildThreadTree(cs);
    assert.equal(trees.length, 1);
    assert.equal(trees[0].replies.length, 1);
    assert.equal(trees[0].replies[0].comment.id, "reply");
  });

  it("treats reply with missing parent as orphan root", () => {
    const cs = [mkComment("orphan", "missing-parent-id")];
    const trees = buildThreadTree(cs);
    assert.equal(trees.length, 1);
    assert.equal(trees[0].comment.id, "orphan");
    assert.equal(trees[0].replies.length, 0);
  });

  it("nests deeply (depth 3+)", () => {
    const cs = [
      mkComment("r"),
      mkComment("c1", "r"),
      mkComment("c2", "c1"),
      mkComment("c3", "c2"),
    ];
    const trees = buildThreadTree(cs);
    assert.equal(trees.length, 1);
    assert.equal(trees[0].replies[0].comment.id, "c1");
    assert.equal(trees[0].replies[0].replies[0].comment.id, "c2");
    assert.equal(trees[0].replies[0].replies[0].replies[0].comment.id, "c3");
  });
});

describe("indexById", () => {
  it("indexes comments by id", () => {
    const cs = [mkComment("a"), mkComment("b")];
    const map = indexById(cs);
    assert.equal(map.get("a")?.id, "a");
    assert.equal(map.get("b")?.id, "b");
    assert.equal(map.get("c"), undefined);
  });
});

describe("walkThread", () => {
  it("yields root then descendants in depth-first order", () => {
    const trees = buildThreadTree([
      mkComment("r"),
      mkComment("a", "r"),
      mkComment("a1", "a"),
      mkComment("b", "r"),
    ]);
    const ids = Array.from(walkThread(trees[0])).map((c) => c.id);
    assert.deepStrictEqual(ids, ["r", "a", "a1", "b"]);
  });

  it("yields a single root with no descendants", () => {
    const trees = buildThreadTree([mkComment("solo")]);
    const ids = Array.from(walkThread(trees[0])).map((c) => c.id);
    assert.deepStrictEqual(ids, ["solo"]);
  });
});
