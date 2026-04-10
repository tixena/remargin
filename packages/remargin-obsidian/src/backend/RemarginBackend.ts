import { exec } from "child_process";
import { z } from "zod/v4";
import {
  type Comment,
  Comment$Schema,
  type QueryResult,
  QueryResult$Schema,
  type ListEntry,
  ListEntry$Schema,
  type SearchMatch,
  SearchMatch$Schema,
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
    const args = ["get", "--file", path];
    if (opts?.startLine != null) args.push("--start-line", String(opts.startLine));
    if (opts?.endLine != null) args.push("--end-line", String(opts.endLine));
    if (opts?.lineNumbers) args.push("--line-numbers");
    return this.exec(args);
  }

  async ls(path: string): Promise<ListEntry[]> {
    const raw = await this.exec(["ls", "--path", path]);
    const parsed = JSON.parse(raw);
    return z.array(ListEntry$Schema).parse(parsed);
  }

  async write(
    path: string,
    content: string,
    opts?: WriteOpts
  ): Promise<void> {
    const args = ["write", "--file", path, "--content", content];
    if (opts?.create) args.push("--create");
    if (opts?.raw) args.push("--raw");
    await this.exec(args);
  }

  async rm(path: string): Promise<{ deleted: string; existed: boolean }> {
    const raw = await this.exec(["rm", "--file", path]);
    return JSON.parse(raw);
  }

  // -- Comments ---------------------------------------------------------------

  async comments(file: string): Promise<Comment[]> {
    const raw = await this.exec(["comments", "--file", file]);
    const parsed = JSON.parse(raw);
    return z.array(Comment$Schema).parse(parsed);
  }

  async comment(
    file: string,
    content: string,
    opts?: CommentOpts
  ): Promise<string> {
    const args = ["comment", "--file", file, "--content", content];
    if (opts?.replyTo) args.push("--reply-to", opts.replyTo);
    if (opts?.afterLine != null) args.push("--after-line", String(opts.afterLine));
    if (opts?.afterComment) args.push("--after-comment", opts.afterComment);
    if (opts?.autoAck) args.push("--auto-ack");
    if (opts?.to) {
      for (const recipient of opts.to) {
        args.push("--to", recipient);
      }
    }
    if (opts?.attachments) {
      for (const attachment of opts.attachments) {
        args.push("--attachment", attachment);
      }
    }
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw);
    return parsed.id as string;
  }

  async ack(file: string, ids: string[]): Promise<void> {
    const args = ["ack", "--file", file];
    for (const id of ids) {
      args.push("--comment-id", id);
    }
    await this.exec(args);
  }

  async deleteComments(file: string, ids: string[]): Promise<void> {
    for (const id of ids) {
      await this.exec(["delete", "--file", file, "--comment-id", id]);
    }
  }

  async edit(file: string, id: string, content: string): Promise<void> {
    await this.exec([
      "edit",
      "--file",
      file,
      "--comment-id",
      id,
      "--content",
      content,
    ]);
  }

  async react(
    file: string,
    id: string,
    emoji: string
  ): Promise<void> {
    await this.exec([
      "react",
      "--file",
      file,
      "--comment-id",
      id,
      "--emoji",
      emoji,
    ]);
  }

  async batch(
    file: string,
    operations: BatchCommentOp[]
  ): Promise<string[]> {
    const batchArgs: string[] = [];
    for (const op of operations) {
      const parts = [`content=${op.content}`];
      if (op.replyTo) parts.push(`reply_to=${op.replyTo}`);
      if (op.afterLine != null) parts.push(`after_line=${op.afterLine}`);
      if (op.afterComment) parts.push(`after_comment=${op.afterComment}`);
      if (op.autoAck) parts.push("auto_ack=true");
      if (op.to) {
        for (const recipient of op.to) {
          parts.push(`to=${recipient}`);
        }
      }
      batchArgs.push(parts.join(","));
    }
    const raw = await this.exec([
      "batch",
      "--file",
      file,
      ...batchArgs.flatMap((b) => ["--op", b]),
    ]);
    const parsed = JSON.parse(raw);
    return parsed.ids as string[];
  }

  // -- Search & query ---------------------------------------------------------

  async query(path: string, opts?: QueryOpts): Promise<QueryResult[]> {
    const args = ["query", "--path", path];
    if (opts?.pending) args.push("--pending");
    if (opts?.pendingFor) args.push("--pending-for", opts.pendingFor);
    if (opts?.author) args.push("--author", opts.author);
    if (opts?.since) args.push("--since", opts.since);
    if (opts?.expanded) args.push("--expanded");
    if (opts?.commentId) args.push("--comment-id", opts.commentId);
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw);
    return z.array(QueryResult$Schema).parse(parsed);
  }

  async search(
    pattern: string,
    opts?: SearchOpts
  ): Promise<SearchMatch[]> {
    const args = ["search", "--pattern", pattern];
    if (opts?.path) args.push("--path", opts.path);
    if (opts?.scope) args.push("--scope", opts.scope);
    if (opts?.regex) args.push("--regex");
    if (opts?.ignoreCase) args.push("--ignore-case");
    if (opts?.context != null) args.push("--context", String(opts.context));
    const raw = await this.exec(args);
    const parsed = JSON.parse(raw);
    return z.array(SearchMatch$Schema).parse(parsed);
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
    const args = ["identity"];
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
            reject(
              new Error(`remargin timed out after ${timeout}ms`)
            );
          } else if ((error as NodeJS.ErrnoException).code === "ENOENT") {
            reject(
              new Error(
                `remargin binary not found at "${binary}". Check plugin settings.`
              )
            );
          } else {
            reject(new Error(stderr.trim() || error.message));
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
