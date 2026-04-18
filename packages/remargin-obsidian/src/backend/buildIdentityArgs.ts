import { expandPath } from "@/lib/expandPath";
import type { RemarginSettings } from "@/types";

/**
 * Build the identity-override flags prepended to every mutating CLI
 * call.
 *
 * Config mode is the source-of-truth contract: when a config file is
 * selected, emit ONLY `--config <path>` and let the CLI resolve
 * identity, type, and key from that one file. Forwarding `--identity`,
 * `--type`, or `--key` here silently overrides whatever the user's
 * YAML says — which is how nine `eduardo-burgos` comments landed
 * unsigned in strict mode under a config scoped to a different author
 * type (rem-ce4).
 *
 * Manual mode is the escape hatch for users without a config file:
 * emit `--identity` and `--type`, but never `--key`. If a user truly
 * runs without a config file, they should create one — the CLI
 * resolves signing keys from there, not from plugin settings.
 *
 * Exported as a pure module-level function so unit tests can exercise
 * it directly without spinning up a `RemarginBackend` (which takes a
 * vault path and settings-change callbacks the tests do not care
 * about).
 */
export function buildIdentityArgs(settings: RemarginSettings): string[] {
  if (settings.identityMode === "config" && settings.configFilePath) {
    return ["--config", expandPath(settings.configFilePath)];
  }
  const args: string[] = [];
  if (settings.authorName) {
    args.push("--identity", settings.authorName);
  }
  args.push("--type", "human");
  return args;
}
