/** Tests for the read token. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { readToken } from "./readToken.ts";

describe("readToken", () => {
  it("matches itself for an identical read", () => {
    assert.equal(readToken(3, "notes/a.md"), readToken(3, "notes/a.md"));
  });

  it("changes with the generation, so a sidebar mutation supersedes the in-flight read", () => {
    assert.notEqual(readToken(1, "notes/a.md"), readToken(2, "notes/a.md"));
  });

  it("changes with the subject, so a file switch supersedes the previous file's read", () => {
    assert.notEqual(readToken(0, "notes/a.md"), readToken(0, "notes/b.md"));
  });

  it("separates a section's own counter from the sidebar's", () => {
    assert.notEqual(readToken(1, 0), readToken(0, 1));
  });

  it("keeps parts apart, so a subject cannot forge another read's token", () => {
    assert.notEqual(readToken(0, "a", "b"), readToken(0, "a,b"));
    assert.notEqual(readToken(0, 'a","b'), readToken(0, "a", "b"));
  });
});
