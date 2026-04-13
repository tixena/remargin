import { strict as assert } from "node:assert";
import { homedir } from "node:os";
import { describe, it } from "node:test";
import { expandPath } from "./expandPath.ts";

describe("expandPath", () => {
  const home = homedir();

  it("returns empty string for empty input", () => {
    assert.strictEqual(expandPath(""), "");
  });

  it("returns empty string for undefined or null input", () => {
    assert.strictEqual(expandPath(undefined), "");
    assert.strictEqual(expandPath(null), "");
  });

  it("returns empty string for whitespace-only input", () => {
    assert.strictEqual(expandPath("   "), "");
    assert.strictEqual(expandPath("\t\n"), "");
  });

  it("trims surrounding whitespace", () => {
    assert.strictEqual(expandPath("  /usr/bin/remargin  "), "/usr/bin/remargin");
  });

  it("leaves a plain absolute path untouched", () => {
    assert.strictEqual(expandPath("/usr/bin/remargin"), "/usr/bin/remargin");
  });

  it("leaves a plain relative path untouched", () => {
    assert.strictEqual(expandPath("bin/remargin"), "bin/remargin");
    assert.strictEqual(expandPath("remargin"), "remargin");
  });

  it("expands a bare ~", () => {
    assert.strictEqual(expandPath("~"), home);
  });

  it("expands ~/foo", () => {
    assert.strictEqual(expandPath("~/.cargo/bin/remargin"), `${home}/.cargo/bin/remargin`);
  });

  it("does not expand ~user forms", () => {
    // A tilde followed by a non-slash char is treated as a literal.
    assert.strictEqual(expandPath("~root/.cargo/bin"), "~root/.cargo/bin");
  });

  it("expands $HOME", () => {
    assert.strictEqual(expandPath("$HOME/.cargo/bin/remargin"), `${home}/.cargo/bin/remargin`);
  });

  it("expands ${HOME}", () => {
    assert.strictEqual(expandPath("${HOME}/.cargo/bin/remargin"), `${home}/.cargo/bin/remargin`);
  });

  it("expands $HOME only as a whole word", () => {
    // $HOMEFOO should not expand — matches $HOME word boundary.
    assert.strictEqual(expandPath("$HOMEFOO/bar"), "$HOMEFOO/bar");
  });

  it("expands %USERPROFILE% (Windows)", () => {
    assert.strictEqual(expandPath("%USERPROFILE%\\AppData\\foo"), `${home}\\AppData\\foo`);
  });

  it("expands %HOME% (Windows)", () => {
    assert.strictEqual(expandPath("%HOME%\\bin\\remargin.exe"), `${home}\\bin\\remargin.exe`);
  });

  it("is idempotent", () => {
    const cases = [
      "~",
      "~/.cargo/bin/remargin",
      "$HOME/.cargo/bin/remargin",
      "${HOME}/bin",
      "/usr/bin/remargin",
      "",
    ];
    for (const input of cases) {
      const once = expandPath(input);
      const twice = expandPath(once);
      assert.strictEqual(twice, once, `not idempotent for ${JSON.stringify(input)}`);
    }
  });
});
