import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { extractTitle } from "./file-title.ts";

describe("extractTitle", () => {
  it("returns the filename stem when the file is empty", () => {
    assert.strictEqual(extractTitle("", "notes.md"), "notes");
  });

  it("returns the filename stem when no H1 or title field exists", () => {
    const body = "just some body text without any heading.";
    assert.strictEqual(extractTitle(body, "diary.md"), "diary");
  });

  it("returns the first H1 heading when present", () => {
    const body = "# Project Overview\n\nSome intro.\n\n## Not H1\n";
    assert.strictEqual(extractTitle(body, "overview.md"), "Project Overview");
  });

  it("skips YAML frontmatter before searching for an H1", () => {
    const body = "---\ntags:\n- a\n---\n# Real Title\n";
    assert.strictEqual(extractTitle(body, "x.md"), "Real Title");
  });

  it("trims trailing whitespace and inline hashes on the H1 line", () => {
    const body = "#   Padded Title   \n\nbody";
    assert.strictEqual(extractTitle(body, "f.md"), "Padded Title");
  });

  it("does not treat an H2 as a title", () => {
    const body = "## Second-level only\n\nbody";
    assert.strictEqual(extractTitle(body, "sub.md"), "sub");
  });

  it("falls back to a frontmatter title: field when no H1 exists", () => {
    const body = "---\ntitle: From Frontmatter\n---\n\nplain body";
    assert.strictEqual(extractTitle(body, "x.md"), "From Frontmatter");
  });

  it("unwraps single-quoted frontmatter titles", () => {
    const body = "---\ntitle: 'Quoted Title'\n---\nbody";
    assert.strictEqual(extractTitle(body, "x.md"), "Quoted Title");
  });

  it("unwraps double-quoted frontmatter titles", () => {
    const body = '---\ntitle: "Double Quoted"\n---\nbody';
    assert.strictEqual(extractTitle(body, "x.md"), "Double Quoted");
  });

  it("prefers an H1 over a frontmatter title when both exist", () => {
    const body = "---\ntitle: FM\n---\n\n# H1 Wins\n\nbody";
    assert.strictEqual(extractTitle(body, "x.md"), "H1 Wins");
  });

  it("handles filenames without an extension", () => {
    assert.strictEqual(extractTitle("", "plain"), "plain");
  });

  it("handles nested paths in the filename fallback", () => {
    assert.strictEqual(extractTitle("", "a/b/c.md"), "c");
  });
});
