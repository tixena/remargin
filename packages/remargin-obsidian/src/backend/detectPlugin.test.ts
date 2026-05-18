import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { parsePluginsListOutput } from "./detectPlugin.ts";

describe("parsePluginsListOutput", () => {
  it("returns absent when no remargin entry", () => {
    const out = `Installed plugins:

  ❯ beads@beads-marketplace
    Version: 0.49.0
    Scope: user
    Status: ✔ enabled
`;
    assert.deepStrictEqual(parsePluginsListOutput(out), { kind: "absent" });
  });

  it("returns installed_enabled when remargin is enabled", () => {
    const out = `Installed plugins:

  ❯ remargin@remargin
    Version: 0.1.0
    Scope: user
    Status: ✔ enabled
`;
    assert.deepStrictEqual(parsePluginsListOutput(out), {
      kind: "installed_enabled",
    });
  });

  it("returns installed_disabled when remargin is disabled", () => {
    const out = `Installed plugins:

  ❯ remargin@remargin
    Version: 0.1.0
    Scope: user
    Status: ✘ disabled
`;
    assert.deepStrictEqual(parsePluginsListOutput(out), {
      kind: "installed_disabled",
    });
  });

  it("handles enabled remargin alongside other plugins", () => {
    const out = `Installed plugins:

  ❯ beads@beads-marketplace
    Version: 0.49.0
    Status: ✔ enabled

  ❯ remargin@some-marketplace
    Version: 0.1.0
    Status: ✔ enabled

  ❯ other@x
    Status: ✘ disabled
`;
    assert.deepStrictEqual(parsePluginsListOutput(out), {
      kind: "installed_enabled",
    });
  });

  it("handles enabled status text without the unicode glyph", () => {
    const out = `  ❯ remargin@remargin\n    Status: enabled\n`;
    assert.deepStrictEqual(parsePluginsListOutput(out), {
      kind: "installed_enabled",
    });
  });

  it("treats absent status line as installed_disabled", () => {
    const out = `  ❯ remargin@remargin\n    Version: 0.1.0\n`;
    assert.deepStrictEqual(parsePluginsListOutput(out), {
      kind: "installed_disabled",
    });
  });

  it("does not match plugin names that merely contain remargin", () => {
    const out = `  ❯ remargin-extras@x\n    Status: ✔ enabled\n`;
    assert.deepStrictEqual(parsePluginsListOutput(out), { kind: "absent" });
  });
});
