import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { parseRemarginBlocks } from "./parseRemarginBlocks.ts";

// The Rust writer (`crates/remargin-core/src/writer.rs::serialize_comment`)
// is the canonical on-disk format. These tests pin the parser to
// `type:` (NOT `author_type:`) and `reply-to:` (NOT `reply_to:`); drift
// flattens threads and inverts author badges in the widget.
describe("parseRemarginBlocks — canonical on-disk YAML keys", () => {
  it("reads `type: agent` from YAML and lands it on comment.author_type", () => {
    const doc = [
      "```remargin",
      "---",
      "id: c1",
      "author: alice",
      "type: agent",
      "ts: 2026-04-25T12:00:00-04:00",
      "---",
      "agent comment",
      "```",
    ].join("\n");

    const blocks = parseRemarginBlocks(doc);
    assert.equal(blocks.length, 1);
    assert.equal(blocks[0].valid, true);
    assert.equal(blocks[0].comment.author_type, "agent");
  });

  it("reads `type: human` from YAML and lands it on comment.author_type", () => {
    const doc = [
      "```remargin",
      "---",
      "id: c1",
      "author: alice",
      "type: human",
      "ts: 2026-04-25T12:00:00-04:00",
      "---",
      "human comment",
      "```",
    ].join("\n");

    const blocks = parseRemarginBlocks(doc);
    assert.equal(blocks[0].comment.author_type, "human");
  });

  it("reads `reply-to: c1` from YAML and lands it on comment.reply_to", () => {
    const doc = [
      "```remargin",
      "---",
      "id: c2",
      "author: bob",
      "type: human",
      "ts: 2026-04-25T12:01:00-04:00",
      "reply-to: c1",
      "---",
      "reply",
      "```",
    ].join("\n");

    const blocks = parseRemarginBlocks(doc);
    assert.equal(blocks.length, 1);
    assert.equal(blocks[0].comment.reply_to, "c1");
  });

  it("YAML keys with hyphens are accepted by the key regex", () => {
    // Regression: the original regex was `/^(\w+):/` which silently
    // dropped any line whose key contained a hyphen. `reply-to:` was
    // ignored entirely and threading came out flat.
    const doc = [
      "```remargin",
      "---",
      "id: c2",
      "author: bob",
      "type: human",
      "ts: 2026-04-25T12:01:00-04:00",
      "reply-to: parent-id",
      "---",
      "reply",
      "```",
    ].join("\n");

    const blocks = parseRemarginBlocks(doc);
    // If the key regex dropped the line, reply_to would be undefined.
    assert.equal(blocks[0].comment.reply_to, "parent-id");
  });
});
