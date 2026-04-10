import { exec } from "child_process";
import type {
  Comment,
  QueryResult,
  ExpandedComment,
  ListEntry,
  SearchMatch,
} from "@/generated";
import type { RemarginSettings } from "@/types";
import type {
  CommentOpts,
  QueryOpts,
  GetOpts,
  WriteOpts,
  SearchOpts,
  BatchCommentOp,
  IdentityInfo,
} from "./types";

function shellescape(arg: string): string {
  return `'${arg.replace(/'/g, "'\\''")}'`;
}

/**
 * Transform a raw comment object emitted by `remargin --json comments` into
 * the shape the UI and generated types expect.
 *
 * The CLI currently hand-writes JSON instead of serializing its typed structs,
 * so there's drift we have to patch up here:
 *
 *  - field name: `type` (lowercase) → `author_type` (what the schema uses)
 *  - enum case:  `"agent"`/`"human"` → `"Agent"`/`"Human"` (PascalCase)
 *  - optional collections are omitted when empty; fill them with defaults so
 *    the UI can iterate them unconditionally.
 */
function normalizeComment(raw: Record<string, unknown>): Comment {
  const rawType =
    (raw["author_type"] as string | undefined) ??
    (raw["type"] as string | undefined);
  const authorType = toPascalAuthorType(rawType);

  return {
    id: (raw["id"] as string) ?? "",
    author: (raw["author"] as string) ?? "",
    author_type: authorType,
    ts: (raw["ts"] as string) ?? "",
    content: (raw["content"] as string) ?? "",
    checksum: (raw["checksum"] as string) ?? "",
    line: (raw["line"] as number) ?? 0,
    fence_depth: (raw["fence_depth"] as number) ?? 3,
    to: (raw["to"] as string[] | undefined) ?? [],
    ack:
      (raw["ack"] as Array<{ author: string; ts: string }> | undefined) ?? [],
    reactions:
      (raw["reactions"] as Record<string, string[]> | undefined) ?? {},
    attachments: (raw["attachments"] as string[] | undefined) ?? [],
    reply_to: raw["reply_to"] as string | undefined,
    thread: raw["thread"] as string | undefined,
    signature: raw["signature"] as string | undefined,
  };
}

/**
 * Same idea as `normalizeComment`, but for the `ExpandedComment` shape that
 * `query --expanded` embeds inside each result (includes a `file` field).
 */
function normalizeExpandedComment(
  raw: Record<string, unknown>
): ExpandedComment {
  const rawType =
    (raw["author_type"] as string | undefined) ??
    (raw["type"] as string | undefined);
  const authorType = toPascalAuthorType(rawType);

  return {
    id: (raw["id"] as string) ?? "",
    author: (raw["author"] as string) ?? "",
    author_type: authorType,
    ts: (raw["ts"] as string) ?? "",
    content: (raw["content"] as string) ?? "",
    checksum: (raw["checksum"] as string) ?? "",
    line: (raw["line"] as number) ?? 0,
    file: (raw["file"] as string) ?? "",
    to: (raw["to"] as string[] | undefined) ?? [],
    ack:
      (raw["ack"] as Array<{ author: string; ts: string }> | undefined) ?? [],
    reactions:
      (raw["reactions"] as Record<string, string[]> | undefined) ?? {},
    attachments: (raw["attachments"] as string[] | undefined) ?? [],
    reply_to: raw["reply_to"] as string | undefined,
    thread: raw["thread"] as string | undefined,
    signature: raw["signature"] as string | undefined,
  };
}

function normalizeQueryResult(raw: Record<string, unknown>): QueryResult {
  const rawComments =
    (raw["comments"] as Array<Record<string, unknown>> | undefined) ?? [];
  return {
    path: (raw["path"] as string) ?? "",
    comment_count: (raw["comment_count"] as number) ?? 0,
    pending_count: (raw["pending_count"] as number) ?? 0,
    pending_for: (raw["pending_for"] as string[] | undefined) ?? [],
    last_activity: raw["last_activity"] as string | undefined,
    comments: rawComments.map(normalizeExpandedComment),
  };
}

function toPascalAuthorType(
  value: string | undefined
): "Agent" | "Human" {
  const lower = (value ?? "").toLowerCase();
  if (lower === "human") return "Human";
  return "Agent";
}

export class RemarginBackend {
  constructor(
    private settings: RemarginSettings,
    private vaultPath: string
  ) {}

  updateSettings(settings: RemarginSettings): void {
    this.settings = settings;
  }

  // -- Document access --------------------------------------------------------

