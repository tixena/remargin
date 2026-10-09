/** Tests for terminal resolution and the osascript launch arguments. */

import { strict as assert } from "node:assert";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
import { buildOsascriptArgs, LINUX_TERMINALS, resolveTerminal } from "./terminalLauncher.ts";

describe("resolveTerminal — configured command", () => {
  it("whitespace-splits the setting into an argv prefix", () => {
    assert.deepEqual(resolveTerminal("kitty -e", "linux"), ["kitty", "-e"]);
    assert.deepEqual(resolveTerminal("  ptyxis   --  ", "win32"), ["ptyxis", "--"]);
  });

  it("wins over platform detection on every platform", () => {
    assert.deepEqual(resolveTerminal("wt.exe", "win32"), ["wt.exe"]);
    assert.deepEqual(resolveTerminal("iterm2-run", "darwin"), ["iterm2-run"]);
  });
});

describe("resolveTerminal — auto-detection", () => {
  const savedPath = process.env["PATH"];

  beforeEach(() => {
    process.env["PATH"] = savedPath;
  });

  afterEach(() => {
    process.env["PATH"] = savedPath;
  });

  it("returns null on win32 with an empty setting", () => {
    assert.equal(resolveTerminal("", "win32"), null);
  });

  it("returns the osascript sentinel on darwin with an empty setting", () => {
    assert.deepEqual(resolveTerminal("", "darwin"), ["osascript"]);
  });

  it("picks the first candidate found on PATH on linux", () => {
    const dir = mkdtempSync(join(tmpdir(), "remargin-term-"));
    writeFileSync(join(dir, "konsole"), "");
    writeFileSync(join(dir, "xterm"), "");
    process.env["PATH"] = dir;
    assert.deepEqual(resolveTerminal("", "linux"), ["konsole", "-e"]);
  });

  it("returns null on linux when no candidate is on PATH", () => {
    process.env["PATH"] = mkdtempSync(join(tmpdir(), "remargin-term-empty-"));
    assert.equal(resolveTerminal("", "linux"), null);
  });

  it("candidate table starts with ptyxis and ends with xterm", () => {
    assert.deepEqual(LINUX_TERMINALS[0], ["ptyxis", "--"]);
    assert.deepEqual(LINUX_TERMINALS[LINUX_TERMINALS.length - 1], ["xterm", "-e"]);
  });
});

describe("buildOsascriptArgs", () => {
  it("cd's into the cwd and backslash-escapes quotes for AppleScript", () => {
    const args = buildOsascriptArgs("cat '/tmp/p.md' | 'claude' -p", "/Users/me/vault");
    assert.equal(args.length, 4);
    assert.equal(args[0], "-e");
    assert.equal(
      args[1],
      `tell application "Terminal" to do script "cd '/Users/me/vault' && cat '/tmp/p.md' | 'claude' -p"`
    );
    assert.equal(args[2], "-e");
    assert.equal(args[3], 'tell application "Terminal" to activate');
  });

  it("escapes embedded double quotes and backslashes", () => {
    const args = buildOsascriptArgs('echo "a\\b"', "/v");
    assert.ok(args[1]?.includes('echo \\"a\\\\b\\"'), args[1]);
  });
});
