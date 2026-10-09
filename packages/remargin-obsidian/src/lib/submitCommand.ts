/**
 * Pure composition helpers for the sandbox Submit flow. The plugin
 * writes each group's composed prompt to a temp file, then launches a
 * terminal running one `cat <promptfile> | <runner>` per group,
 * chained with `;` (sequential, continue-on-failure).
 */

/** Wrap `s` in single quotes, escaping embedded single quotes. */
export function shellQuote(s: string): string {
  return `'${s.replace(/'/g, "'\\''")}'`;
}

/** Default runner used when the resolved prompt has no `runner:`. */
export function defaultRunner(claudePath: string, remarginPath: string): string {
  const claude = claudePath.trim() || "claude";
  const mcpConfig = JSON.stringify({
    mcpServers: { remargin: { command: remarginPath.trim() || "remargin", args: ["mcp", "run"] } },
  });
  return `${shellQuote(claude)} -p --permission-mode auto --allowedTools "mcp__remargin__*" --mcp-config ${shellQuote(mcpConfig)}`;
}

/**
 * Inline prompt body: resolved prompt + file list. Sandbox markers are
 * NOT the agent's to clear — they belong to the submitter's identity
 * (sandbox state is per-identity), so the submit shell line removes
 * them on success instead of instructing the agent to try.
 */
export function composeInlinePrompt(prompt: string, files: string[]): string {
  const fileList = files.length > 0 ? `\n\nFiles:\n${files.join("\n")}` : "";
  return `${prompt}${fileList}\n`;
}

/** Identity under which the on-success `sandbox remove` runs. */
export interface SubmitCleanup {
  remarginPath: string;
  identityArgs: string[];
}

/** One group of a Submit: its prompt file, runner and staged files. */
export interface SubmitEntry {
  promptFile: string;
  runner: string;
  /** Staged files of this group, cleared on runner success. */
  files?: string[];
}

/**
 * One `cat <promptfile> | <runner>` per group, chained with `;`.
 * Groups with staged files get `&& remargin … sandbox remove <files>`
 * so the submitter's markers clear exactly when that group's runner
 * exits 0 — a failed group stays staged for resubmission.
 */
export function buildSubmitShellLine(entries: SubmitEntry[], cleanup?: SubmitCleanup): string {
  return entries
    .map((e) => {
      const run = `cat ${shellQuote(e.promptFile)} | ${e.runner}`;
      if (!cleanup || !e.files || e.files.length === 0) return run;
      const remove = [
        shellQuote(cleanup.remarginPath),
        ...cleanup.identityArgs.map(shellQuote),
        "sandbox",
        "remove",
        ...e.files.map(shellQuote),
      ].join(" ");
      return `${run} && ${remove}`;
    })
    .join("; ");
}

/** Filesystem-safe slug for a group's temp prompt-file basename. */
export function promptFileSlug(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug.length > 0 ? slug : "default";
}
