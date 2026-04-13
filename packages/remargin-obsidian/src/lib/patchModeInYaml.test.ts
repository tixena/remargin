import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { patchModeInYaml } from "./patchModeInYaml.ts";

describe("patchModeInYaml", () => {
  it("creates a minimal file when source is empty", () => {
    assert.equal(patchModeInYaml("", "strict"), "mode: strict\n");
  });

  it("appends mode when no existing key is present", () => {
    const source = ["identity: alice", "type: human", "key: /path/to/key"].join("\n") + "\n";
    const patched = patchModeInYaml(source, "open");
    assert.equal(
      patched,
      ["identity: alice", "type: human", "key: /path/to/key", "mode: open", ""].join("\n")
    );
  });

  it("adds a trailing newline before appending when source does not end with one", () => {
    const source = "identity: alice";
    const patched = patchModeInYaml(source, "strict");
    assert.equal(patched, "identity: alice\nmode: strict\n");
  });

  it("rewrites an existing top-level mode in place", () => {
    const source = ["identity: alice", "mode: open", "type: human"].join("\n") + "\n";
    const patched = patchModeInYaml(source, "strict");
    assert.equal(patched, ["identity: alice", "mode: strict", "type: human", ""].join("\n"));
  });

  it("preserves trailing comments on the mode line", () => {
    const source = "mode: open   # was open\n";
    const patched = patchModeInYaml(source, "strict");
    assert.equal(patched, "mode: strict   # was open\n");
  });

  it("ignores nested mode keys (leading whitespace)", () => {
    const source = ["outer:", "  mode: nested", "identity: alice"].join("\n") + "\n";
    const patched = patchModeInYaml(source, "strict");
    // Since no top-level mode exists, a new one is appended and the nested
    // one is left untouched.
    assert.equal(
      patched,
      ["outer:", "  mode: nested", "identity: alice", "mode: strict", ""].join("\n")
    );
  });

  it("preserves blank lines and comments in the file", () => {
    const source = [
      "# remargin vault config",
      "",
      "identity: alice",
      "mode: open",
      "",
      "type: human",
    ].join("\n") + "\n";
    const patched = patchModeInYaml(source, "registered");
    assert.equal(
      patched,
      [
        "# remargin vault config",
        "",
        "identity: alice",
        "mode: registered",
        "",
        "type: human",
        "",
      ].join("\n")
    );
  });

  it("only rewrites the first top-level mode key", () => {
    // If somehow the file has two top-level mode keys (invalid YAML but we
    // should be conservative), only the first one is rewritten.
    const source = "mode: open\nmode: strict\n";
    const patched = patchModeInYaml(source, "registered");
    assert.equal(patched, "mode: registered\nmode: strict\n");
  });
});
