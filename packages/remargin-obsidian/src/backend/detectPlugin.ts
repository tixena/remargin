import type { PluginPresence } from "./types.ts";

const PLUGIN_NAME = "remargin";

export function parsePluginsListOutput(output: string): PluginPresence {
  const lines = output.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line) continue;
    const match = /^[^a-zA-Z@]*([a-zA-Z0-9_-]+)@[a-zA-Z0-9_-]+/.exec(line);
    if (!match) continue;
    if (match[1] !== PLUGIN_NAME) continue;
    for (let j = i + 1; j < lines.length && j < i + 8; j++) {
      const inner = lines[j];
      if (!inner) continue;
      const statusMatch = /Status:\s*(?:[^\sa-zA-Z]+\s*)?(enabled|disabled)/i.exec(inner);
      if (statusMatch) {
        return statusMatch[1].toLowerCase() === "enabled"
          ? { kind: "installed_enabled" }
          : { kind: "installed_disabled" };
      }
      if (/^[^a-zA-Z@]*[a-zA-Z0-9_-]+@[a-zA-Z0-9_-]+/.test(inner)) {
        break;
      }
    }
    return { kind: "installed_disabled" };
  }
  return { kind: "absent" };
}
