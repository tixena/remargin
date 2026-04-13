import { homedir } from "node:os";

/**
 * Expand home-directory references in a user-provided path so the same
 * setting string works across machines with different usernames / $HOME
 * values (e.g. macOS /Users/alice vs. Linux /home/alice).
 *
 * Handles:
 *   - Leading `~` or `~/...` (but not `~user` — POSIX user-home syntax is
 *     out of scope).
 *   - `$HOME` and `${HOME}`.
 *   - Windows `%USERPROFILE%` and `%HOME%`.
 *
 * Returns the input unchanged when empty or when no expansion applies. The
 * function is idempotent: running it twice on its own output yields the
 * same result, because the expanded home directory no longer contains any
 * tilde or env-var markers.
 */
export function expandPath(input: string | undefined | null): string {
  if (!input) return "";
  const trimmed = input.trim();
  if (!trimmed) return "";

  const home = homedir();
  let out = trimmed;

  // Leading `~` or `~/...` — but not `~user`.
  if (out === "~") {
    out = home;
  } else if (out.startsWith("~/") || out.startsWith("~\\")) {
    out = home + out.slice(1);
  }

  // $HOME and ${HOME}. Use a regex so it works mid-path too.
  out = out.replace(/\$\{HOME\}/g, home);
  out = out.replace(/\$HOME\b/g, home);

  // Windows-style %USERPROFILE% / %HOME%.
  out = out.replace(/%USERPROFILE%/gi, home);
  out = out.replace(/%HOME%/gi, home);

  return out;
}
