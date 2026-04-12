import { spawn } from "child_process";
import { z } from "zod/v4";
import {
  type Comment,
  Comment$Schema,
  type ListEntry,
  ListEntry$Schema,
  type QueryResult,
  QueryResult$Schema,
  type SearchMatch,
  SearchMatch$Schema,
} from "@/generated";
import type { RemarginSettings } from "@/types";
import type {
  BatchCommentOp,
  CommentOpts,
  GetOpts,
  IdentityInfo,
  QueryOpts,
  SandboxListEntry,
  SearchOpts,
  WriteOpts,
} from "./types";

/**
 * The CLI wraps its payload in an object that also contains timing metadata
 * (`elapsed_ms`), so the top-level envelopes are parsed with loose objects
 * that only care about the specific payload fields.
 */
const CommentsEnvelope$Schema = z.looseObject({
  comments: z.array(Comment$Schema),
});

const QueryEnvelope$Schema = z.looseObject({
  results: z.array(QueryResult$Schema),
});

const ListEnvelope$Schema = z.looseObject({
  entries: z.array(ListEntry$Schema),
});

const SearchEnvelope$Schema = z.looseObject({
  matches: z.array(SearchMatch$Schema),
});

const SandboxListEntry$Schema = z.looseObject({
  path: z.string(),
  since: z.string(),
});

const SandboxListEnvelope$Schema = z.looseObject({
  files: z.array(SandboxListEntry$Schema),
});

const SandboxRemoveEnvelope$Schema = z.looseObject({
  removed: z.array(z.string()).optional(),
  skipped: z.array(z.string()).optional(),
  failed: z
    .array(
      z.looseObject({
        path: z.string(),
        reason: z.string(),
      })
    )
    .optional(),
});

/**
 * Parse CLI stdout against a Zod schema and surface a readable error on
 * validation failure so callers can tell the difference between a broken
 * CLI version and a transient runtime problem.
 */
function parseEnvelope<T>(raw: string, schema: z.ZodType<T>, label: string): T {
  let payload: unknown;
  try {
    payload = JSON.parse(raw);
  } catch (err) {
    throw new Error(`remargin ${label}: could not parse JSON (${(err as Error).message})`);
  }
  const result = schema.safeParse(payload);
  if (!result.success) {
    throw new Error(`remargin ${label}: output did not match schema: ${result.error.message}`);
  }
  return result.data;
}

export class RemarginBackend {
  constructor(
    private settings: RemarginSettings,
    private vaultPath: string
  ) {}

  updateSettings(settings: RemarginSettings): void {
    this.settings = settings;
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
    return parseEnvelope(raw, ListEnvelope$Schema, "ls").entries;
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
    return parseEnvelope(raw, CommentsEnvelope$Schema, "comments").comments;
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
    return parseEnvelope(raw, SandboxListEnvelope$Schema, "sandbox list").files;
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
    // Validate the shape but discard the result — we refetch after this.
    parseEnvelope(raw, SandboxRemoveEnvelope$Schema, "sandbox remove");
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

  async ack(file: string, ids: string[]): Promise<void> {
    const args: string[] = ["ack", "--file", file, ...ids];
    await this.exec(args);
  }

  async deleteComments(file: string, ids: string[]): Promise<void> {
    await this.exec(["delete", file, ...ids]);
  }

  async edit(file: string, id: string, content: string): Promise<void> {
    await this.exec(["edit", file, id, content]);
  }

  async react(file: string, id: string, emoji: string): Promise<void> {
    await this.exec(["react", file, id, emoji]);
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
    const raw = await this.exec(args);
    return parseEnvelope(raw, QueryEnvelope$Schema, "query").results;
  }

  async search(pattern: string, opts?: SearchOpts): Promise<SearchMatch[]> {
    const args: string[] = ["search", pattern];
    if (opts?.path) args.push("--path", opts.path);
    if (opts?.scope) args.push("--scope", opts.scope);
    if (opts?.regex) args.push("--regex");
    if (opts?.ignoreCase) args.push("--ignore-case");
    if (opts?.context != null) args.push("--context", String(opts.context));
    const raw = await this.exec(args);
    return parseEnvelope(raw, SearchEnvelope$Schema, "search").matches;
  }

  async version(): Promise<string> {
    const raw = await this.exec(["--version"], {
      useJson: false,
      skipIdentity: true,
    });
    return raw.trim();
  }

  /**
   * Ask the CLI to resolve an identity config by walking up from the vault.
   * Does not pass any identity flags (so it can run before settings are
   * populated).
   */
  async identity(type?: "human" | "agent"): Promise<IdentityInfo> {
    const args: string[] = ["identity"];
    if (type) args.push("--type", type);
    const raw = await this.exec(args, { skipIdentity: true });
    const parsed = JSON.parse(raw) as IdentityInfo;
    return parsed;
  }

  private async exec(
    args: string[],
    opts?: { timeout?: number; useJson?: boolean; skipIdentity?: boolean }
  ): Promise<string> {
    const binary = this.settings.remarginPath || "remargin";
    const cwd = this.settings.workingDirectory || this.vaultPath;
    const timeout = opts?.timeout ?? 30000;
    const useJson = opts?.useJson ?? true;
    const skipIdentity = opts?.skipIdentity ?? false;
    const identityArgs = skipIdentity ? [] : this.buildIdentityArgs();
    // The CLI parses global flags before the subcommand, so identity/JSON
    // flags must come first.
    const fullArgs = [...identityArgs, ...(useJson ? ["--json"] : []), ...args];

    return new Promise<string>((resolve, reject) => {
      const child = spawn(binary, fullArgs, { cwd });

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
    if (this.settings.identityMode === "config" && this.settings.configFilePath) {
      return ["--config", this.settings.configFilePath, "--type", "human"];
    }
    const args: string[] = [];
    if (this.settings.authorName) {
      args.push("--identity", this.settings.authorName);
    }
    args.push("--type", "human");
    return args;
  }
}
