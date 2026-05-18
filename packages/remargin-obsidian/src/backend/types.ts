/**
 * A single entry from `remargin registry show --json`. Mirrors the CLI JSON
 * shape: `display_name` is always present (the CLI substitutes the id when
 * the registry leaves it blank), so consumers never need to handle null.
 *
 * Revoked participants are included so historical comments from them can
 * still render their human-friendly name; downstream UI (e.g. the to: picker)
 * is expected to filter by `status === "active"`.
 */
export interface Participant {
  name: string;
  display_name: string;
  type: "human" | "agent";
  status: "active" | "revoked";
  pubkeys: number;
}

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
  /**
   * Regex applied to comment content. Composes with metadata filters (pending,
   * author, since, comment-id) and runs after them so the regex only executes
   * against the already-filtered comment set.
   */
  contentRegex?: string;
  /**
   * Case-insensitive match for `contentRegex`. Has no effect without
   * `contentRegex` set.
   */
  ignoreCase?: boolean;
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

/**
 * Response from `remargin --json resolve-mode`. Mode is a directory-tree
 * property (not an identity property), so this probe exists independently of
 * the identity resolution: it walks up from the given `cwd` looking for the
 * nearest `.remargin.yaml` regardless of its `type:` field.
 *
 * When no config is found, `mode` defaults to `"open"` and `source` is
 * `null` — matching the CLI's open-by-default posture.
 */
export interface ResolvedMode {
  /** Effective mode: `"open"`, `"registered"`, or `"strict"`. */
  mode: string;
  /**
   * Absolute path of the `.remargin.yaml` that declared the mode, or
   * `null` when the resolution fell back to the default.
   */
  source: string | null;
}

/**
 * Response from `remargin prompt resolve <file> --json`. The prompt walk
 * is identity-free: a folder's prompt is a property of the directory
 * tree, not the caller. When no `.remargin.yaml` in the parent chain
 * declared a `system_prompt:` block, the CLI returns the y76 Default
 * body with `is_default = true` and `source = null`.
 */
export interface ResolvedSystemPrompt {
  /** True when the walk exhausted and the resolver returned the Default body. */
  is_default: boolean;
  /**
   * Human-readable label. Resolved from the YAML `name:` field when
   * present, otherwise from the owning folder basename. `"default"`
   * for the Default fallback.
   */
  name: string;
  /** Body to forward to Claude. */
  prompt: string;
  /**
   * Absolute path of the `.remargin.yaml` that declared the prompt, or
   * `null` for the Default fallback.
   */
  source: string | null;
}

/**
 * One entry from `remargin prompt list <folder> --json`. Each
 * `.remargin.yaml` under the walked root that declares a
 * `system_prompt:` block surfaces as a row.
 */
export interface PromptListEntry {
  /** Absolute path of the folder containing the `.remargin.yaml`. */
  folder: string;
  /** `system_prompt.name` when present in the YAML, else `null`. */
  name: string | null;
  /** Verbatim prompt body. */
  prompt: string;
  /** Absolute path of the `.remargin.yaml` that declared the prompt. */
  source: string;
}

export type PluginPresence =
  | { kind: "absent" }
  | { kind: "installed_disabled" }
  | { kind: "installed_enabled" };