  async get(path: string, opts?: GetOpts): Promise<string> {
    const args: string[] = ["get", path];
    if (opts?.startLine != null) args.push("--start", String(opts.startLine));
    if (opts?.endLine != null) args.push("--end", String(opts.endLine));
    if (opts?.lineNumbers) args.push("--line-numbers");
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw) as { content?: string; lines?: unknown };
    // When --line-numbers is set the CLI returns { lines: [...] }; otherwise
    // { content: "..." }. The callers here only use the plain-content form.
    if (typeof parsed.content === "string") return parsed.content;
    return raw;
  }

  async ls(path: string): Promise<ListEntry[]> {
    const raw = await this.exec(["ls", path]);
    const parsed = JSON.parse(raw) as { entries?: ListEntry[] };
    return parsed.entries ?? [];
  }

  async write(
    path: string,
    content: string,
    opts?: WriteOpts
  ): Promise<void> {
    const args: string[] = ["write", path, content];
    if (opts?.create) args.push("--create");
    if (opts?.raw) args.push("--raw");
    await this.exec(args);
  }

  async rm(path: string): Promise<{ deleted: string; existed: boolean }> {
    const raw = await this.exec(["rm", path]);
    return JSON.parse(raw);
  }

  // -- Comments ---------------------------------------------------------------

  async comments(file: string): Promise<Comment[]> {
    const raw = await this.exec(["comments", file]);
    const parsed = JSON.parse(raw) as {
      comments?: Array<Record<string, unknown>>;
    };
    const list = parsed.comments ?? [];
    return list.map(normalizeComment);
  }

  async comment(
    file: string,
    content: string,
    opts?: CommentOpts
  ): Promise<string> {
    const args: string[] = ["comment", file, content];
    if (opts?.replyTo) args.push("--reply-to", opts.replyTo);
    if (opts?.afterLine != null)
      args.push("--after-line", String(opts.afterLine));
    if (opts?.afterComment) args.push("--after-comment", opts.afterComment);
    if (opts?.autoAck) args.push("--auto-ack");
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

  async batch(
    file: string,
    operations: BatchCommentOp[]
  ): Promise<string[]> {
    const ops = operations.map((op) => {
      const obj: Record<string, unknown> = { content: op.content };
      if (op.replyTo) obj["reply_to"] = op.replyTo;
      if (op.afterLine != null) obj["after_line"] = op.afterLine;
      if (op.afterComment) obj["after_comment"] = op.afterComment;
      if (op.autoAck) obj["auto_ack"] = true;
      if (op.to) obj["to"] = op.to;
      return obj;
    });
    const raw = await this.exec([
      "batch",
      file,
      "--ops",
      JSON.stringify(ops),
    ]);
    const parsed = JSON.parse(raw);
    return parsed.ids as string[];
  }

  // -- Search & query ---------------------------------------------------------

  async query(path: string, opts?: QueryOpts): Promise<QueryResult[]> {
    const args: string[] = ["query", path];
    if (opts?.pending) args.push("--pending");
    if (opts?.pendingFor) args.push("--pending-for", opts.pendingFor);
    if (opts?.author) args.push("--author", opts.author);
    if (opts?.since) args.push("--since", opts.since);
    if (opts?.expanded) args.push("--expanded");
    if (opts?.commentId) args.push("--comment-id", opts.commentId);
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw) as {
      results?: Array<Record<string, unknown>>;
    };
    const list = parsed.results ?? [];
    return list.map(normalizeQueryResult);
  }

  async search(
    pattern: string,
    opts?: SearchOpts
  ): Promise<SearchMatch[]> {
    const args: string[] = ["search", pattern];
    if (opts?.path) args.push("--path", opts.path);
    if (opts?.scope) args.push("--scope", opts.scope);
    if (opts?.regex) args.push("--regex");
    if (opts?.ignoreCase) args.push("--ignore-case");
    if (opts?.context != null) args.push("--context", String(opts.context));
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw) as { matches?: SearchMatch[] };
    return parsed.matches ?? [];
  }

  // -- Utility ----------------------------------------------------------------

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

  // -- Internal ---------------------------------------------------------------

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
    const fullArgs = [
      ...identityArgs,
      ...(useJson ? ["--json"] : []),
      ...args,
    ];

    const cmd = [shellescape(binary), ...fullArgs.map(shellescape)].join(" ");

    return new Promise((resolve, reject) => {
      exec(cmd, { cwd, timeout }, (error, stdout, stderr) => {
        if (error) {
          if (error.killed) {
            reject(new Error(`remargin timed out after ${timeout}ms`));
          } else if ((error as NodeJS.ErrnoException).code === "ENOENT") {
            reject(
              new Error(
                `remargin binary not found at "${binary}". Check plugin settings.`
              )
            );
          } else {
            const detail = stderr.trim() || error.message;
            // Surface the command that failed so users can reproduce it.
            reject(new Error(`${detail}\n  command: ${cmd}`));
          }
          return;
        }
        resolve(stdout);
      });
    });
  }

  private buildIdentityArgs(): string[] {
    if (
      this.settings.identityMode === "config" &&
      this.settings.configFilePath
    ) {
      return ["--config", this.settings.configFilePath];
    }
    const args: string[] = [];
    if (this.settings.authorName) {
      args.push("--identity", this.settings.authorName);
    }
    return args;
  }
}
