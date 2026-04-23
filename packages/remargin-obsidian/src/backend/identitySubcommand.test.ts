import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { assembleExecArgs } from "./assembleExecArgs.ts";
import { IDENTITY_ACCEPTING_SUBCOMMANDS } from "./identityAcceptingSubcommands.ts";

/**
 * rem-3dw0: lock in that `remargin identity` is treated as
 * identity-accepting by the plugin.
 *
 * Before this change `backend.identity()` explicitly passed
 * `skipIdentity: true` and `"identity"` was absent from
 * `IDENTITY_ACCEPTING_SUBCOMMANDS`. Both stripped the `--config` (or
 * `--identity`/`--type`) flags built from plugin settings, so the
 * CLI walked up from cwd and returned whichever `.remargin.yaml` it
 * happened to find first. That produced a `me` that disagreed with
 * the identity every mutating op was writing under, which flipped
 * the ack UI branch (AckToggle vs AckButton) in the threaded view.
 *
 * These tests pin both halves of the fix so a future regression
 * trips on the structure before it reaches the UI.
 */
describe("identity subcommand is identity-accepting (rem-3dw0)", () => {
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
