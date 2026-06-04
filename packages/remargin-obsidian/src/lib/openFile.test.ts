import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { TFile } from "obsidian";
import { openFileAtLine } from "./openFile.ts";

// ---------------------------------------------------------------------------
// Minimal plugin-shaped mock
// ---------------------------------------------------------------------------

/**
 * Build a minimal plugin stub.
 *
 * `resolvesExact` is the vault-root-relative path that the vault mock treats
 * as resolvable.  Pass `undefined` to make every path unresolvable.
 *
 * The leaf mock records the `TFile` passed to each `openFile` call so tests
 * can assert whether the file-open step was reached.
 */
function makePlugin(resolvesExact: string | undefined): {
  plugin: object;
  openFileCalls: TFile[];
  lastLookupPath: () => string | undefined;
} {
  const fakeFile = new TFile();
  const openFileCalls: TFile[] = [];
  let lastLookup: string | undefined;

  const leaf = {
    view: {},
    openFile(f: TFile) {
      openFileCalls.push(f);
      return Promise.resolve();
    },
  };

  const plugin = {
    app: {
      vault: {
        getAbstractFileByPath(path: string) {
          lastLookup = path;
          return resolvesExact !== undefined && path === resolvesExact ? fakeFile : null;
        },
      },
      workspace: {
        getLeavesOfType(_type: string) {
          return [];
        },
        getLeaf(_newLeaf: boolean) {
          return leaf;
        },
      },
    },
    getLastMarkdownView() {
      return null;
    },
  };

  return { plugin, openFileCalls, lastLookupPath: () => lastLookup };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("openFileAtLine", () => {
  // ── Unresolvable path: early-return / Notice branch ──────────────────────

  it("returns without throwing when the path is unresolvable", async () => {
    const { plugin } = makePlugin(undefined);

    await assert.doesNotReject(async () => {
      await openFileAtLine(plugin as never, "vault/gone.md");
    });
  });

  it("does not call leaf.openFile when the path is unresolvable", async () => {
    const { plugin, openFileCalls } = makePlugin(undefined);

    await openFileAtLine(plugin as never, "vault/gone.md");

    assert.strictEqual(
      openFileCalls.length,
      0,
      "early return must prevent openFile from being called"
    );
  });

  // ── Resolvable path: normal open path ────────────────────────────────────

  it("calls leaf.openFile exactly once for a resolvable vault-relative path", async () => {
    const { plugin, openFileCalls } = makePlugin("notes/doc.md");

    await openFileAtLine(plugin as never, "notes/doc.md");

    assert.strictEqual(openFileCalls.length, 1, "openFile should be called exactly once");
  });

  // ── normalizePath: paths are normalised before vault lookup ──────────────

  it("strips a leading ./ so the vault lookup matches the canonical path", async () => {
    const { plugin, openFileCalls, lastLookupPath } = makePlugin("notes/doc.md");

    await openFileAtLine(plugin as never, "./notes/doc.md");

    assert.strictEqual(
      lastLookupPath(),
      "notes/doc.md",
      "normalizePath should have stripped the leading ./"
    );
    assert.strictEqual(openFileCalls.length, 1, "file should have been opened after normalisation");
  });

  it("converts backslash separators to forward slashes", async () => {
    const { plugin, openFileCalls, lastLookupPath } = makePlugin("notes/sub/doc.md");

    await openFileAtLine(plugin as never, "notes\\sub\\doc.md");

    assert.strictEqual(
      lastLookupPath(),
      "notes/sub/doc.md",
      "normalizePath should have converted backslashes"
    );
    assert.strictEqual(openFileCalls.length, 1, "file should have been opened after normalisation");
  });

  // ── Isolation: each plugin instance has an independent call tracker ──────

  it("call counters are independent across plugin instances", async () => {
    const { plugin: gone, openFileCalls: callsGone } = makePlugin(undefined);
    const { plugin: exists, openFileCalls: callsExists } = makePlugin("vault/exists.md");

    await openFileAtLine(gone as never, "vault/gone.md");
    await openFileAtLine(exists as never, "vault/exists.md");

    assert.strictEqual(callsGone.length, 0, "unresolvable: openFile not called");
    assert.strictEqual(callsExists.length, 1, "resolvable: openFile called once");
  });
});
