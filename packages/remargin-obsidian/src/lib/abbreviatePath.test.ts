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
    // "src/01_personal/remargin/ui" is 27 chars
    // After abbreviating "src" -> "s": "s/01_personal/remargin/ui" = 25
    // After abbreviating "01_personal" -> "0": "s/0/remargin/ui" = 15
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
    // Segments that are already 1 char should not be abbreviated further
    const result = abbreviatePath("a/b/c/deep", 5);
    assert.strictEqual(result, "a/b/c/d");
  });

  it("stops abbreviating once the path fits", () => {
    // "docs/guide/reference" = 20 chars
    // Abbreviating "docs" -> "d": "d/guide/reference" = 17 chars
    // 17 <= 18, so it stops
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
