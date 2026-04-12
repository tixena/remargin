import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { snapAfterCommentBlock } from "./line-snap.ts";

/**
 * Helper that expands a multi-line string literal into the `lines` argument
 * expected by `snapAfterCommentBlock`. Using a helper keeps the test bodies
 * readable — the raw string matches what the file looks like on disk.
 */
function toLines(text: string): string[] {
  return text.split("\n");
}

describe("snapAfterCommentBlock", () => {
  it("returns the target line unchanged when there are no remargin blocks", () => {
    const lines = toLines("alpha\nbeta\ngamma\ndelta");
    assert.equal(snapAfterCommentBlock(lines, 2), 2);
    assert.equal(snapAfterCommentBlock(lines, 4), 4);
  });

  it("returns the target line unchanged when the cursor is outside every block", () => {
    const lines = toLines(
      [
        "line1",
        "line2",
        "```remargin",
        "---",
        "id: abc",
        "---",
        "body",
        "```",
        "line9",
        "line10",
      ].join("\n")
    );
    // Line 2 is before the block, line 9 is after it.
    assert.equal(snapAfterCommentBlock(lines, 2), 2);
    assert.equal(snapAfterCommentBlock(lines, 9), 9);
    assert.equal(snapAfterCommentBlock(lines, 10), 10);
  });

  it("snaps to the line after the closing fence when the cursor is inside a block", () => {
    const lines = toLines(
      [
        "intro",
        "```remargin", // 2
        "---", // 3
        "id: abc", // 4
        "---", // 5
        "body", // 6
        "```", // 7
        "outro", // 8
      ].join("\n")
    );
    // Cursor on the YAML line inside the block snaps to line 8.
    assert.equal(snapAfterCommentBlock(lines, 4), 8);
    // Cursor on body line inside the block snaps to line 8.
    assert.equal(snapAfterCommentBlock(lines, 6), 8);
  });

  it("treats the opening fence as inside the block", () => {
    const lines = toLines(
      ["intro", "```remargin", "---", "id: x", "---", "body", "```", "outro"].join("\n")
    );
    assert.equal(snapAfterCommentBlock(lines, 2), 8);
  });

  it("treats the closing fence as inside the block", () => {
    const lines = toLines(
      ["intro", "```remargin", "---", "id: x", "---", "body", "```", "outro"].join("\n")
    );
    // Line 7 is the closing fence — we still want to snap past it because
    // inserting "after" the fence and "before" the fence would otherwise be
    // ambiguous.
    assert.equal(snapAfterCommentBlock(lines, 7), 8);
  });

  it("snaps across adjacent stacked blocks", () => {
    const lines = toLines(
      [
        "intro", // 1
        "```remargin", // 2
        "---", // 3
        "id: a", // 4
        "---", // 5
        "body a", // 6
        "```", // 7
        "```remargin", // 8  — adjacent block starts on the next line
        "---", // 9
        "id: b", // 10
        "---", // 11
        "body b", // 12
        "```", // 13
        "tail", // 14
      ].join("\n")
    );
    // Cursor on line 4 (inside block A) snaps to 8 (inside block B), then
    // across to 14.
    assert.equal(snapAfterCommentBlock(lines, 4), 14);
    // Cursor inside block B alone snaps to 14.
    assert.equal(snapAfterCommentBlock(lines, 10), 14);
    // Cursor after both blocks is unchanged.
    assert.equal(snapAfterCommentBlock(lines, 14), 14);
  });

  it("caps at end of file when the cursor is inside a block that runs to EOF", () => {
    const lines = toLines(
      [
        "intro", // 1
        "```remargin", // 2
        "---", // 3
        "id: tail", // 4
        "---", // 5
        "body", // 6
        "```", // 7  — last line of file
      ].join("\n")
    );
    assert.equal(snapAfterCommentBlock(lines, 4), 7);
    assert.equal(snapAfterCommentBlock(lines, 7), 7);
  });

  it("handles unclosed blocks by snapping to the end of file", () => {
    const lines = toLines(
      [
        "intro", // 1
        "```remargin", // 2
        "---", // 3
        "id: oops", // 4
        "---", // 5
        "body with no closing fence", // 6
      ].join("\n")
    );
    assert.equal(snapAfterCommentBlock(lines, 4), 6);
  });

  it("does not confuse non-remargin code fences with remargin blocks", () => {
    const lines = toLines(
      [
        "intro", // 1
        "```ts", // 2  — not a remargin block
        "const x = 1;", // 3
        "```", // 4
        "middle", // 5
      ].join("\n")
    );
    assert.equal(snapAfterCommentBlock(lines, 3), 3);
    assert.equal(snapAfterCommentBlock(lines, 4), 4);
  });

  it("respects the fence depth of the opening line when matching the close", () => {
    const lines = toLines(
      [
        "intro", // 1
        "````remargin", // 2  — 4 backticks
        "---", // 3
        "id: nested", // 4
        "---", // 5
        "```", // 6  — 3-backtick fence inside the block, NOT a close
        "body", // 7
        "```", // 8  — still not a close (3 vs 4)
        "````", // 9  — actual close
        "tail", // 10
      ].join("\n")
    );
    // Cursor inside the block (line 7) snaps to the line after the 4-backtick
    // closing fence.
    assert.equal(snapAfterCommentBlock(lines, 7), 10);
  });

  it("returns the target unchanged when it is negative or zero", () => {
    const lines = toLines("a\nb\nc");
    assert.equal(snapAfterCommentBlock(lines, 0), 0);
    assert.equal(snapAfterCommentBlock(lines, -3), -3);
  });
});
