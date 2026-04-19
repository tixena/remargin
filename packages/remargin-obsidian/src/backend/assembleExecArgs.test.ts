import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { assembleExecArgs } from "./assembleExecArgs.ts";

describe("assembleExecArgs", () => {
  it("places --config and --json AFTER the subcommand name", () => {
    const out = assembleExecArgs({
      args: ["sandbox", "add", "file.md"],
      identityArgs: ["--config", "/tmp/.remargin.yaml"],
      useJson: true,
      identityAccepted: true,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, [
      "sandbox",
      "--config",
      "/tmp/.remargin.yaml",
      "--json",
      "add",
      "file.md",
    ]);
  });

  it("places --identity/--type AFTER the subcommand name in manual mode", () => {
    const out = assembleExecArgs({
      args: ["comment", "note.md", "hello"],
      identityArgs: ["--identity", "alice", "--type", "human"],
      useJson: true,
      identityAccepted: true,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, [
      "comment",
      "--identity",
      "alice",
      "--type",
      "human",
      "--json",
      "note.md",
      "hello",
    ]);
  });

  it("omits identity flags entirely for read-only subcommands", () => {
    const out = assembleExecArgs({
      args: ["ls", "notes"],
      identityArgs: ["--config", "/tmp/.remargin.yaml"],
      useJson: true,
      identityAccepted: false,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, ["ls", "--json", "notes"]);
  });

  it("omits identity flags when skipIdentity is set (e.g. --version)", () => {
    const out = assembleExecArgs({
      args: ["--version"],
      identityArgs: ["--config", "/tmp/.remargin.yaml"],
      useJson: false,
      identityAccepted: false,
      skipIdentity: true,
    });
    assert.deepStrictEqual(out, ["--version"]);
  });

  it("drops --json when useJson is false", () => {
    const out = assembleExecArgs({
      args: ["edit", "note.md", "cm-123", "new content"],
      identityArgs: ["--config", "/tmp/.remargin.yaml"],
      useJson: false,
      identityAccepted: true,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, [
      "edit",
      "--config",
      "/tmp/.remargin.yaml",
      "note.md",
      "cm-123",
      "new content",
    ]);
  });

  it("preserves nested action args after the per-subcommand flags", () => {
    const out = assembleExecArgs({
      args: ["sandbox", "add", "a.md", "b.md", "c.md"],
      identityArgs: ["--identity", "alice", "--type", "human"],
      useJson: true,
      identityAccepted: true,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, [
      "sandbox",
      "--identity",
      "alice",
      "--type",
      "human",
      "--json",
      "add",
      "a.md",
      "b.md",
      "c.md",
    ]);
  });

  it("returns just the per-subcommand flags when args is empty", () => {
    const out = assembleExecArgs({
      args: [],
      identityArgs: ["--identity", "alice"],
      useJson: true,
      identityAccepted: true,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, ["--identity", "alice", "--json"]);
  });
});
