import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { Comment } from "@/generated/types";
import {
  isPendingBroadcast,
  isPendingFor,
  shouldAutoExpand,
  summarizeThread,
} from "./pendingState.ts";
import { buildThreadTree } from "./threadTree.ts";

interface MkOpts {
  to?: string[];
  ackBy?: string[];
  replyTo?: string;
}

function mk(id: string, opts: MkOpts = {}): Comment {
  return {
    ack: (opts.ackBy ?? []).map((author) => ({ author, ts: "2026-01-01T00:00:00Z" })),
    attachments: [],
    author: "alice",
    author_type: "human",
    checksum: "",
    content: "",
    edited_at: undefined,
    id,
    line: 0,
    reactions: {},
    remargin_kind: undefined,
    reply_to: opts.replyTo,
    signature: undefined,
    thread: undefined,
    to: opts.to ?? [],
    ts: "2026-01-01T00:00:00Z",
  };
}

describe("isPendingFor", () => {
  it("true when recipient is in `to` and has not acked", () => {
    assert.equal(isPendingFor(mk("a", { to: ["bob"] }), "bob"), true);
  });

  it("false when recipient acked", () => {
    assert.equal(
      isPendingFor(mk("a", { to: ["bob"], ackBy: ["bob"] }), "bob"),
      false,
    );
  });

  it("false when recipient is not in `to`", () => {
    assert.equal(isPendingFor(mk("a", { to: ["alice"] }), "bob"), false);
  });

  it("false when `to` is empty (broadcast — different concept)", () => {
    assert.equal(isPendingFor(mk("a", { to: [] }), "bob"), false);
  });
});

describe("isPendingBroadcast", () => {
  it("true when no `to` and no acks", () => {
    assert.equal(isPendingBroadcast(mk("a")), true);
  });

  it("false when `to` is non-empty", () => {
    assert.equal(isPendingBroadcast(mk("a", { to: ["bob"] })), false);
  });

  it("false when broadcast already has any ack", () => {
    assert.equal(isPendingBroadcast(mk("a", { to: [], ackBy: ["bob"] })), false);
  });
});

describe("summarizeThread", () => {
  it("counts replies excluding root, plus pending categories", () => {
    const trees = buildThreadTree([
      mk("r", { to: ["bob"] }), // pending for bob
      mk("c1", { to: ["bob"], ackBy: ["bob"], replyTo: "r" }), // acked
      mk("c2", { to: [], replyTo: "r" }), // broadcast pending
    ]);
    const s = summarizeThread(trees[0], "bob");
    assert.equal(s.totalReplies, 2);
    assert.equal(s.pendingForMe, 2); // r is pending for me, c2 is broadcast
    assert.equal(s.pendingForOthers, 0);
  });

  it("counts pending for others when me is in nobody's `to`", () => {
    const trees = buildThreadTree([mk("r", { to: ["alice"] })]);
    const s = summarizeThread(trees[0], "bob");
    assert.equal(s.totalReplies, 0);
    assert.equal(s.pendingForMe, 0);
    assert.equal(s.pendingForOthers, 1);
  });

  it("totalReplies is 0 for a single-root thread", () => {
    const trees = buildThreadTree([mk("solo")]);
    const s = summarizeThread(trees[0], "alice");
    assert.equal(s.totalReplies, 0);
  });
});

describe("shouldAutoExpand", () => {
  it("true when subtree contains a pending-for-me comment", () => {
    const trees = buildThreadTree([
      mk("r", { to: ["alice"], ackBy: ["alice"] }),
      mk("c", { to: ["bob"], replyTo: "r" }),
    ]);
    assert.equal(shouldAutoExpand(trees[0], "bob"), true);
  });

  it("true when subtree contains a broadcast pending", () => {
    const trees = buildThreadTree([mk("r"), mk("c", { replyTo: "r" })]);
    // Both r and c are broadcast (no to:, no ack).
    assert.equal(shouldAutoExpand(trees[0], "anyone"), true);
  });

  it("false when every comment is fully acked", () => {
    const trees = buildThreadTree([
      mk("r", { to: ["bob"], ackBy: ["bob"] }),
      mk("c", { to: ["alice"], ackBy: ["alice"], replyTo: "r" }),
    ]);
    assert.equal(shouldAutoExpand(trees[0], "alice"), false);
  });

  it("false when only directed pendings exist for someone else", () => {
    const trees = buildThreadTree([mk("r", { to: ["alice"] })]);
    assert.equal(shouldAutoExpand(trees[0], "bob"), false);
  });

  it("broadcast pending fires regardless of identity", () => {
    const trees = buildThreadTree([mk("solo")]);
    assert.equal(shouldAutoExpand(trees[0], null), true);
  });
});
