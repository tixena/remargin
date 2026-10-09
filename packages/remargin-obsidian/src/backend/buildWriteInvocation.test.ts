/** Tests for the argv and stdin of a `remargin write`. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { buildWriteInvocation } from "./buildWriteInvocation.ts";

describe("buildWriteInvocation", () => {
  // A body starting with a `---` fence would be read by clap as a flag, so it must ride stdin.
  it("routes a frontmatter-leading body to stdin, never argv", () => {
    const body = '---\ntop_of_mind: "true"\narea: personal\n---\n# STACK\n';
    const { args, stdin } = buildWriteInvocation("STACK.md", body);
    assert.equal(stdin, body);
    assert.deepStrictEqual(args, ["write", "STACK.md"]);
    assert.ok(!args.includes(body), "body must never appear as a CLI arg");
  });

  it("keeps any `-`-leading body off argv", () => {
    const body = "--not-a-flag but a note body";
    const { args, stdin } = buildWriteInvocation("note.md", body);
    assert.equal(stdin, body);
    assert.deepStrictEqual(args, ["write", "note.md"]);
  });

  it("appends --create and --raw flags after the path", () => {
    const { args, stdin } = buildWriteInvocation("new.md", "hi", {
      create: true,
      raw: true,
    });
    assert.deepStrictEqual(args, ["write", "new.md", "--create", "--raw"]);
    assert.equal(stdin, "hi");
  });

  it("omits flags when opts is absent", () => {
    const { args } = buildWriteInvocation("f.md", "x");
    assert.deepStrictEqual(args, ["write", "f.md"]);
  });
});
