/** Finds a terminal emulator and launches the Submit shell line in it. */

import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";
import { spawn } from "child_process";
import { shellQuote } from "@/lib/submitCommand";

/** `[binary, ...exec-flag]` candidates, first found on PATH wins. */
export const LINUX_TERMINALS: string[][] = [
  ["ptyxis", "--"],
  ["gnome-terminal", "--"],
  ["konsole", "-e"],
  ["kitty", "-e"],
  ["alacritty", "-e"],
  ["xterm", "-e"],
];

const OSASCRIPT_PREFIX = ["osascript"];

function isOnPath(bin: string): boolean {
  const path = process.env["PATH"] ?? "";
  return path.split(delimiter).some((dir) => dir !== "" && existsSync(join(dir, bin)));
}

/**
 * Resolve the terminal argv prefix: `terminalCommand` (whitespace-split)
 * when set, otherwise per-OS detection. Returns `null` when nothing is
 * configured and nothing can be detected (e.g. Windows) — the caller
 * shows a Notice pointing at the Terminal command setting.
 */
export function resolveTerminal(
  terminalCommand: string,
  platform: NodeJS.Platform
): string[] | null {
  const configured = terminalCommand.trim();
  if (configured) return configured.split(/\s+/);
  if (platform === "darwin") return OSASCRIPT_PREFIX;
  if (platform === "linux") {
    for (const candidate of LINUX_TERMINALS) {
      const [bin] = candidate;
      if (bin && isOnPath(bin)) return [...candidate];
    }
  }
  return null;
}

/**
 * Build the `osascript` argv that opens Terminal.app running the shell
 * line. Terminal.app ignores spawn cwd, so the script cd's explicitly;
 * `\` and `"` are backslash-escaped for the AppleScript string literal.
 */
export function buildOsascriptArgs(shellLine: string, cwd: string): string[] {
  const script = `cd ${shellQuote(cwd)} && ${shellLine}`;
  const escaped = script.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
  return [
    "-e",
    `tell application "Terminal" to do script "${escaped}"`,
    "-e",
    'tell application "Terminal" to activate',
  ];
}

/** Spawn the terminal detached running `sh -c <shellLine>` in `cwd`. Never awaits. */
export function launchInTerminal(argvPrefix: string[], shellLine: string, cwd: string): void {
  if (argvPrefix.length === 1 && argvPrefix[0] === "osascript") {
    spawn("osascript", buildOsascriptArgs(shellLine, cwd), {
      detached: true,
      stdio: "ignore",
    }).unref();
    return;
  }
  const [bin, ...flags] = argvPrefix;
  if (!bin) return;
  spawn(bin, [...flags, "sh", "-c", shellLine], { cwd, detached: true, stdio: "ignore" }).unref();
}
