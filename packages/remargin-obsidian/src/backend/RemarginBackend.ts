import { createWriteStream, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname as dirnamePath, join as joinPath } from "node:path";
import { spawn } from "child_process";
import {
  type Comment,
  Comment$Schema,
  type ListEntry,
  ListEntry$Schema,
  ParticipantView$Schema,
  type QueryResult,
  QueryResult$Schema,
  SandboxFailureEntry$Schema,
  SandboxListEntry$Schema,
  type SearchMatch,
  SearchMatch$Schema,
} from "@/generated";
import { expandPath } from "@/lib/expandPath";
import type { ReleasesFetcher, UpdateCheckState } from "@/lib/githubReleases";
import { patchModeInYaml } from "@/lib/patchModeInYaml";
import type { RemarginSettings } from "@/types";
import { assembleExecArgs } from "./assembleExecArgs";
import { buildIdentityArgs } from "./buildIdentityArgs";
import { parsePluginsListOutput } from "./detectPlugin";
import { parsePayloadArray } from "./envelopeParsing";
import { IDENTITY_ACCEPTING_SUBCOMMANDS } from "./identityAcceptingSubcommands";
import { performUpdateCheck } from "./performUpdateCheck";
import type {
  BatchCommentOp,
  CommentOpts,
  GetOpts,
  IdentityInfo,
  Participant,
  PluginPresence,
  PromptListEntry,
  QueryOpts,
  ResolvedMode,
  ResolvedSystemPrompt,
  SandboxListEntry,
  SearchOpts,
  WriteOpts,
} from "./types";

// `IDENTITY_ACCEPTING_SUBCOMMANDS` lives in its own file
// (`identityAcceptingSubcommands.ts`) so tests can import it without
// pulling in this module, whose TypeScript parameter-property
// constructor the test runner's strip-only loader cannot parse.

export class RemarginBackend {
  private pluginPresenceCache: PluginPresence | null = null;

  constructor(
    private settings: RemarginSettings,
    private vaultPath: string
  ) {}

  updateSettings(settings: RemarginSettings): void {
    this.settings = settings;
  }

  /**
   * Probe `claude plugins list` and report whether the remargin plugin
   * is installed/enabled. Cached for the lifetime of this backend; call
   * `invalidatePluginPresence()` to force a fresh probe.
   */
  async detectPlugin(): Promise<PluginPresence> {
    if (this.pluginPresenceCache) return this.pluginPresenceCache;
    const binary = this.resolveClaudeBinary();
    const output = await new Promise<string>((resolve) => {
      const child = spawn(binary, ["plugins", "list"], {
        stdio: ["ignore", "pipe", "pipe"],
      });
      const chunks: Buffer[] = [];
      child.stdout.on("data", (chunk: Buffer) => {
        chunks.push(chunk);
      });
      child.on("error", () => {
        resolve("");
      });
      child.on("close", () => {
        resolve(Buffer.concat(chunks).toString("utf-8"));
      });
    });
    const presence = parsePluginsListOutput(output);
    this.pluginPresenceCache = presence;
    return presence;
  }

  invalidatePluginPresence(): void {
    this.pluginPresenceCache = null;
  }

