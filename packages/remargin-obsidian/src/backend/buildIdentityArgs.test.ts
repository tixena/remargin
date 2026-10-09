/** Tests for the identity flags forwarded to the CLI in config and manual modes. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { DEFAULT_SETTINGS, type RemarginSettings } from "@/types";
import { buildIdentityArgs } from "./buildIdentityArgs.ts";

/**
 * Base settings helper so each test only spells out the fields it cares
 * about. Spread on top of DEFAULT_SETTINGS to inherit path defaults.
 */
function settingsWith(overrides: Partial<RemarginSettings>): RemarginSettings {
  return { ...DEFAULT_SETTINGS, ...overrides };
}

describe("buildIdentityArgs", () => {
  it("config mode with a config file path emits only --config", () => {
    const args = buildIdentityArgs(
      settingsWith({
        identityMode: "config",
        configFilePath: "/home/eduardo/.remargin.yaml",
        authorName: "ignored-when-config-set",
        keyFilePath: "/home/eduardo/.ssh/id_ed25519",
      })
    );
    assert.deepStrictEqual(args, ["--config", "/home/eduardo/.remargin.yaml"]);
  });

  it("config mode expands ~ in the config file path", () => {
    const args = buildIdentityArgs(
      settingsWith({
        identityMode: "config",
        configFilePath: "~/.remargin.yaml",
      })
    );
    assert.strictEqual(args[0], "--config");
    assert.strictEqual(args.length, 2);
    assert.ok(args[1]?.endsWith("/.remargin.yaml"));
    assert.ok(!args[1]?.startsWith("~"), "path must be expanded, not literal ~/");
  });

  it("config mode never emits --identity, --type, or --key", () => {
    const args = buildIdentityArgs(
      settingsWith({
        identityMode: "config",
        configFilePath: "/tmp/.remargin.yaml",
        authorName: "eduardo-burgos",
        keyFilePath: "/tmp/key",
      })
    );
    for (const flag of ["--identity", "--type", "--key"]) {
      assert.ok(
        !args.includes(flag),
        `config mode must not forward ${flag}; got ${JSON.stringify(args)}`
      );
    }
  });

  it("manual mode with an author emits --identity and --type human", () => {
    const args = buildIdentityArgs(
      settingsWith({
        identityMode: "manual",
        authorName: "alice",
      })
    );
    assert.deepStrictEqual(args, ["--identity", "alice", "--type", "human"]);
  });

  it("manual mode without an author emits only --type human", () => {
    const args = buildIdentityArgs(
      settingsWith({
        identityMode: "manual",
        authorName: "",
      })
    );
    assert.deepStrictEqual(args, ["--type", "human"]);
  });

  it("manual mode never forwards --key, even when keyFilePath is set", () => {
    const args = buildIdentityArgs(
      settingsWith({
        identityMode: "manual",
        authorName: "alice",
        keyFilePath: "/home/alice/.ssh/id_ed25519",
      })
    );
    assert.ok(
      !args.includes("--key"),
      `manual mode must never forward --key; got ${JSON.stringify(args)}`
    );
  });

  it("config mode with empty configFilePath falls back to manual", () => {
    // `--config ""` would be ambiguous to the CLI, so an empty path falls back to the manual args.
    const args = buildIdentityArgs(
      settingsWith({
        identityMode: "config",
        configFilePath: "",
        authorName: "alice",
      })
    );
    assert.deepStrictEqual(args, ["--identity", "alice", "--type", "human"]);
  });
});
