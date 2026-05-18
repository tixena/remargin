import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { assembleExecArgs } from "./assembleExecArgs.ts";
import { IDENTITY_ACCEPTING_SUBCOMMANDS } from "./identityAcceptingSubcommands.ts";

/**
 * Pins `remargin identity` as identity-accepting so the plugin's
 * read-path for `me` resolves under the same flags the write-path
 * uses. Drift here flips the ack UI branch in the threaded view.
 */
describe("identity subcommand is identity-accepting", () => {
  it("IDENTITY_ACCEPTING_SUBCOMMANDS contains 'identity'", () => {
    assert.ok(
      IDENTITY_ACCEPTING_SUBCOMMANDS.has("identity"),
      "identity must be in the set so assembleExecArgs forwards --config"
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
    // Settings-driven identity flags land in the per-subcommand slot;
    // the caller's extra --type follows in the trailing args slot.
    // The CLI's clap layer will reject the combination because
    // --config conflicts with --type, which is exactly the belt-and-
    // braces the three-branch resolver relies on.
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
