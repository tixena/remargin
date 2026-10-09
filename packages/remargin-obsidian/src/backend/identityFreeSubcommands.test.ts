/** Tests for the gate deciding which subcommands receive the identity flags. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { DEFAULT_SETTINGS, type RemarginSettings } from "@/types";
import { assembleExecArgs } from "./assembleExecArgs.ts";
import { buildIdentityArgs } from "./buildIdentityArgs.ts";
import { buildQueryArgs } from "./buildQueryArgs.ts";
import { acceptsIdentity, IDENTITY_FREE_SUBCOMMANDS } from "./identityFreeSubcommands.ts";

const IDENTITY_RELEVANT_SUBCOMMANDS = [
  "ack",
  "batch",
  "comment",
  "comments",
  "delete",
  "edit",
  "get",
  "identity",
  "ls",
  "prompt",
  "query",
  "react",
  "rm",
  "sandbox",
  "search",
  "write",
];

function settingsWith(overrides: Partial<RemarginSettings>): RemarginSettings {
  return { ...DEFAULT_SETTINGS, ...overrides };
}

describe("identity forwarding gate", () => {
  it("exception list holds exactly the identity-free machine/tree reads", () => {
    assert.deepStrictEqual([...IDENTITY_FREE_SUBCOMMANDS].sort(), [
      "obsidian",
      "registry",
      "resolve-mode",
    ]);
  });

  it("forwards identity for every identity-relevant subcommand", () => {
    for (const subcommand of IDENTITY_RELEVANT_SUBCOMMANDS) {
      assert.ok(acceptsIdentity(subcommand), `${subcommand} must accept identity flags`);
    }
  });

  it("withholds identity for the exception entries", () => {
    for (const subcommand of IDENTITY_FREE_SUBCOMMANDS) {
      assert.ok(!acceptsIdentity(subcommand), `${subcommand} must stay identity-free`);
    }
  });

  it("withholds identity for bare-flag probes like --version", () => {
    assert.ok(!acceptsIdentity("--version"));
    assert.ok(!acceptsIdentity(undefined));
  });

  it("forwards identity for subcommands unknown to the plugin (loud-failure contract)", () => {
    // A subcommand missing from the list gets identity flags, so a rejection is a visible error.
    assert.ok(acceptsIdentity("some-future-subcommand"));
  });
});

describe("pending-for-me regression", () => {
  it("query with pendingForMe carries --config in config mode", () => {
    const args = buildQueryArgs(".", { pendingForMe: true, expanded: true });
    const out = assembleExecArgs({
      args,
      identityArgs: buildIdentityArgs(
        settingsWith({
          identityMode: "config",
          configFilePath: "/home/eduardo/.remargin.yaml",
        })
      ),
      useJson: true,
      identityAccepted: acceptsIdentity(args[0]),
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, [
      "query",
      "--config",
      "/home/eduardo/.remargin.yaml",
      "--json",
      ".",
      "--pending-for-me",
      "--expanded",
    ]);
  });

  it("query with pendingForMe carries --identity in manual mode", () => {
    const args = buildQueryArgs(".", { pendingForMe: true });
    const out = assembleExecArgs({
      args,
      identityArgs: buildIdentityArgs(
        settingsWith({ identityMode: "manual", authorName: "eduardo-burgos" })
      ),
      useJson: true,
      identityAccepted: acceptsIdentity(args[0]),
      skipIdentity: false,
    });
    assert.deepStrictEqual(out, [
      "query",
      "--identity",
      "eduardo-burgos",
      "--type",
      "human",
      "--json",
      ".",
      "--pending-for-me",
    ]);
  });
});
