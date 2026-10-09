/** Tests that `remargin identity` receives the identity flags. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { assembleExecArgs } from "./assembleExecArgs.ts";
import { acceptsIdentity } from "./identityFreeSubcommands.ts";

// The read path for `me` must resolve under the same flags the write path uses.
describe("identity subcommand is identity-accepting", () => {
  it("the exec gate forwards identity for 'identity'", () => {
    assert.ok(
      acceptsIdentity("identity"),
      "identity must not be identity-free so assembleExecArgs forwards --config"
    );
  });

  it("assembleExecArgs forwards --config to `identity` when identityAccepted is true", () => {
    const out = assembleExecArgs({
      args: ["identity"],
      identityArgs: ["--config", "/home/alice/.remargin.yaml"],
      useJson: true,
      identityAccepted: true,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, ["identity", "--config", "/home/alice/.remargin.yaml", "--json"]);
  });

  it("assembleExecArgs forwards --identity/--type to `identity` in manual mode", () => {
    const out = assembleExecArgs({
      args: ["identity"],
      identityArgs: ["--identity", "alice", "--type", "human"],
      useJson: true,
      identityAccepted: true,
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, ["identity", "--identity", "alice", "--type", "human", "--json"]);
  });

  it("assembleExecArgs preserves an explicit --type passed alongside identity subcommand args", () => {
    const out = assembleExecArgs({
      args: ["identity", "--type", "agent"],
      identityArgs: ["--config", "/home/alice/.remargin.yaml"],
      useJson: true,
      identityAccepted: true,
      skipIdentity: false,
    });
    // Settings-driven flags fill the per-subcommand slot; the caller's `--type` trails them.
    assert.deepStrictEqual(out, [
      "identity",
      "--config",
      "/home/alice/.remargin.yaml",
      "--json",
      "--type",
      "agent",
    ]);
  });
});
