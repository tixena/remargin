export interface CommentOpts {
  replyTo?: string;
  afterLine?: number;
  afterComment?: string;
  to?: string[];
  attachments?: string[];
  autoAck?: boolean;
  /**
   * Stage the target file in the caller's sandbox in the same atomic write
   * (`remargin comment ... --sandbox`). Preferred over issuing a separate
   * `sandbox add` call because it avoids split-brain states where the comment
   * was written but the sandbox entry was not (or vice versa).
   */
  sandbox?: boolean;
}

/**
 * One entry from `remargin sandbox list --json`, tracking a markdown file that
 * the current identity has staged for a future Submit-to-Claude.
 */
export interface SandboxListEntry {
  /**
   * Path reported by the CLI. Relative to the vault (or the `--path` root)
   * unless `absolute` was requested.
   */
  path: string;
  /** ISO 8601 timestamp of when the file was staged by this identity. */
  since: string;
}

export interface QueryOpts {
  pending?: boolean;
  pendingFor?: string;
  author?: string;
  since?: string;
  expanded?: boolean;
  commentId?: string;
}

export interface GetOpts {
  startLine?: number;
  endLine?: number;
  lineNumbers?: boolean;
}

export interface WriteOpts {
  create?: boolean;
  raw?: boolean;
}

export interface SearchOpts {
  path?: string;
  scope?: "all" | "body" | "comments";
  regex?: boolean;
  ignoreCase?: boolean;
  context?: number;
}

export interface BatchCommentOp {
  content: string;
  replyTo?: string;
  afterLine?: number;
  afterComment?: string;
  to?: string[];
  autoAck?: boolean;
}

export interface IdentityInfo {
  found: boolean;
  path?: string;
  identity?: string;
  author_type?: string;
  key?: string;
  mode?: string;
}
