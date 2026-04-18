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

describe("buildIdentityArgs (rem-ce4)", () => {
  // ---- Config mode: forward ONLY --config ----

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
    // Regression guard for the original rem-ce4 bug: the plugin used to
    // forward --type on every CLI call even in config mode, silently
    // overriding the YAML's type: field. That combination prevented the
    // CLI from resolving a type-scoped signing key.
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

  // ---- Manual mode: --identity + --type, never --key ----

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
    // The plugin deprecated the keyFilePath setting (see SettingsTab for
    // the UI hint). Any future commit that reintroduces a --key
    // forwarding in manual mode will fail this test.
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

  // ---- Fallback when config mode is selected but path is empty ----

  it("config mode with empty configFilePath falls back to manual", () => {
    // If the user selects "Config file" but hasn't typed a path yet, we
    // must not emit `--config ""` (which would mean "I have no config
    // file" to the CLI in an ambiguous way). Falling back to the manual
    // identity args keeps operations working with whatever author name
    // is set.
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
