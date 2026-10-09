/** Tests for directory path abbreviation. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { abbreviatePath } from "./abbreviatePath.ts";

describe("abbreviatePath", () => {
  it("returns empty string for empty input", () => {
    assert.strictEqual(abbreviatePath("", 10), "");
  });

  it("returns the full path when it fits within maxChars", () => {
    assert.strictEqual(abbreviatePath("src/ui", 100), "src/ui");
  });

  it("abbreviates leftmost segments first", () => {
    // 27 chars; "src" -> "s" leaves 25, then "01_personal" -> "0" leaves "s/0/remargin/ui" at 15.
    const result = abbreviatePath("src/01_personal/remargin/ui", 20);
    assert.strictEqual(result, "s/0/remargin/ui");
  });

  it("abbreviates all segments when maxChars is very small", () => {
    const result = abbreviatePath("src/components/sidebar", 5);
    assert.strictEqual(result, "s/c/s");
  });

  it("handles single segment", () => {
    assert.strictEqual(abbreviatePath("src", 100), "src");
    assert.strictEqual(abbreviatePath("src", 1), "s");
  });

  it("handles already-short segments", () => {
    const result = abbreviatePath("a/b/c/deep", 5);
    assert.strictEqual(result, "a/b/c/d");
  });

  it("stops abbreviating once the path fits", () => {
    // 20 chars; "docs" -> "d" leaves "d/guide/reference" at 17, which fits in 18, so it stops.
    const result = abbreviatePath("docs/guide/reference", 18);
    assert.strictEqual(result, "d/guide/reference");
  });

  it("preserves rightmost segments as long as possible", () => {
    const result = abbreviatePath("packages/remargin-obsidian/src/components", 30);
    // "packages" -> "p": "p/remargin-obsidian/src/components" = 34
    // "remargin-obsidian" -> "r": "p/r/src/components" = 18, fits in 30
    assert.strictEqual(result, "p/r/src/components");
  });
});
