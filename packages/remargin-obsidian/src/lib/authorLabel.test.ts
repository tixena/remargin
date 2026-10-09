/** Tests for the author label and its tooltip title. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { authorLabel } from "./authorLabel.ts";

describe("authorLabel", () => {
  it("returns display name and id tooltip when they differ", () => {
    const resolve = (id: string) => (id === "eduardo-burgos" ? "Eduardo Burgos Minier" : id);
    const result = authorLabel("eduardo-burgos", resolve);
    assert.deepStrictEqual(result, {
      label: "Eduardo Burgos Minier",
      title: "eduardo-burgos",
    });
  });

  it("returns id as label with no tooltip when display equals id (no display_name registered)", () => {
    // When the registry has no entry the label is the id, and a tooltip repeating it is redundant.
    const identity = (id: string) => id;
    const result = authorLabel("ci-bot", identity);
    assert.deepStrictEqual(result, {
      label: "ci-bot",
      title: undefined,
    });
  });

  it("returns id as label with no tooltip for an unknown id (fallback path)", () => {
    const empty = (id: string) => id;
    const result = authorLabel("stranger", empty);
    assert.deepStrictEqual(result, {
      label: "stranger",
      title: undefined,
    });
  });

  it("exposes the id as tooltip even when display name is a minor variant", () => {
    const resolve = (id: string) => (id === "alice" ? "alice smith" : id);
    const result = authorLabel("alice", resolve);
    assert.strictEqual(result.label, "alice smith");
    assert.strictEqual(result.title, "alice");
  });
});
