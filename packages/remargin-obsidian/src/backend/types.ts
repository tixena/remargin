/** Option and response shapes for the backend's CLI calls. */

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

/** Options for `remargin comment`. */
export interface CommentOpts {
  replyTo?: string;
  afterLine?: number;
  afterComment?: string;
  to?: string[];
  attachments?: string[];
  autoAck?: boolean;
  /** Stages the file in the same atomic write, so comment and sandbox entry cannot diverge. */
  sandbox?: boolean;
}

/**
 * One entry from `remargin sandbox list --json`, tracking a markdown file that
 * the current identity has staged for a future Submit-to-Claude.
 */
export interface SandboxListEntry {
  path: string;
  since: string;
}

/** Filters for `remargin query`. */
export interface QueryOpts {
  pending?: boolean;
  pendingFor?: string;
  /** `--pending-for-me`: sugar for `pendingFor` with the CLI-resolved caller. */
  pendingForMe?: boolean;
  /** `--pending-broadcast`: `to:`-less comments the caller has not acked. */
  pendingBroadcast?: boolean;
  author?: string;
  since?: string;
  expanded?: boolean;
  commentId?: string;
  /** Runs after the metadata filters, so the regex only sees the already-filtered comments. */
  contentRegex?: string;
  ignoreCase?: boolean;
}

/** Line-window options for `remargin get`. */
export interface GetOpts {
  startLine?: number;
  endLine?: number;
  lineNumbers?: boolean;
}

/** Options for `remargin write`. */
export interface WriteOpts {
  create?: boolean;
  raw?: boolean;
}

/** Options for `remargin search`. */
export interface SearchOpts {
  path?: string;
  scope?: "all" | "body" | "comments";
  regex?: boolean;
  ignoreCase?: boolean;
  context?: number;
}

/** One comment in a `remargin batch`. */
export interface BatchCommentOp {
  content: string;
  replyTo?: string;
  afterLine?: number;
  afterComment?: string;
  to?: string[];
  autoAck?: boolean;
}

/** Response from `remargin identity --json`; `found` is false when no config resolved. */
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
  /** `null` when the resolution fell back to the default. */
  source: string | null;
}

/**
 * Response from `remargin prompt resolve <file> --json`. The prompt walk
 * is identity-free: a folder's prompt is a property of the directory
 * tree, not the caller. When no `.remargin.yaml` in the parent chain
 * declared a `system_prompt:` block, the CLI returns the Default
 * body with `is_default = true` and `source = null`.
 */
export interface ResolvedSystemPrompt {
  is_default: boolean;
  /** From the YAML `name:` field, else the owning folder's basename; `"default"` for the fallback. */
  name: string;
  prompt: string;
  /** `null` or absent means the caller's default runner. */
  runner?: string | null;
  /** `null` for the Default fallback. */
  source: string | null;
}

/**
 * One entry from `remargin prompt list <folder> --json`. Each
 * `.remargin.yaml` under the walked root that declares a
 * `system_prompt:` block surfaces as a row.
 */
export interface PromptListEntry {
  folder: string;
  name: string | null;
  prompt: string;
  runner?: string | null;
  source: string;
}

/** Whether the remargin Claude Code plugin is absent, installed but disabled, or enabled. */
export type PluginPresence =
  | { kind: "absent" }
  | { kind: "installed_disabled" }
  | { kind: "installed_enabled" };
