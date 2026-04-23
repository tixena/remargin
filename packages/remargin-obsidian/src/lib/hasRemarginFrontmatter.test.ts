import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { hasRemarginFrontmatter } from "./hasRemarginFrontmatter.ts";

describe("hasRemarginFrontmatter", () => {
  it("returns false for an empty document", () => {
    assert.strictEqual(hasRemarginFrontmatter(""), false);
  });

  it("returns false when there is no frontmatter", () => {
    assert.strictEqual(hasRemarginFrontmatter("# Hello\n\nBody.\n"), false);
  });

  it("returns false for non-remargin frontmatter", () => {
    const doc = "---\ntitle: Draft\ntags: [notes]\n---\n\n# Hello\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), false);
  });

  it("returns true when remargin_pending is present", () => {
    const doc = "---\ntitle: Draft\nremargin_pending: 0\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });

  it("returns true when remargin_total is present", () => {
    const doc = "---\nremargin_total: 3\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });

  it("returns true when remargin_last_activity is present", () => {
    const doc = "---\nremargin_last_activity: 2026-04-06T12:00:00Z\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });

  it("returns true when any remargin_* field is present alongside others", () => {
    const doc = "---\ntitle: Draft\nauthor: alice\nremargin_pending_for:\n  - bob\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });

  it("returns false for an unclosed frontmatter block", () => {
    const doc = "---\nremargin_pending: 0\n\n# Body without closing fence\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), false);
  });

  it("returns false for an empty frontmatter block", () => {
    assert.strictEqual(hasRemarginFrontmatter("---\n---\n# Hello\n"), false);
  });

  it("tolerates CRLF line endings", () => {
    const doc = "---\r\nremargin_pending: 0\r\n---\r\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });

  it("tolerates a leading UTF-8 BOM", () => {
    const doc = "\u{FEFF}---\nremargin_pending: 0\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });

  it("is case-sensitive: only lowercase remargin_ is a hit", () => {
    // The CLI only emits lowercase `remargin_*`; uppercase is a user
    // field, not a managed one.
    const doc = "---\nRemargin_Pending: 0\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), false);
  });

  it("tolerates indented keys", () => {
    const doc = "---\n  remargin_pending: 0\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });

  it("does not match bare `remargin:` (no underscore)", () => {
    const doc = "---\nremargin: true\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), false);
  });

  it("does not get fooled by `---` inside a value", () => {
    const doc = "---\ntitle: before --- after\nremargin_pending: 0\n---\n";
    assert.strictEqual(hasRemarginFrontmatter(doc), true);
  });
});