  async get(path: string, opts?: GetOpts): Promise<string> {
    const args: string[] = ["get", path];
    if (opts?.startLine != null) args.push("--start", String(opts.startLine));
    if (opts?.endLine != null) args.push("--end", String(opts.endLine));
    if (opts?.lineNumbers) args.push("--line-numbers");
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw) as { content?: string; lines?: unknown };
    if (typeof parsed.content === "string") return parsed.content;
    return raw;
  }

  async ls(path: string): Promise<ListEntry[]> {
    const raw = await this.exec(["ls", path]);
    return parsePayloadArray(raw, "entries", ListEntry$Schema, "ls");
  }

  async write(path: string, content: string, opts?: WriteOpts): Promise<void> {
    const args: string[] = ["write", path, content];
    if (opts?.create) args.push("--create");
    if (opts?.raw) args.push("--raw");
    await this.exec(args);
  }

  async rm(path: string): Promise<{ deleted: string; existed: boolean }> {
    const raw = await this.exec(["rm", path]);
    return JSON.parse(raw);
  }

  async comments(file: string): Promise<Comment[]> {
    const raw = await this.exec(["comments", file]);
    return parsePayloadArray(raw, "comments", Comment$Schema, "comments");
  }

  async comment(file: string, content: string, opts?: CommentOpts): Promise<string> {
    const args: string[] = ["comment", file, content];
    if (opts?.replyTo) args.push("--reply-to", opts.replyTo);
    if (opts?.afterLine != null) args.push("--after-line", String(opts.afterLine));
    if (opts?.afterComment) args.push("--after-comment", opts.afterComment);
    if (opts?.autoAck) args.push("--auto-ack");
    if (opts?.sandbox) args.push("--sandbox");
    if (opts?.to) {
      for (const recipient of opts.to) {
        args.push("--to", recipient);
      }
    }
    if (opts?.attachments) {
      for (const attachment of opts.attachments) {
        args.push("--attach", attachment);
      }
    }
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw);
    return parsed.id as string;
  }

  /**
   * List every markdown file staged for the current identity's sandbox.
   *
   * Runs `remargin sandbox list --json` against the configured working
   * directory (or the vault root when no explicit working directory is set).
   * The CLI resolves the identity itself; callers do not override it.
   *
   * Paths in the returned entries are relative to the CLI's effective root,
   * matching the convention used elsewhere in the plugin.
   */
  async sandboxList(): Promise<SandboxListEntry[]> {
    const raw = await this.exec(["sandbox", "list"]);
    return parsePayloadArray(raw, "files", SandboxListEntry$Schema, "sandbox list");
  }

  /**
   * Remove the current identity's sandbox entry from one or more markdown
   * files. Used after a successful Submit-to-Claude so the plugin's sidepanel
   * refetches an empty list for the files that were just processed.
   *
   * The CLI emits a best-effort JSON envelope with `removed`, `skipped`, and
   * `failed` arrays; callers typically just check that the promise resolves
   * and refetch the sandbox list.
   */
  async sandboxRemove(files: string[]): Promise<void> {
    if (files.length === 0) return;
    const raw = await this.exec(["sandbox", "remove", ...files]);
    // Validate the failure rows; result discarded (we refetch after).
    parsePayloadArray(raw, "failed", SandboxFailureEntry$Schema, "sandbox remove");
  }

  /**
   * Stage one or more markdown files in the current identity's sandbox.
   * Calls `remargin sandbox add <files...>`. The operation is idempotent:
   * re-adding a file that is already staged preserves its existing timestamp.
   */
  async sandboxAdd(files: string[]): Promise<void> {
    if (files.length === 0) return;
    await this.exec(["sandbox", "add", ...files]);
  }

  async ack(file: string, ids: string[], remove = false): Promise<void> {
    const args: string[] = ["ack", "--file", file];
    if (remove) args.push("--remove");
    args.push(...ids);
    await this.exec(args);
  }

  async deleteComments(file: string, ids: string[]): Promise<void> {
    await this.exec(["delete", file, ...ids]);
  }

  async edit(file: string, id: string, content: string): Promise<void> {
    await this.exec(["edit", file, id, content]);
  }

  async react(file: string, id: string, emoji: string, remove = false): Promise<void> {
    const args = ["react", file, id, emoji];
    if (remove) args.push("--remove");
    await this.exec(args);
  }

  async batch(file: string, operations: BatchCommentOp[]): Promise<string[]> {
    const ops = operations.map((op) => {
      const obj: Record<string, unknown> = { content: op.content };
      if (op.replyTo) obj["reply_to"] = op.replyTo;
      if (op.afterLine != null) obj["after_line"] = op.afterLine;
      if (op.afterComment) obj["after_comment"] = op.afterComment;
      if (op.autoAck) obj["auto_ack"] = true;
      if (op.to) obj["to"] = op.to;
      return obj;
    });
    const raw = await this.exec(["batch", file, "--ops", JSON.stringify(ops)]);
    const parsed = JSON.parse(raw);
    return parsed.ids as string[];
  }

  async query(path: string, opts?: QueryOpts): Promise<QueryResult[]> {
    const args: string[] = ["query", path];
    if (opts?.pending) args.push("--pending");
    if (opts?.pendingFor) args.push("--pending-for", opts.pendingFor);
    if (opts?.author) args.push("--author", opts.author);
    if (opts?.since) args.push("--since", opts.since);
    if (opts?.expanded) args.push("--expanded");
    if (opts?.commentId) args.push("--comment-id", opts.commentId);
    if (opts?.contentRegex) args.push("--content-regex", opts.contentRegex);
    if (opts?.ignoreCase) args.push("--ignore-case");
    const raw = await this.exec(args);
    return parsePayloadArray(raw, "results", QueryResult$Schema, "query");
  }

  async search(pattern: string, opts?: SearchOpts): Promise<SearchMatch[]> {
    const args: string[] = ["search", pattern];
    if (opts?.path) args.push("--path", opts.path);
    if (opts?.scope) args.push("--scope", opts.scope);
    if (opts?.regex) args.push("--regex");
    if (opts?.ignoreCase) args.push("--ignore-case");
    if (opts?.context != null) args.push("--context", String(opts.context));
    const raw = await this.exec(args);
    return parsePayloadArray(raw, "matches", SearchMatch$Schema, "search");
  }

  async version(): Promise<string> {
    const raw = await this.exec(["--version"], {
      useJson: false,
      skipIdentity: true,
    });
    return raw.trim();
  }

  /**
   * Trigger the plugin's self-update flow by shelling out to
   * `remargin obsidian install --vault-path <vault>`.
   *
   * The CLI subcommand is responsible for re-staging `main.js`,
   * `manifest.json`, and `styles.css` into this vault's
   * `.obsidian/plugins/remargin/` directory. Install commands are
   * idempotent — succeed even if already registered.
   *
   * Returns `{ ok: true }` on exit code 0. On non-zero exit the stderr
   * tail is surfaced verbatim so the UI Notice can echo the CLI's own
   * error.
   *
   * `--vault-path` resolves identically to `exec`'s `cwd` so the CLI
   * mutates the same vault the plugin is running in, regardless of the
   * user's `workingDirectory` setting.
   */
  async installPluginToVault(): Promise<{ ok: boolean; stderr: string }> {
    const cwd = expandPath(this.settings.workingDirectory) || this.vaultPath;
    try {
      await this.exec(["obsidian", "install", "--vault-path", cwd], {
        useJson: false,
        skipIdentity: true,
        timeout: 60000,
      });
      return { ok: true, stderr: "" };
    } catch (err) {
      return {
        ok: false,
        stderr: err instanceof Error ? err.message : "plugin install failed",
      };
    }
  }

  /**
   * Run a version-check against the remargin GitHub releases feed.
   *
   * Thin method wrapper around the standalone `performUpdateCheck`
   * helper (see `./performUpdateCheck.ts`) so callers working with a
   * `RemarginBackend` instance do not have to thread the CLI-version
   * probe themselves. The standalone helper exists because the
   * test-runner's strip-only TypeScript loader cannot parse this class's
   * parameter-property constructor, so tests import the helper directly.
   */
  async checkForUpdates(args: {
    force: boolean;
    installedPlugin: string;
    fetcher: ReleasesFetcher;
    cache?: UpdateCheckState;
    now?: () => Date;
  }): Promise<UpdateCheckState> {
    return performUpdateCheck({
      ...args,
      cliVersion: () => this.version(),
    });
  }

  /**
   * Ask the CLI to resolve an identity config by walking up from the vault.
   * Does not pass any identity flags (so it can run before settings are
   * populated).
   */
  /**
   * Write or patch the `mode:` field in the vault-root `.remargin.yaml`.
   *
   * The vault root is the working directory resolved the same way `exec`
   * resolves it (expanded `workingDirectory` setting, falling back to the
   * plugin's known vault path). If the file does not exist, it is created
   * with just `mode: <value>` so we do not guess at identity or key on the
   * user's behalf.
   *
   * The write preserves every other field, comment, and blank line in the
   * file — see `patchModeInYaml`. This is a plugin-side edit (no CLI call)
   * because the target is a single well-known file and the patch is local.
   */
  setVaultMode(mode: string): void {
    const root = expandPath(this.settings.workingDirectory) || this.vaultPath;
    if (!root) {
      throw new Error("vault root is not configured; cannot write .remargin.yaml");
    }
    const target = joinPath(root, ".remargin.yaml");
    const existing = existsSync(target) ? readFileSync(target, "utf-8") : "";
    const patched = patchModeInYaml(existing, mode);
    writeFileSync(target, patched, "utf-8");
  }

  /**
   * Resolve the effective enforcement mode for the vault (or an explicit
   * directory) by walking up from `cwd` and reading the nearest
   * `.remargin.yaml`. Decoupled from identity resolution: never filters by
   * `type:` field, because mode is a directory-tree property.
   *
   * Returns `{ mode: "open", source: null }` when no config is found, matching
   * the CLI's open-by-default posture. No identity flags are forwarded so
   * this is safe to call before settings are populated.
   */
  async resolveMode(cwd?: string): Promise<ResolvedMode> {
    const args: string[] = ["resolve-mode"];
    if (cwd) args.push("--cwd", cwd);
    const raw = await this.exec(args, { skipIdentity: true });
    const parsed = JSON.parse(raw) as ResolvedMode;
    return parsed;
  }

  /**
   * Resolve the folder-scoped system prompt for `file` via
   * `remargin prompt resolve <file> --json`. Identity-free walk: the
   * prompt is a property of the directory tree, not the caller, so
   * different identities resolving from the same path get the same
   * answer.
   */
  async resolvePrompt(file: string): Promise<ResolvedSystemPrompt> {
    const raw = await this.exec(["prompt", "resolve", file]);
    return JSON.parse(raw) as ResolvedSystemPrompt;
  }

  /**
   * Resolve prompts for a batch of files in parallel. Naive but correct:
   * the CLI invocation cost is dominated by process spawn, not the walk
   * itself. If sandboxes ever grow large enough that this becomes a
   * bottleneck a `--batch` CLI mode can be added without changing the
   * public surface here.
   */
  async resolvePrompts(files: string[]): Promise<Map<string, ResolvedSystemPrompt>> {
    const entries = await Promise.all(
      files.map(async (f) => [f, await this.resolvePrompt(f)] as const)
    );
    return new Map(entries);
  }

  /**
   * Create or replace the `system_prompt:` block in
   * `<folder>/.remargin.yaml` via `remargin prompt set`. The body is
   * piped on stdin so multi-line content round-trips byte-for-byte
   * without flag-escaping. Other YAML fields are preserved verbatim
   * by the CLI's post-write diff.
   */
  async promptSet(folder: string, name: string, prompt: string): Promise<void> {
    await this.exec(["prompt", "set", folder, "--name", name], { stdin: prompt });
  }

  /**
   * Strip the `system_prompt:` block from `<folder>/.remargin.yaml`
   * via `remargin prompt delete`. Idempotent; the file is preserved
   * even if it ends up empty.
   */
  async promptDelete(folder: string): Promise<void> {
    await this.exec(["prompt", "delete", folder]);
  }

  /**
   * Recursively enumerate every `.remargin.yaml` under `folder` that
   * declares a `system_prompt:` block. Read-only, identity-free.
   */
  async promptList(folder: string): Promise<PromptListEntry[]> {
    const raw = await this.exec(["prompt", "list", folder]);
    const parsed = JSON.parse(raw) as { entries: PromptListEntry[] };
    return parsed.entries;
  }

  /**
   * Ask the CLI which identity is active under the plugin's current
   * settings. Forwarding `buildIdentityArgs(settings)` keeps this read
   * path aligned with every mutating op — without it the plugin picks
   * up the nearest walked `.remargin.yaml` instead of the config file
   * the user pointed settings at, and `me` disagrees with the identity
   * writes land under (which flips the ack UI).
   *
   * `type` still narrows the branch-3 walk filter in manual mode
   * (when the plugin has not been pointed at a config file).
   */
  async identity(type?: "human" | "agent"): Promise<IdentityInfo> {
    const args: string[] = ["identity"];
    if (type) args.push("--type", type);
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw) as IdentityInfo;
    return parsed;
  }

  /**
   * Fetch the current vault's registered participants via
   * `remargin --json registry show`.
   *
   * Gracefully degrades to an empty list when the CLI errors with
   * `"no registry found"` so the plugin works on vaults that haven't
   * set up a registry yet. Any other error (binary missing, parse
   * failure, permission) is rethrown so real bugs surface.
   */
  async registryShow(): Promise<Participant[]> {
    try {
      const raw = await this.exec(["registry", "show"]);
      return parsePayloadArray(
        raw,
        "participants",
        ParticipantView$Schema,
        "registry show"
      ) as Participant[];
    } catch (err) {
      if (err instanceof Error && /no registry found/i.test(err.message)) {
        return [];
      }
      throw err;
    }
  }

  private async exec(
    args: string[],
    opts?: { timeout?: number; useJson?: boolean; skipIdentity?: boolean; stdin?: string }
  ): Promise<string> {
    const binary = this.resolveBinary();
    const cwd = expandPath(this.settings.workingDirectory) || this.vaultPath;
    const timeout = opts?.timeout ?? 30000;
    const useJson = opts?.useJson ?? true;
    const skipIdentity = opts?.skipIdentity ?? false;
    const stdinInput = opts?.stdin;
    const subcommand = args[0];
    const identityAccepted =
      subcommand !== undefined && IDENTITY_ACCEPTING_SUBCOMMANDS.has(subcommand);
    const fullArgs = assembleExecArgs({
      args,
      identityArgs: this.buildIdentityArgs(),
      useJson,
      identityAccepted,
      skipIdentity,
    });

    return new Promise<string>((resolve, reject) => {
      const child = spawn(binary, fullArgs, { cwd });
      if (stdinInput !== undefined) {
        child.stdin.write(stdinInput);
        child.stdin.end();
      }

      const stdoutChunks: Buffer[] = [];
      const stderrChunks: Buffer[] = [];
      let settled = false;

      const settle = (fn: () => void): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        fn();
      };

      child.stdout.on("data", (chunk: Buffer) => stdoutChunks.push(chunk));
      child.stderr.on("data", (chunk: Buffer) => stderrChunks.push(chunk));

      // Manual timeout — spawn doesn't have a built-in timeout option.
      const timer = setTimeout(() => {
        child.kill();
        settle(() => reject(new Error(`remargin timed out after ${timeout}ms`)));
      }, timeout);

      child.on("error", (err: NodeJS.ErrnoException) => {
        settle(() => {
          if (err.code === "ENOENT") {
            reject(new Error(`remargin binary not found at "${binary}". Check plugin settings.`));
          } else {
            reject(new Error(`failed to spawn remargin: ${err.message}`));
          }
        });
      });

      child.on("close", (code) => {
        settle(() => {
          const stdout = Buffer.concat(stdoutChunks).toString("utf-8");
          const stderr = Buffer.concat(stderrChunks).toString("utf-8");

          if (code !== 0) {
            const detail = stderr.trim() || `exit code ${code ?? "unknown"}`;
            const cmdPreview = [binary, ...fullArgs].join(" ");
            reject(new Error(`${detail}\n  command: ${cmdPreview}`));
            return;
          }
          resolve(stdout);
        });
      });
    });
  }

  private buildIdentityArgs(): string[] {
    return buildIdentityArgs(this.settings);
  }

  /**
   * Resolve the remargin binary to invoke. Applies `expandPath` to the
   * configured path so `~/...` and `$HOME/...` work, then falls back to a
   * bare PATH lookup (`remargin`) if the configured path does not exist on
   * disk. This lets users leave the setting blank when remargin is on their
   * PATH, and lets them use a portable `~/.cargo/bin/remargin` style entry
   * across machines.
   */
  private resolveBinary(): string {
    const configured = expandPath(this.settings.remarginPath);
    if (!configured) return "remargin";
    // If the user typed a bare command name (no path separator), trust
    // PATH lookup — don't stat it.
    const looksLikePath = configured.includes("/") || configured.includes("\\");
    if (!looksLikePath) return configured;
    if (existsSync(configured)) return configured;
    // Fallback: try the bare name on PATH.
    return "remargin";
  }

  /**
   * Spawn `claude -p <prompt>` and wait for completion. Used by the
   * Submit-all pipeline (task 48): one invocation per prompt group,
   * sequential, continue-on-failure handled by the caller.
   *
   * The prompt body is passed as a single argv element via `spawn`'s
   * args array, so shell-special characters in the body are inert
   * (no `sh -c`).
   */
  async invokeClaude(prompt: string, files: string[], opts?: InvokeClaudeOpts): Promise<void> {
    const binary = this.resolveClaudeBinary();
    const cwd = opts?.cwd ?? (expandPath(this.settings.workingDirectory) || this.vaultPath);
    const timeout = opts?.timeout ?? 300_000;
    const fullPrompt = buildClaudePrompt(prompt, files, opts?.useSlashCommand);

    const identity = await this.identity().catch(() => null);
    const logStream = opts?.logPath
      ? openLogStream(opts.logPath, {
          binary,
          prompt,
          files,
          cwd,
          promptName: opts?.promptName,
          identity: identity?.identity,
        })
      : null;

    // WHY: headless claude denies any tool requiring runtime approval
    // because there's no interactive prompt. Pre-approving the remargin
    // MCP namespace lets the launched agent read pending comments,
    // write replies, etc. without the user having to click through
    // permission dialogs the plugin can't surface.
    const args = ["-p", fullPrompt, "--allowedTools", "mcp__remargin__*"];

    return new Promise<void>((resolve, reject) => {
      // WHY: claude -p waits ~3s on stdin before printing
      // "no stdin data received". We never pipe to stdin, so close it
      // immediately via stdio[0]='ignore' (equivalent to < /dev/null).
      const child = spawn(binary, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
      const stderrChunks: Buffer[] = [];
      let settled = false;

      const settle = (fn: () => void): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        fn();
      };

      child.stderr.on("data", (chunk: Buffer) => {
        stderrChunks.push(chunk);
        logStream?.write(chunk);
      });
      // Drain stdout so the child doesn't deadlock on a full pipe.
      child.stdout.on("data", (chunk: Buffer) => {
        logStream?.write(chunk);
      });

      const timer = setTimeout(() => {
        child.kill();
        settle(() => {
          logStream?.end(`\n${"`".repeat(4)}\n\n_[remargin] killed: timeout after ${timeout}ms_\n`);
          reject(new Error(`claude timed out after ${timeout}ms`));
        });
      }, timeout);

      child.on("error", (err: NodeJS.ErrnoException) => {
        settle(() => {
          const msg =
            err.code === "ENOENT"
              ? `claude binary not found at "${binary}". Check plugin settings.`
              : `failed to spawn claude: ${err.message}`;
          logStream?.end(`\n${"`".repeat(4)}\n\n_[remargin] spawn error: ${msg}_\n`);
          reject(new Error(msg));
        });
      });

      child.on("close", (code) => {
        settle(() => {
          logStream?.end(`\n${"`".repeat(4)}\n\n_[remargin] exit ${code ?? "unknown"}_\n`);
          if (code !== 0) {
            const stderr = Buffer.concat(stderrChunks).toString("utf-8");
            const detail = stderr.trim() || `exit code ${code ?? "unknown"}`;
            reject(new Error(detail));
            return;
          }
          resolve();
        });
      });
    });
  }

  /**
   * Resolve the `claude` binary path. Mirrors [`resolveBinary`] but
   * keyed off `settings.claudePath`. Empty / bare-name values fall
   * back to PATH lookup.
   */
  private resolveClaudeBinary(): string {
    const configured = expandPath(this.settings.claudePath);
    if (!configured) return "claude";
    const looksLikePath = configured.includes("/") || configured.includes("\\");
    if (!looksLikePath) return configured;
    if (existsSync(configured)) return configured;
    return "claude";
  }
}

export function buildClaudePrompt(
  prompt: string,
  files: string[],
  slash?: { command: string; arg?: string }
): string {
  if (slash) {
    return slash.arg ? `/${slash.command} ${slash.arg}` : `/${slash.command}`;
  }
  return files.length > 0 ? `${prompt}\n\nFiles:\n${files.join("\n")}` : prompt;
}

export interface InvokeClaudeOpts {
  /** Timeout in ms. Default: 300_000 (5 min). Per-group, not total. */
  timeout?: number;
  /**
   * Working directory for the spawn. Default: the plugin's working
   * directory (or vault root when blank).
   */
  cwd?: string;
  /**
   * Absolute path of the per-run log file. When set, stdout+stderr are
   * appended in real time. Parent directories are created if missing.
   */
  logPath?: string;
  /** Resolved system-prompt name; used as the log file's H1. */
  promptName?: string;
  /**
   * Invoke a slash command instead of the inline-prompt argv. When set,
   * `prompt` and `files` are ignored — the spawned claude receives
   * `/<command>[ <arg>]` as its `-p` argument.
   */
  useSlashCommand?: { command: string; arg?: string };
}

interface OpenLogStreamArgs {
  binary: string;
  prompt: string;
  files: string[];
  cwd: string;
  promptName?: string;
  identity?: string;
}

function openLogStream(logPath: string, args: OpenLogStreamArgs) {
  mkdirSync(dirnamePath(logPath), { recursive: true });
  const stream = createWriteStream(logPath, { flags: "a" });
  const ts = new Date().toISOString();
  const title = args.promptName?.trim() || "Submit run";
  const fileList = args.files.length ? args.files.map((f) => `- \`${f}\``).join("\n") : "_(none)_";
  // WHY: 4-backtick fence so a 3-backtick block inside the spawn output
  // doesn't close it early. The footer (written on stream end) closes
  // this fence and adds the exit-status line.
  const fence = "`".repeat(4);
  const header =
    "---\n" +
    `ts: ${ts}\n` +
    (args.identity ? `identity: ${args.identity}\n` : "") +
    `cwd: ${args.cwd}\n` +
    `command: ${args.binary} -p\n` +
    "---\n\n" +
    `# ${title}\n\n` +
    "## Prompt\n\n" +
    `${args.prompt.trim()}\n\n` +
    "## Files\n\n" +
    `${fileList}\n\n` +
    "## Output\n\n" +
    `${fence}\n`;
  stream.write(header);
  return stream;
}
