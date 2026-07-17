//! MCP server: stdio transport for tool integration.
//!
//! Implements the Model Context Protocol (MCP) over stdio transport using
//! JSON-RPC 2.0. Each tool maps 1:1 to a library function.

#[cfg(test)]
mod tests;

use std::io::{self, BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System;
use serde_json::{Map, Value, json};

use crate::activity;
use crate::config::identity::{IdentityFlags, resolve_identity_report};
use crate::config::permissions::resolve::{ResolvedPermissions, resolve_permissions};
use crate::config::system_prompt::resolve_system_prompt;
use crate::config::{ResolvedConfig, parse_author_type};
use crate::document;
use crate::document::get_image as image_ops;
use crate::kind::matches_kind_filter;
use crate::linter;
use crate::operations;
use crate::operations::batch::BatchCommentOp;
use crate::operations::cp as cp_op;
use crate::operations::mv as mv_op;
use crate::operations::plan as plan_ops;
use crate::operations::projections;
use crate::operations::prompt as prompt_ops;
use crate::operations::purge;
use crate::operations::query::{self, QueryFilter};
use crate::operations::replace;
use crate::operations::sandbox as sandbox_ops;
use crate::operations::search;
use crate::parser;
use crate::path::expand_path;
use crate::permissions::doctor as permissions_doctor;
use crate::permissions::inspect as permissions_inspect;
use crate::permissions::op_guard::{CallerInfo, check_against_resolved_for_caller};
use crate::responses;
use crate::writer::InsertPosition;

/// Standard JSON-RPC: invalid params.
const INVALID_PARAMS: i64 = -32602;

/// Standard JSON-RPC: method not found.
const METHOD_NOT_FOUND: i64 = -32601;

/// Standard JSON-RPC: parse error.
const PARSE_ERROR: i64 = -32700;

/// MCP protocol version supported by this server.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server name reported in the initialize response.
const SERVER_NAME: &str = "remargin";

/// Seed for a session's [`SessionState::spill_cap`], in UTF-8 bytes of
/// emitted tool-result text. This is a self-consistent proxy for the
/// client's token limit, NOT an equivalent — remargin measures bytes, the
/// client counts tokens. Seeded conservatively near Claude Code's ~10k-token
/// soft-warn boundary and well under its 25k-token default hard spill, so a
/// fresh session rarely spills before it learns; the cap only ratchets DOWN
/// from here via [`handle_report_spill`].
const DEFAULT_SPILL_CAP: usize = 40_000;

/// Fraction of [`SessionState::spill_cap`] held back for the response
/// envelope (matches wrapper, `total`, `effective_limit`, injected
/// `elapsed_ms`) and pretty-print indentation when sizing a search page.
const SPILL_MARGIN_DIVISOR: usize = 10;

/// Path-like top-level fields that every tool accepts. Each is run through
/// [`expand_path`] before the dispatch hands the params off to the per-tool
/// handler so `~` / `$VAR` behave identically to the CLI side. The list is
/// deliberately narrow — adding a new path-shaped field to an MCP schema is
/// a deliberate act, and it belongs here.
///
/// `config_path` and `key` are the per-tool identity-declaration fields
///: they are pre-expanded here so the identity resolver sees the
/// same already-canonical paths the CLI feeds to [`resolve_identity`].
const SCALAR_PATH_FIELDS: &[&str] = &["config_path", "dst", "file", "key", "path", "src"];

/// Array-valued path fields — each element is expanded independently.
const ARRAY_PATH_FIELDS: &[&str] = &["files", "attachments"];

/// Tools that take no path-shaped argument OR whose semantics
/// require querying paths outside the dispatch-time boundary.
/// Listed alphabetically.
///
/// `permissions_check` is the read-only inspection surface — it is
/// expected to answer "is this path restricted?" for any path the
/// caller hands in, even one outside `trusted_roots`. Gating it
/// would defeat its purpose. Other listed tools take no path at all.
///
/// `claude_restrict` and `claude_unrestrict` are intentionally absent
/// from the MCP surface: they mutate permission policy and must only
/// be invokable by the human via the CLI.
const NO_PATH_TOOLS: &[&str] = &[
    "doctor",
    "identity_create",
    "permissions_check",
    "permissions_show",
    "prompt_resolve",
    "report_spill",
    "whoami",
];

/// Tools whose handler defaults the target to MCP `cwd` when the
/// caller omits `path`. The dispatch-time boundary check synthesises
/// `"."` for these so an unconstrained `ls` (or `query`, `search`, …)
/// cannot read cwd when cwd is not in `trusted_roots`. `ack`'s
/// folder-walk fallback only fires when both `file` and `path` are
/// absent, so it is handled inline rather than via this list.
const PATH_DEFAULTS_TO_CWD_TOOLS: &[&str] = &["activity", "ls", "query", "sandbox_list", "search"];

/// Identity-declaration flags the MCP surface rejects. An MCP server
/// is launched per session with a stable startup identity, so per-call
/// identity flips are out of scope. The CLI keeps these flags.
const REJECTED_IDENTITY_FLAGS: &[&str] = &["config_path", "identity", "key", "type"];

/// Description of a single MCP tool.
struct ToolDesc {
    /// Human-readable description.
    description: &'static str,
    /// Tool name (short, no prefix).
    name: &'static str,
    /// JSON Schema for the tool's input parameters.
    schema: Value,
}

/// Structured MCP identity-flag rejection.
///
/// Hosts branch on `error_kind == "mcp_identity_flag_rejected"` (via
/// [`McpIdentityFlagRejected::to_json`]) to detect this class without
/// regex-matching the free-form message.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct McpIdentityFlagRejected {
    /// Offending parameter the caller sent.
    pub flag: String,
    /// MCP tool the caller invoked (short name, no prefix).
    pub tool: String,
}

/// Caller-owned scratch for [`build_plan_comment_request`]: the projected
/// `ProjectCommentParams` borrows these, so they must outlive the request.
/// `attach_refs` is kept separate (it borrows `attach_names`).
struct PlanCommentStaging {
    attach_names: Vec<String>,
    position: InsertPosition,
    reply_to_owned: Option<String>,
    to_owned: Vec<String>,
}

/// Per-session adaptive state owned by the stdio run loop.
///
/// `spill_cap` is a learned size ceiling in remargin's own unit (UTF-8 bytes
/// of emitted tool-result text) — a self-consistent proxy for the client's
/// token limit, not an equivalent. It only ratchets DOWN (from an
/// agent-reported spill) and is seeded fresh each session with no disk
/// persistence, so a session re-learns rather than carrying a stale ceiling.
/// `last_response_size` is the byte size of the most recently emitted tool
/// result — what `report_spill` infers the offending size from.
#[derive(Debug)]
struct SessionState {
    last_response_size: usize,
    spill_cap: usize,
}

impl Default for PlanCommentStaging {
    fn default() -> Self {
        Self {
            attach_names: Vec::new(),
            position: InsertPosition::Append,
            reply_to_owned: None,
            to_owned: Vec::new(),
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            last_response_size: 0,
            spill_cap: DEFAULT_SPILL_CAP,
        }
    }
}

impl McpIdentityFlagRejected {
    /// One-line plain-English summary.
    #[must_use]
    pub fn headline(&self) -> String {
        format!(
            "identity flag '{}' is not supported on MCP tool '{}'; use the CLI for per-call \
             identity projection",
            self.flag, self.tool,
        )
    }

    /// JSON shape used by [`tool_result_error_json`].
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "error_kind": "mcp_identity_flag_rejected",
            "flag": self.flag,
            "headline": self.headline(),
            "tool": self.tool,
        })
    }
}

/// Reject any identity-declaration flag in `params`. Returns the
/// structured rejection on the first hit. Defense against clients
/// that ignore the schema (which no longer advertises these flags).
fn reject_identity_flags(
    tool: &str,
    params: &Map<String, Value>,
) -> Option<McpIdentityFlagRejected> {
    // `identity_create` is exempt: its `identity` / `type` / `key`
    // params name the NEW identity being rendered, not a per-call
    // re-declaration of the caller's identity.
    if tool == "identity_create" {
        return None;
    }
    for &flag in REJECTED_IDENTITY_FLAGS {
        if params.contains_key(flag) {
            return Some(McpIdentityFlagRejected {
                tool: String::from(tool),
                flag: String::from(flag),
            });
        }
    }
    None
}

/// Build the `activity` tool descriptor.
fn desc_activity() -> ToolDesc {
    ToolDesc {
        name: "activity",
        description: "Call this BEFORE processing pending comments on a file or workspace you \
             haven't acted on recently - it surfaces new comments, reactions, acks, and sandbox \
             adds you'd otherwise miss. Walks <path> (file or directory; defaults to the MCP \
             server's working directory) and returns per-file change records (comments, acks, \
             sandbox-adds) sorted by ts. With `since` omitted, the per-file cutoff is the \
             caller's last action in that file; files where the caller has never acted return \
             everything.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File or directory to scan; defaults to the MCP server's working directory." },
                "since": { "type": "string", "description": "ISO 8601 cutoff. Omit to derive per-file from caller's last action." }
            }
        }),
    }
}

/// Build the ack tool descriptor.
fn desc_ack() -> ToolDesc {
    ToolDesc {
        name: "ack",
        description: "Acknowledge one or more comments (or remove this identity's ack with remove=true). Omit file to resolve by ID across the folder tree.",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document (omit to resolve by ID across the folder tree)" },
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Comment IDs to acknowledge"
                },
                "path": { "type": "string", "description": "Base directory to search when resolving by ID (default: .)", "default": "." },
                "remove": { "type": "boolean", "description": "Remove this identity's ack instead of adding one", "default": false }
            },
            "required": ["ids"]
        }),
    }
}

/// Build the batch tool descriptor.
fn desc_batch() -> ToolDesc {
    ToolDesc {
        name: "batch",
        description: "PREFERRED for any time you'll post more than one comment on a file. Atomic \
             across the set; correctly tracks line shifts between insertions. Each sub-op has \
             the same fields as a single `comment`. Comments are rendered as markdown; record \
             observed state, not future-tense announcements.",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string", "description": "Comment body text in markdown (bold/italic/links/code-blocks/lists render). Record observed or completed state - not future-tense announcements (\"I'll do X\"). Write it the way you'd want to re-read it in three months." },
                            "to": { "type": "array", "items": { "type": "string" }, "description": "Names from the registry who must see this in their pending queue. Omit for a non-actionable note. Use a parent author's name only when your reply requires their response - replies do NOT auto-include the parent's author.", "default": [] },
                            "reply_to": { "type": "string" },
                            "attachments": { "type": "array", "items": { "type": "string" }, "default": [] },
                            "after_line": { "type": "integer" },
                            "after_comment": { "type": "string" },
                            "after_heading": { "type": "string", "description": "ATX heading path; resolved at write time. Mutually exclusive with after_line/after_comment." },
                            "auto_ack": { "type": "boolean", "description": "Acknowledge the parent comment when replying. If omitted, the parent is auto-acked iff its author differs from the caller (replies to your own comment don't auto-ack). Pass true to force the ack, false to skip it." },
                            "ack_skip_reason": { "type": "string", "description": "Required when auto_ack:false skips acking another author's comment: explain why you are not acknowledging it. Not needed for self-replies or the smart default." }
                        },
                        "required": ["content"]
                    },
                    "description": "List of comment operations"
                }
            },
            "required": ["file", "operations"]
        }),
    }
}

/// Build the cp tool descriptor.
fn desc_cp() -> ToolDesc {
    ToolDesc {
        name: "cp",
        description: "Copy a tracked file. The SOURCE is never modified. Non-markdown and \
             comment-free markdown copy verbatim; a comment-bearing markdown file is copied \
             BODY-ONLY (the duplicate carries no comment blocks) so it introduces no duplicate \
             comment IDs and no broken signatures. Reports kind + comments_dropped. Use plan \
             op=cp to preview.",
        schema: json!({
            "type": "object",
            "properties": {
                "src": { "type": "string", "description": "Source path." },
                "dst": { "type": "string", "description": "Destination path." },
                "force": { "type": "boolean", "description": "Overwrite destination if it exists.", "default": false }
            },
            "required": ["src", "dst"]
        }),
    }
}

/// Build the comment tool descriptor.
fn desc_comment() -> ToolDesc {
    ToolDesc {
        name: "comment",
        description: "Create a single comment in a document. For two or more comments on the \
             same file in one turn, use `batch` - `comment` doesn't track line shifts between \
             insertions, so a loop of `comment` calls will misplace later entries. Use `reply` \
             (not this tool) when responding to an existing comment. Comments record observed/\
             done state and are rendered as markdown; do not post future-tense announcements \
             (\"I'll do X\").",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "content": { "type": "string", "description": "Comment body text in markdown (bold/italic/links/code-blocks/lists render). Record observed or completed state - not future-tense announcements (\"I'll do X\"). Write it the way you'd want to re-read it in three months." },
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Names from the registry who must see this in their pending queue. Omit for a non-actionable note. Use a parent author's name only when your reply requires their response - replies do NOT auto-include the parent's author.",
                    "default": []
                },
                "reply_to": { "type": "string", "description": "ID of the comment to reply to" },
                "auto_ack": { "type": "boolean", "description": "Acknowledge the parent comment when replying. If omitted, the parent is auto-acked iff its author differs from the caller (replies to your own comment don't auto-ack). Pass true to force the ack, false to skip it." },
                "ack_skip_reason": { "type": "string", "description": "Required when auto_ack:false skips acking another author's comment: explain why you are not acknowledging it. Not needed for self-replies or the smart default." },
                "attachments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File paths to attach",
                    "default": []
                },
                "after_line": { "type": "integer", "description": "Insert after this line number (1-indexed)" },
                "after_comment": { "type": "string", "description": "Insert after this comment ID" },
                "after_heading": { "type": "string", "description": "Insert after the ATX heading addressed by this `>`-separated path. Mutually exclusive with after_line/after_comment." },
                "sandbox": { "type": "boolean", "description": "Atomically stage the file in the caller's sandbox (see sandbox_add)", "default": false },
                "remargin_kind": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Classification tags. Each entry must match [A-Za-z0-9_ \\-]{1,15}; at most 8 entries.",
                    "default": []
                }
            },
            "required": ["file", "content"]
        }),
    }
}

/// Build the comments tool descriptor.
fn desc_comments() -> ToolDesc {
    ToolDesc {
        name: "comments",
        description: "List comments in a document",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "remargin_kind": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "OR-semantics filter: only return comments whose remargin_kind contains at least one of these values. Empty = no filter.",
                    "default": []
                }
            },
            "required": ["file"]
        }),
    }
}

/// Build the delete tool descriptor.
fn desc_delete() -> ToolDesc {
    ToolDesc {
        name: "delete",
        description: "Delete one or more comments",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Comment IDs to delete"
                }
            },
            "required": ["file", "ids"]
        }),
    }
}

/// Build the doctor tool descriptor.
fn desc_doctor() -> ToolDesc {
    ToolDesc {
        name: "doctor",
        description: "Run health checks on the remargin permission stack. \
                       Checks (in order): (1) hook-installed - verifies the \
                       PreToolUse hook is wired into Claude settings; when \
                       absent from both scopes, all other checks are skipped. \
                       Returns a structured DoctorReport.",
        schema: json!({
            "type": "object",
            "properties": {
                "user_settings_file": {
                    "type": "string",
                    "description": "Path to the user-scope Claude settings file \
                                    (default: ~/.claude/settings.json)."
                }
            },
            "required": []
        }),
    }
}

/// Build the edit tool descriptor.
fn desc_edit() -> ToolDesc {
    ToolDesc {
        name: "edit",
        description: "Edit a comment (cascading ack clear)",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "id": { "type": "string", "description": "Comment ID to edit" },
                "content": { "type": "string", "description": "New comment body" },
                "remargin_kind": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Replacement classification tags. Omit to preserve the stored list; pass [] to clear. Each entry must match [A-Za-z0-9_ \\-]{1,15}; at most 8 entries."
                }
            },
            "required": ["file", "id", "content"]
        }),
    }
}

/// Build the get tool descriptor.
fn desc_get() -> ToolDesc {
    ToolDesc {
        name: "get",
        description: "Read a file's contents. Text mode (default) returns \
             UTF-8 content. Pass `binary: true` to fetch non-markdown files \
             as bytes; returns `{binary, content (base64), mime, path, \
             size_bytes}`. Rejects `.md` in binary mode. Run `metadata` first \
             to check size before pulling large blobs.",
        schema: json!({
            "type": "object",
            "properties": {
                "binary": { "type": "boolean", "description": "Return raw bytes base64-encoded; rejects .md", "default": false },
                "end_line": { "type": "integer", "description": "End line (1-indexed). Text mode only." },
                "line_numbers": { "type": "boolean", "description": "Prefix each line with its 1-indexed line number. Text mode only.", "default": false },
                "path": { "type": "string", "description": "Path to the file" },
                "start_line": { "type": "integer", "description": "Start line (1-indexed). Text mode only." }
            },
            "required": ["path"]
        }),
    }
}

/// Build the lint tool descriptor.
fn desc_lint() -> ToolDesc {
    ToolDesc {
        name: "lint",
        description: "Run structural lint checks",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" }
            },
            "required": ["file"]
        }),
    }
}

/// Build the ls tool descriptor.
fn desc_ls() -> ToolDesc {
    ToolDesc {
        name: "ls",
        description: "List files and directories",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path to list", "default": "." }
            },
            "required": []
        }),
    }
}

/// Build the metadata tool descriptor.
fn desc_metadata() -> ToolDesc {
    ToolDesc {
        name: "metadata",
        description: "Get file metadata. Always returns size_bytes, mime, \
             binary, path. Markdown files additionally return comment_count, \
             line_count, pending_count, pending_for, last_activity, \
             frontmatter. Use before `get --binary` to avoid pulling large \
             blobs.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file (any allowlisted extension)" }
            },
            "required": ["path"]
        }),
    }
}

/// Build the `mv` tool descriptor.
fn desc_mv() -> ToolDesc {
    ToolDesc {
        name: "mv",
        description: "Move or rename a tracked file or directory. Auto-detects a directory source and renames the directory + every nested file as an atomic unit; comments / threads / acks survive because the path of every nested file changes consistently. Atomic same-FS rename, copy+remove fallback on cross-filesystem (EXDEV) for files. Both endpoints flow through the same sandbox / forbidden-target / `trusted_roots` checks every other mutating op uses. Idempotent: same-path no-op; src missing AND dst already in place returns success with bytes_moved=0.",
        schema: json!({
            "type": "object",
            "properties": {
                "src": { "type": "string", "description": "Source path (file or directory)." },
                "dst": { "type": "string", "description": "Destination path." },
                "force": { "type": "boolean", "description": "Overwrite an existing destination (for directories: removes the existing destination subtree before the rename).", "default": false }
            },
            "required": ["src", "dst"]
        }),
    }
}

/// Build the plan tool descriptor.
fn desc_plan() -> ToolDesc {
    ToolDesc {
        name: "plan",
        description: "Dry-run projection for mutating ops. Returns a PlanReport (noop/would_commit/reject_reason/subset_gate/checksums/changed_line_ranges/comment diff) without touching disk. Document ops: ack, batch, comment, reply, delete, edit, purge, react, sandbox-add, sandbox-remove, sign, write. `reply` is a synonym for `comment` with required `parent_id` (translated to `reply_to`). The subset_gate field mirrors SubsetGateFailure when the projected op would introduce a new anomaly not present in the on-disk pre-state - the same shape commit_with_verify would return. File-relocation op: mv - surfaces an `mv_diff` describing canonical src/dst, dst_exists, noop_same_path, idempotent_already_settled. File-copy op: cp - surfaces a `cp_diff` describing canonical src/dst, dst_exists, kind (verbatim/body_only/noop), and comments_to_drop. Config ops (claude_restrict / claude_unrestrict) are CLI-only - use `remargin plan claude restrict` / `remargin plan claude unrestrict`.",
        schema: json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "description": "Op to project: ack | batch | comment | reply | delete | edit | react | cp | mv | purge | sandbox-add | sandbox-remove | sign | write" },
                "file": { "type": "string", "description": "Path to the document (required for wired document ops)" },
                "src": { "type": "string", "description": "For mv: source path." },
                "dst": { "type": "string", "description": "For mv: destination path." },
                "force": { "type": "boolean", "description": "For mv: project the --force overwrite semantics.", "default": false },
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Comment IDs (used by ack / delete / sign)"
                },
                "all_mine": { "type": "boolean", "description": "For sign: project signing every unsigned comment authored by the current identity. Mutually exclusive with `ids`.", "default": false },
                "id": { "type": "string", "description": "Single comment ID (used by edit / react)" },
                "content": { "type": "string", "description": "Body text (used by comment / edit)" },
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Addressees (used by comment)"
                },
                "reply_to": { "type": "string", "description": "Parent comment ID (used by comment)" },
                "after_comment": { "type": "string", "description": "Insert after this comment ID (used by comment)" },
                "after_heading": { "type": "string", "description": "Insert after the ATX heading addressed by this `>`-separated path (used by comment)" },
                "after_line": { "type": "integer", "description": "Insert after this 1-indexed line (used by comment)" },
                "attach_names": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Attachment basenames to record on the projected comment. Bytes are NOT copied by plan."
                },
                "auto_ack": { "type": "boolean", "description": "For comment replies: auto-ack the parent. If omitted, the parent is auto-acked iff its author differs from the caller. Pass true to force the ack, false to skip it." },
                "ack_skip_reason": { "type": "string", "description": "For comment/reply: required when auto_ack:false skips acking another author's comment. Mirrors the live reply gate." },
                "sandbox": { "type": "boolean", "description": "For comment: atomically project a sandbox entry", "default": false },
                "emoji": { "type": "string", "description": "Emoji for react op" },
                "remove": { "type": "boolean", "description": "For ack / react: remove instead of add", "default": false },
                "ops": {
                    "type": "array",
                    "description": "Sub-ops for the batch projection. Each entry has the same shape as a `batch` sub-op: content (required), reply_to, after_comment, after_heading, after_line, attach_names, auto_ack, ack_skip_reason, to.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "reply_to": { "type": "string" },
                            "after_comment": { "type": "string" },
                            "after_heading": { "type": "string" },
                            "after_line": { "type": "integer" },
                            "attach_names": { "type": "array", "items": { "type": "string" } },
                            "auto_ack": { "type": "boolean" },
                            "ack_skip_reason": { "type": "string" },
                            "to": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["content"]
                    }
                },
                "binary": { "type": "boolean", "description": "For write: content is base64 binary (implies raw)", "default": false },
                "create": { "type": "boolean", "description": "For write: create the file if missing", "default": false },
                "lines": { "type": "string", "description": "For write: partial-write line range as START-END (1-indexed inclusive)" },
                "raw": { "type": "boolean", "description": "For write: treat content as raw bytes; plan returns a non-Markdown reject_reason", "default": false }
            },
            "required": ["op"]
        }),
    }
}

/// Build the purge tool descriptor.
fn desc_purge() -> ToolDesc {
    ToolDesc {
        name: "purge",
        description: "Strip all comments from a document, or every .md file in a directory when `recursive` is true. To preview without writing, use `plan` with op=\"purge\".",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document (or directory when `recursive` is true)" },
                "recursive": { "type": "boolean", "description": "Recursively purge every visible .md file under `file` (treats `file` as a directory)." }
            },
            "required": ["file"]
        }),
    }
}

/// Build the `prompt_resolve` tool descriptor.
fn desc_prompt_resolve() -> ToolDesc {
    ToolDesc {
        name: "prompt_resolve",
        description: "Resolve the nearest folder-scoped system prompt for a file or directory. \
             Walks `.remargin.yaml` files upward; first one declaring a `system_prompt:` block \
             wins. Falls through to a locked Default body when the walk exhausts. Read-only.",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the file (or directory) to resolve a prompt for." }
            },
            "required": ["file"]
        }),
    }
}

/// Build the `prompt_set` tool descriptor.
fn desc_prompt_set() -> ToolDesc {
    ToolDesc {
        name: "prompt_set",
        description: "Create or replace the `system_prompt:` block in `<folder>/.remargin.yaml`. \
             Other fields in the file are preserved byte-for-byte. The post-write diff refuses \
             any change outside the system_prompt mapping. Folder defaults to the MCP root when \
             omitted.",
        schema: json!({
            "type": "object",
            "properties": {
                "folder": { "type": "string", "description": "Folder containing the `.remargin.yaml`. Defaults to the MCP root." },
                "name":   { "type": "string", "description": "Human-readable display label. Required." },
                "prompt": { "type": "string", "description": "Prompt body text. Required." }
            },
            "required": ["name", "prompt"]
        }),
    }
}

/// Build the `prompt_delete` tool descriptor.
fn desc_prompt_delete() -> ToolDesc {
    ToolDesc {
        name: "prompt_delete",
        description: "Strip the `system_prompt:` block from `<folder>/.remargin.yaml`. Idempotent: \
             a missing block (or missing file) succeeds. The .remargin.yaml is preserved even if \
             it ends up empty. Folder defaults to the MCP root.",
        schema: json!({
            "type": "object",
            "properties": {
                "folder": { "type": "string", "description": "Folder containing the `.remargin.yaml`. Defaults to the MCP root." }
            },
            "required": []
        }),
    }
}

/// Build the `prompt_list` tool descriptor.
fn desc_prompt_list() -> ToolDesc {
    ToolDesc {
        name: "prompt_list",
        description: "Recursively list every `.remargin.yaml` under `folder` that declares a \
             `system_prompt:` block. Read-only. Folder defaults to the MCP root.",
        schema: json!({
            "type": "object",
            "properties": {
                "folder": { "type": "string", "description": "Root folder. Defaults to the MCP root." }
            },
            "required": []
        }),
    }
}

/// Build the `identity_create` tool descriptor.
///
/// Mirrors the CLI `remargin identity create` surface: prints a
/// ready-to-use identity YAML block. `mode:` is deliberately omitted
/// (tree property, resolved by walk). No `--write` equivalent — MCP
/// writes to `.remargin.yaml` are banned.
fn desc_identity_create() -> ToolDesc {
    ToolDesc {
        name: "identity_create",
        description: "Render a ready-to-use identity YAML block for `.remargin.yaml`. \
             Returns the YAML text; the caller writes it themselves. `mode:` \
             is never emitted (mode is a tree property, not identity-scoped).",
        schema: json!({
            "type": "object",
            "properties": {
                "identity": { "type": "string", "description": "Identity (author name) to record." },
                "type": { "type": "string", "enum": ["human", "agent"], "description": "Author type." },
                "key": { "type": "string", "description": "Optional path to the signing key (emitted verbatim; no existence check)." }
            },
            "required": ["identity", "type"]
        }),
    }
}

/// Build the query tool descriptor.
fn desc_query() -> ToolDesc {
    ToolDesc {
        name: "query",
        description: "Search across documents for comments. \
             Pending filters (`pending`, `pending_for`, `pending_for_me`, `pending_broadcast`) \
             compose as a union when more than one is set. `pending` (broad form) includes \
             both directed comments with unacked recipients AND broadcast (no-`to`) comments \
             that nobody has acked. `pending_for_me` and `pending_broadcast` use the MCP \
             server's configured identity.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Base directory to search", "default": "." },
                "comment_id": { "type": "string", "description": "Only documents containing a comment with this structural ID" },
                "content_regex": { "type": "string", "description": "Regex applied to comment content; composes with metadata filters" },
                "expanded": { "type": "boolean", "description": "Include individual matching comments in each result (default: true via non-summary mode)", "default": false },
                "ignore_case": { "type": "boolean", "description": "Case-insensitive match for content_regex", "default": false },
                "pending": { "type": "boolean", "description": "Only documents with pending (unacked) comments. Matches both directed and broadcast shapes.", "default": false },
                "pending_broadcast": { "type": "boolean", "description": "Only surface broadcast (no-`to`) comments the server identity has not acked yet.", "default": false },
                "pending_for": { "type": "string", "description": "Only pending for this recipient" },
                "pending_for_me": { "type": "boolean", "description": "Sugar for pending_for=<server identity>. Surfaces directed comments addressed to the caller and not yet acked.", "default": false },
                "author": { "type": "string", "description": "Only documents with comments by this author" },
                "remargin_kind": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "OR-semantics filter: only surface comments whose remargin_kind contains at least one of these values. Empty = no filter.",
                    "default": []
                },
                "since": { "type": "string", "description": "Only activity after this ISO 8601 timestamp" },
                "summary": { "type": "boolean", "description": "Return only counts/summary, suppress comment data", "default": false }
            },
            "required": []
        }),
    }
}

/// Build the react tool descriptor.
fn desc_react() -> ToolDesc {
    ToolDesc {
        name: "react",
        description: "Add or remove an emoji reaction",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "id": { "type": "string", "description": "Comment ID" },
                "emoji": { "type": "string", "description": "Emoji to add/remove" },
                "remove": { "type": "boolean", "description": "Remove instead of add", "default": false }
            },
            "required": ["file", "id", "emoji"]
        }),
    }
}

/// Build the reply tool descriptor.
fn desc_reply() -> ToolDesc {
    ToolDesc {
        name: "reply",
        description: "PREFERRED way to respond to a comment. Wraps `comment` with required \
             `parent_id`; the parent must live in the named `file`. Smart auto-ack default: if \
             `auto_ack` is omitted, the parent is acked iff its author differs from the caller \
             (replies to your own comments don't ack). Set `auto_ack: true` to force the ack, \
             `auto_ack: false` to skip it. Comments render as markdown; record observed state, \
             not future-tense announcements.",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document containing the parent comment" },
                "parent_id": { "type": "string", "description": "ID of the comment you're replying to. Must exist in `file`." },
                "content": { "type": "string", "description": "Reply body text in markdown. Past-tense / observed-state only - no future-tense promises." },
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional recipients beyond the parent's author. NOT auto-populated with the parent's author - be explicit if you want them paged.",
                    "default": []
                },
                "auto_ack": {
                    "type": "boolean",
                    "description": "Force the smart default off. true = always ack the parent; false = never ack. Omit for the smart default: ack iff parent.author differs from caller."
                },
                "ack_skip_reason": {
                    "type": "string",
                    "description": "Required when auto_ack:false skips acking another author's comment: explain why you are not acknowledging it. Not needed for self-replies or the smart default."
                },
                "attachments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File paths to attach",
                    "default": []
                },
                "sandbox": {
                    "type": "boolean",
                    "description": "Atomically stage the file in the caller's sandbox (see sandbox_add)",
                    "default": false
                },
                "remargin_kind": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Classification tags. Each entry must match [A-Za-z0-9_ \\-]{1,15}; at most 8 entries.",
                    "default": []
                }
            },
            "required": ["file", "parent_id", "content"]
        }),
    }
}

/// Build the `report_spill` tool descriptor.
fn desc_report_spill() -> ToolDesc {
    ToolDesc {
        name: "report_spill",
        description: "Call this the moment your client spills a remargin tool result to a file \
             (because it exceeded the client's output-token limit) - BEFORE you read that file. \
             remargin infers the offending size from the last result it emitted and ratchets its \
             per-session page cap down so future `search` pages stay under your client's limit. \
             The cap only ever falls, never rises, and resets each session. Omit `size` for the \
             inferred value; pass it only to force an explicit one.",
        schema: json!({
            "type": "object",
            "properties": {
                "size": { "type": "integer", "minimum": 0, "description": "Optional explicit size (bytes) of the spilled result. Omit to let remargin infer it from the last emitted result." }
            }
        }),
    }
}

/// Build the rm tool descriptor.
fn desc_rm() -> ToolDesc {
    ToolDesc {
        name: "rm",
        description: "Remove a file from the managed document tree (idempotent). \
             Pointed at a directory it removes the tree recursively (always \
             recursive, no flag): it deletes everything remargin can see \
             bottom-up, leaves behind only directories holding entries it \
             cannot list (hidden files, a nested .remargin.yaml), and returns a \
             report {files_deleted, folders_removed, folders_left_behind}. If \
             any listed resource is unreadable the call fails and nothing is \
             deleted. A markdown file carrying one or more comments is refused \
             (single file or anywhere in a directory tree): purge the path \
             first to strip its comments, then rm.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file or directory to delete" }
            },
            "required": ["path"]
        }),
    }
}

fn desc_get_image() -> ToolDesc {
    ToolDesc {
        name: "get_image",
        description: "Return a downscaled / cropped raster image sized to fit \
             the MCP token budget. Use this when `get --binary` would exceed \
             the inline limit. Accepts PNG / JPEG / GIF / WebP; rejects SVG, \
             PDF, audio, video. Returns `{binary, content (base64), mime, \
             format, width, height, size_bytes, source}` where `source` echoes \
             the original mime / dimensions / size. Defaults: max_dimension=1024, \
             max_bytes=262144 (256 KiB), format=jpeg for photographic source \
             formats / png for lossless ones.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the image attachment." },
                "crop": {
                    "type": "string",
                    "description": "Optional pixel crop applied before scaling, formatted X,Y,W,H (origin top-left). Clamped to the image bounds; an origin outside the image is rejected."
                },
                "max_dimension": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Upper bound (in pixels) on the longer edge of the output. Defaults to 1024.",
                    "default": 1024
                },
                "max_bytes": {
                    "type": "integer",
                    "minimum": 1024,
                    "description": "Target ceiling on the encoded output size in bytes. JPEG quality is stepped down (and then the dimension cap halved) until this fits. Defaults to 256 KiB.",
                    "default": 256 * 1024
                },
                "format": {
                    "type": "string",
                    "enum": ["jpeg", "jpg", "png"],
                    "description": "Output format. Defaults to jpeg for photographic source images (JPEG / WebP) and png for lossless source images (PNG / GIF)."
                }
            },
            "required": ["path"]
        }),
    }
}

/// Build the search tool descriptor.
fn desc_search() -> ToolDesc {
    ToolDesc {
        name: "search",
        description: "Search across documents for text matches",
        schema: json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Text or regex pattern to search for" },
                "path": { "type": "string", "description": "Base directory to search", "default": "." },
                "regex": { "type": "boolean", "description": "Treat pattern as a regex", "default": false },
                "scope": { "type": "string", "description": "Search scope: all, body, or comments", "default": "all" },
                "context": { "type": "integer", "description": "Lines of context around matches", "default": 0 },
                "ignore_case": { "type": "boolean", "description": "Case-insensitive matching", "default": false },
                "limit": { "type": "integer", "minimum": 1, "description": "Page size: return at most this many matches. Omit for all matches. The response always carries the exact total, so a bounded page is never a silent truncation." },
                "offset": { "type": "integer", "minimum": 0, "description": "Number of matches to skip before the returned page", "default": 0 }
            },
            "required": ["pattern"]
        }),
    }
}

/// Build the replace tool descriptor.
fn desc_replace() -> ToolDesc {
    ToolDesc {
        name: "replace",
        description: "Find/replace across document BODY text only (never inside comments), over a \
             file or folder. Integrity-gated: each file flows through comment-preservation plus \
             the post-verify subset gate, so a comment can never be corrupted. A pattern that \
             occurs only inside comments is a no-op. In a folder, every visible `.md` file is \
             rewritten; a file the gate refuses is skipped, recorded, and the run continues. \
             `path` is required (no silent cwd fan-out). To preview without writing, use the CLI \
             `remargin replace --dry-run`.",
        schema: json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Text or regex to find" },
                "replacement": { "type": "string", "description": "Replacement text ($1/${name} in regex mode; literal otherwise)" },
                "path": { "type": "string", "description": "Target file or directory (required)" },
                "regex": { "type": "boolean", "description": "Treat pattern as a regex", "default": false },
                "ignore_case": { "type": "boolean", "description": "Case-insensitive matching", "default": false }
            },
            "required": ["pattern", "replacement", "path"]
        }),
    }
}

/// Build the sign tool descriptor.
fn desc_sign() -> ToolDesc {
    ToolDesc {
        name: "sign",
        description: "Back-sign missing-signature comments authored by the current identity. \
             Refuses to sign comments authored by anyone else (forgery guard). \
             Already-signed comments listed under `ids` are reported as skipped; \
             `all_mine` silently excludes them. Pass exactly one of `ids` or `all_mine`. \
             Pass `repair_checksum: true` to recompute the target comment's \
             checksum from its current bytes before signing (useful when the \
             content was edited out-of-band and the signer wants to re-vouch \
             for the updated content). To preview without writing, use `plan` \
             with op=\"sign\".",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Comment ids to sign. Mutually exclusive with all_mine."
                },
                "all_mine": { "type": "boolean", "description": "Sign every unsigned comment authored by the current identity.", "default": false },
                "repair_checksum": { "type": "boolean", "description": "Recompute each target comment's checksum from its current content before signing. Scoped by the forgery guard: only the caller's own comments can be repaired.", "default": false }
            },
            "required": ["file"]
        }),
    }
}

/// Build the verify tool descriptor.
fn desc_verify() -> ToolDesc {
    ToolDesc {
        name: "verify",
        description: "Verify comment integrity (checksums and signatures) against the participant registry. \
             Accepts a file or a directory: a directory walks every visible `.md` file recursively \
             (honoring `.gitignore`) and returns a failures-only summary \
             {ok, files_verified, files_passed, failures}: passing files are counted, never enumerated; \
             each failing file lists only its not-clean rows (or a parse/read `error`). \
             A file returns per-comment status plus an aggregate `ok` flag driven by the active mode.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to a document or a directory to verify recursively." },
                "file": { "type": "string", "description": "Backward-compatible alias for `path` (single document). Prefer `path`." }
            },
            "required": ["path"]
        }),
    }
}

/// Build the `whoami` tool descriptor.
fn desc_whoami() -> ToolDesc {
    ToolDesc {
        name: "whoami",
        description: "Return the MCP server's startup-resolved identity. \
             Returns `{found: false}` when no identity is configured \
             (soft miss; never errors on missing config).",
        schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    }
}

/// Build the `permissions_show` tool descriptor.
fn desc_permissions_show() -> ToolDesc {
    ToolDesc {
        name: "permissions_show",
        description: "Print the resolved permissions for the MCP server's working directory \
             (parent-walk of `.remargin.yaml`). Includes recursive expansion of \
             `trusted_roots` that are themselves realms (bounded depth, cycle-safe). \
             Read-only; no identity flags.",
        schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

/// Build the `permissions_check` tool descriptor.
fn desc_permissions_check() -> ToolDesc {
    ToolDesc {
        name: "permissions_check",
        description: "Gitignore-style: returns `restricted=true` when the path is outside \
             the `trusted_roots` allow-list or covered by a `deny_ops` rule from the \
             parent-walked `.remargin.yaml`. With `why=true`, the matching rule's kind, \
             source file, and rule text are included. Read-only; no identity flags.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or relative path to test." },
                "why": { "type": "boolean", "description": "Include the matching rule details when restricted.", "default": false }
            },
            "required": ["path"]
        }),
    }
}

/// Build the `sandbox_add` tool descriptor.
fn desc_sandbox_add() -> ToolDesc {
    ToolDesc {
        name: "sandbox_add",
        description: "Stage one or more markdown files in the caller's sandbox. Idempotent per identity.",
        schema: json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Paths to markdown files to stage"
                }
            },
            "required": ["files"]
        }),
    }
}

/// Build the `sandbox_remove` tool descriptor.
fn desc_sandbox_remove() -> ToolDesc {
    ToolDesc {
        name: "sandbox_remove",
        description: "Remove the caller's sandbox entry from one or more markdown files. Idempotent per identity.",
        schema: json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Paths to markdown files to unstage"
                }
            },
            "required": ["files"]
        }),
    }
}

/// Build the `sandbox_list` tool descriptor.
fn desc_sandbox_list() -> ToolDesc {
    ToolDesc {
        name: "sandbox_list",
        description: "Return all markdown files in the given path that are staged for the caller's identity.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Base directory to walk (default: .)", "default": "." }
            }
        }),
    }
}

/// Build the write tool descriptor.
fn desc_write() -> ToolDesc {
    ToolDesc {
        name: "write",
        description: "Replace whole file OR a contiguous line range (via start_line/end_line - \
             1-indexed inclusive). For markdown files, existing remargin comment blocks are \
             preserved automatically - do not pre-strip them, and do not regenerate the whole \
             file when only a section changed. Pass start_line and end_line together for \
             surgical edits.",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file" },
                "content": { "type": "string", "description": "File content to write (base64-encoded when binary=true)" },
                "binary": { "type": "boolean", "description": "Content is base64-encoded binary data. Implies raw mode. Not supported for markdown (.md) files.", "default": false },
                "create": { "type": "boolean", "description": "Create a new file, creating any missing parent directories (within the sandbox); the file itself must not already exist", "default": false },
                "start_line": { "type": "integer", "minimum": 1, "description": "Partial write: first line of the range to replace (1-indexed, inclusive). Must be paired with end_line. Incompatible with create/raw/binary." },
                "end_line": { "type": "integer", "minimum": 1, "description": "Partial write: last line of the range to replace (1-indexed, inclusive). Must be paired with start_line. Incompatible with create/raw/binary." },
                "raw": { "type": "boolean", "description": "Write content exactly as provided, skipping frontmatter injection and comment preservation. Not supported for markdown (.md) files.", "default": false }
            },
            "required": ["path", "content"]
        }),
    }
}

/// Build the list of all tool descriptors.
fn tool_descriptors() -> Vec<ToolDesc> {
    vec![
        desc_ack(),
        desc_activity(),
        desc_batch(),
        desc_comment(),
        desc_comments(),
        desc_cp(),
        desc_delete(),
        desc_doctor(),
        desc_edit(),
        desc_get(),
        desc_get_image(),
        desc_identity_create(),
        desc_lint(),
        desc_ls(),
        desc_metadata(),
        desc_mv(),
        desc_permissions_check(),
        desc_permissions_show(),
        desc_plan(),
        desc_prompt_delete(),
        desc_prompt_list(),
        desc_prompt_resolve(),
        desc_prompt_set(),
        desc_purge(),
        desc_query(),
        desc_react(),
        desc_replace(),
        desc_reply(),
        desc_report_spill(),
        desc_rm(),
        desc_sandbox_add(),
        desc_sandbox_list(),
        desc_sandbox_remove(),
        desc_search(),
        desc_sign(),
        desc_verify(),
        desc_whoami(),
        desc_write(),
    ]
}

/// Build a JSON-RPC success response.
fn success_response(id: &Value, result: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

/// Build a JSON-RPC error response.
fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

/// Build an MCP tool result (success).
fn tool_result_success(content: &Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(content).unwrap_or_default()
        }]
    })
}

/// Build an MCP tool result (error).
fn tool_result_error(message: &str) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": message
        }],
        "isError": true
    })
}

/// Build an MCP tool result (error) whose `text` field is a JSON-stringified
/// payload. Used for typed errors (e.g. verify-gate failures) so callers
/// can branch on `error_kind` instead of regex-matching a free-form string.
/// The caller provides the structured value; we serialize it so the
/// outer `tools/call` `elapsed_ms` injector can re-parse and decorate it.
fn tool_result_error_json(payload: &Value) -> Value {
    let text = serde_json::to_string_pretty(payload).unwrap_or_default();
    json!({
        "content": [{
            "type": "text",
            "text": text
        }],
        "isError": true
    })
}

/// Extract an optional bool field from a JSON object.
fn optional_bool(params: &Map<String, Value>, field: &str) -> bool {
    params.get(field).and_then(Value::as_bool).unwrap_or(false)
}

/// Tri-state read: distinguishes "field absent" (None) from
/// "explicitly false" (Some(false)). Used for fields whose default
/// behavior depends on whether the caller supplied a value.
fn optional_bool_opt(params: &Map<String, Value>, field: &str) -> Option<bool> {
    params.get(field).and_then(Value::as_bool)
}

/// Return a copy of `params` with every path-like field expanded via
/// [`expand_path`]. Fields not present are skipped; fields present but
/// not strings are passed through unchanged (the downstream handler
/// will produce the usual "expected string" error).
fn normalize_path_fields(
    system: &dyn System,
    params: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let mut out = params.clone();
    for field in SCALAR_PATH_FIELDS {
        if let Some(Value::String(raw)) = out.get(*field).cloned() {
            let expanded = expand_path(system, &raw)
                .with_context(|| format!("expanding path field `{field}` ({raw:?})"))?;
            out.insert(
                (*field).to_owned(),
                Value::String(expanded.to_string_lossy().into_owned()),
            );
        }
    }
    for field in ARRAY_PATH_FIELDS {
        if let Some(Value::Array(arr)) = out.get(*field).cloned() {
            let mut expanded_arr = Vec::with_capacity(arr.len());
            for (idx, item) in arr.iter().enumerate() {
                if let Value::String(raw) = item {
                    let expanded = expand_path(system, raw).with_context(|| {
                        format!("expanding path field `{field}[{idx}]` ({raw:?})")
                    })?;
                    expanded_arr.push(Value::String(expanded.to_string_lossy().into_owned()));
                } else {
                    expanded_arr.push(item.clone());
                }
            }
            out.insert((*field).to_owned(), Value::Array(expanded_arr));
        }
    }
    Ok(out)
}

/// Extract an optional string field from a JSON object.
fn optional_str<'val>(params: &'val Map<String, Value>, field: &str) -> Option<&'val str> {
    params.get(field).and_then(Value::as_str)
}

/// Extract an optional integer field from a JSON object.
fn optional_usize(params: &Map<String, Value>, field: &str) -> Option<usize> {
    let val = params.get(field).and_then(Value::as_u64)?;
    usize::try_from(val).ok()
}

/// Extract a required string field from a JSON object.
fn required_str<'val>(params: &'val Map<String, Value>, field: &str) -> Result<&'val str> {
    params
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("missing required field: {field}"))
}

/// Extract a string array field from a JSON object (defaults to empty).
fn string_array(params: &Map<String, Value>, field: &str) -> Vec<String> {
    params
        .get(field)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Dispatch-time boundary check. Mirrors the historic `McpSandbox` for
/// unconstrained sessions (cwd fallback) and routes constrained / locked
/// sessions through `op_guard` so `trusted_roots` and `deny_ops` drive
/// the verdict. The per-tool handler still re-runs `op_guard` on the
/// canonical target — this dispatch hop catches paths handlers like
/// `lint` / `query` / `search` would not otherwise gate.
///
/// Per-tool special cases:
///
/// - `permissions_check`: the `path` field IS the target; we still
///   gate it so callers cannot probe arbitrary filesystem paths.
/// - `search`, `query`, `ls`, `sandbox_list`: the `path` field is a
///   base directory the op walks; the boundary check applies.
fn ensure_path_in_scope(
    system: &dyn System,
    base_dir: &Path,
    permissions: &ResolvedPermissions,
    tool_name: &str,
    params: &Map<String, Value>,
) -> Result<()> {
    if NO_PATH_TOOLS.contains(&tool_name) {
        return Ok(());
    }

    for field in SCALAR_PATH_FIELDS {
        // `config_path` and `key` are identity-declaration paths that
        // legitimately point outside the sandbox (e.g. `~/.ssh/id_ed25519`
        // or a `.remargin.yaml` in the user's home). They are not the
        // op's target, so the boundary check skips them. The identity
        // resolver's own validation handles their existence and
        // reachability.
        if matches!(*field, "config_path" | "key") {
            continue;
        }
        if let Some(raw) = params.get(*field).and_then(Value::as_str) {
            check_one_path(system, base_dir, permissions, tool_name, raw)?;
        }
    }
    for field in ARRAY_PATH_FIELDS {
        // `attachments` are write-side asset sources whose existence
        // is checked at the asset-copy step; the boundary check focuses
        // on the canonical target paths in `files`.
        if *field == "attachments" {
            continue;
        }
        if let Some(items) = params.get(*field).and_then(Value::as_array) {
            for item in items {
                if let Some(raw) = item.as_str() {
                    check_one_path(system, base_dir, permissions, tool_name, raw)?;
                }
            }
        }
    }

    // Folder-walk tools that omit `path` fall back to cwd at handler
    // time; mirror that fallback at the boundary so cwd cannot be read
    // when it is not in `trusted_roots`.
    let needs_cwd_fallback = PATH_DEFAULTS_TO_CWD_TOOLS.contains(&tool_name)
        || (tool_name == "ack" && !params.contains_key("file"));
    if needs_cwd_fallback && !params.contains_key("path") {
        check_one_path(system, base_dir, permissions, tool_name, ".")?;
    }

    Ok(())
}

/// Boundary check for a single path. UNCONSTRAINED sessions get the
/// cwd-fallback shape that `McpSandbox` used to enforce: the path must
/// canonicalise under `base_dir`. CONSTRAINED / LOCKED sessions route
/// through `op_guard` so violations carry the canonical
/// `trusted_roots` / `deny_ops` error wording.
fn check_one_path(
    system: &dyn System,
    base_dir: &Path,
    permissions: &ResolvedPermissions,
    tool_name: &str,
    raw: &str,
) -> Result<()> {
    let candidate = Path::new(raw);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    };
    let canonical = canonicalize_or_lexical(system, &absolute)?;

    if permissions.trusted_roots_unconstrained() {
        let canonical_base = system
            .canonicalize(base_dir)
            .unwrap_or_else(|_err| base_dir.to_path_buf());
        if canonical == canonical_base || canonical.starts_with(&canonical_base) {
            return Ok(());
        }
        anyhow::bail!("path escapes MCP sandbox: {raw}");
    }

    check_against_resolved_for_caller(
        system,
        tool_name,
        &canonical,
        permissions,
        &CallerInfo::default(),
    )
}

/// Canonicalise `target` if it exists; otherwise walk parents until one
/// canonicalises and append the missing tail. Lets the boundary admit
/// writes to not-yet-existing files inside a covered root while still
/// rejecting paths outside it.
fn canonicalize_or_lexical(system: &dyn System, target: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = system.canonicalize(target) {
        return Ok(canonical);
    }
    let mut suffix = PathBuf::new();
    let mut cursor = target.to_path_buf();
    while let Some(parent) = cursor.parent().map(Path::to_path_buf) {
        if parent.as_os_str().is_empty() {
            break;
        }
        let tail = cursor
            .file_name()
            .map(PathBuf::from)
            .with_context(|| format!("path missing file component: {}", target.display()))?;
        suffix = tail.join(&suffix);
        if let Ok(canonical_parent) = system.canonicalize(&parent) {
            return Ok(canonical_parent.join(suffix));
        }
        cursor = parent;
    }
    Ok(target.to_path_buf())
}

/// Resolve insertion position from tool parameters.
///
/// Shim around [`InsertPosition::from_hints`]: extracts the
/// three placement fields from the JSON params map and delegates the
/// actual precedence rule to core so CLI + MCP cannot disagree on where
/// a comment lands.
fn resolve_insert_position(params: &Map<String, Value>, reply_to: Option<&str>) -> InsertPosition {
    InsertPosition::from_hints(
        reply_to,
        optional_str(params, "after_comment"),
        optional_str(params, "after_heading"),
        optional_usize(params, "after_line"),
    )
}

/// Dispatch a tool call to the appropriate library function.
///
/// Returns the tool result as a JSON value (either success or error content).
fn dispatch_tool(
    system: &dyn System,
    base_dir: &Path,
    permissions: &ResolvedPermissions,
    config: &ResolvedConfig,
    session: &mut SessionState,
    tool_name: &str,
    params: &Map<String, Value>,
) -> Value {
    // Normalize path-like fields (`~`, `$VAR`, `${VAR}`) before dispatch
    // so every downstream handler sees already-expanded paths. Keeps CLI +
    // MCP in lockstep. A normalization failure is reported as a
    // tool-level error with the same surface as any other invalid param.
    let normalized = match normalize_path_fields(system, params) {
        Ok(map) => map,
        Err(err) => return tool_result_error(&format!("{err:#}")),
    };
    let p = &normalized;

    // Reject identity-declaration flags. The schema no longer
    // advertises them, but a schema-ignoring client can still send
    // them — this is the last defensible checkpoint.
    if let Some(rejection) = reject_identity_flags(tool_name, p) {
        return tool_result_error_json(&rejection.to_json());
    }

    // Dispatch-time boundary check. UNCONSTRAINED sessions get the
    // cwd-fallback behaviour the historic `McpSandbox` enforced;
    // CONSTRAINED / LOCKED sessions surface `op_guard` violations
    // with the canonical `trusted_roots` / `deny_ops` wording. The
    // per-tool handler still re-runs `op_guard` on the canonical
    // target — this hop catches handlers like `lint` / `query` /
    // `search` that would not otherwise gate.
    if let Err(err) = ensure_path_in_scope(system, base_dir, permissions, tool_name, p) {
        return tool_result_error(&format!("{err:#}"));
    }
    let result = match tool_name {
        "ack" => handle_ack(system, base_dir, config, p),
        "activity" => handle_activity(system, base_dir, config, p),
        "batch" => handle_batch(system, base_dir, config, p),
        "comment" => handle_comment(system, base_dir, config, p),
        "comments" => handle_comments(system, base_dir, p),
        "cp" => handle_cp(system, base_dir, config, p),
        "delete" => handle_delete(system, base_dir, config, p),
        "doctor" => handle_doctor(system, base_dir, p),
        "edit" => handle_edit(system, base_dir, config, p),
        "get" => handle_get(system, base_dir, config, p),
        "get_image" => handle_get_image(system, base_dir, config, p),
        "identity_create" => handle_identity_create(p),
        "lint" => handle_lint(system, base_dir, p),
        "ls" => handle_ls(system, base_dir, config, p),
        "metadata" => handle_metadata(system, base_dir, config, p),
        "mv" => handle_mv(system, base_dir, config, p),
        "permissions_check" => handle_permissions_check(system, base_dir, p),
        "permissions_show" => handle_permissions_show(system, base_dir),
        "plan" => handle_plan(system, base_dir, config, p),
        "prompt_delete" => handle_prompt_delete(system, base_dir, config, p),
        "prompt_list" => handle_prompt_list(system, base_dir, p),
        "prompt_resolve" => handle_prompt_resolve(system, base_dir, p),
        "prompt_set" => handle_prompt_set(system, base_dir, config, p),
        "purge" => handle_purge(system, base_dir, config, p),
        "query" => handle_query(system, base_dir, config, p),
        "react" => handle_react(system, base_dir, config, p),
        "replace" => handle_replace(system, base_dir, config, p),
        "reply" => handle_reply(system, base_dir, config, p),
        "report_spill" => Ok(handle_report_spill(session, p)),
        "claude_restrict" => {
            return tool_result_error(
                "tool 'claude_restrict' is not available via MCP - use the CLI: 'remargin claude \
                 restrict' or 'remargin plan claude restrict'",
            );
        }
        "claude_unrestrict" => {
            return tool_result_error(
                "tool 'claude_unrestrict' is not available via MCP - use the CLI: 'remargin claude \
                 unrestrict' or 'remargin plan claude unrestrict'",
            );
        }
        "rm" => handle_rm(system, base_dir, config, p),
        "sandbox_add" => handle_sandbox_add(system, base_dir, config, p),
        "sandbox_list" => handle_sandbox_list(system, base_dir, config, p),
        "sandbox_remove" => handle_sandbox_remove(system, base_dir, config, p),
        "search" => handle_search(system, base_dir, session.spill_cap, p),
        "sign" => handle_sign(system, base_dir, config, p),
        "verify" => handle_verify(system, base_dir, config, p),
        "whoami" => handle_whoami(system, base_dir),
        "write" => handle_write(system, base_dir, config, p),
        _ => return tool_result_error(&format!("unknown tool: {tool_name}")),
    };

    match result {
        Ok(value) => {
            // If the handler returned a pre-built MCP response (has "content"
            // array), pass it through unchanged. Otherwise wrap it.
            if value.get("content").is_some_and(Value::is_array) {
                value
            } else {
                tool_result_success(&value)
            }
        }
        Err(err) => err
            .downcast_ref::<operations::verify::SubsetGateFailure>()
            .map(operations::verify::SubsetGateFailure::to_json)
            .or_else(|| {
                err.downcast_ref::<operations::verify::VerifyFailure>()
                    .map(operations::verify::VerifyFailure::to_json)
            })
            .map_or_else(
                || tool_result_error(&format!("{err:#}")),
                |payload| tool_result_error_json(&payload),
            ),
    }
}

/// Handle the `activity` tool.
fn handle_activity(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let target = optional_str(params, "path").map_or_else(
        || base_dir.to_path_buf(),
        |raw| {
            let candidate = Path::new(raw);
            if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                base_dir.join(candidate)
            }
        },
    );
    let cutoff = match optional_str(params, "since") {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .with_context(|| format!("activity: invalid ISO 8601 since={raw:?}"))?,
        ),
        None => None,
    };
    let cfg = config;
    let caller = cfg
        .identity
        .as_deref()
        .context("activity: caller identity required (declare via identity / config_path)")?;

    let result = activity::gather_activity(system, &target, cutoff, caller)?;
    serde_json::to_value(&result).context("serializing activity result")
}

/// Handle the `ack` tool: acknowledge one or more comments.
fn handle_ack(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let ids = string_array(params, "ids");
    let remove = optional_bool(params, "remove");
    let cfg = config;

    if let Some(file) = optional_str(params, "file") {
        // Direct file path provided.
        let path = base_dir.join(file);
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        operations::ack_comments(system, &path, cfg, &id_refs, remove)?;
    } else {
        // Folder-wide ack: resolve each ID across the folder tree.
        let search_path = optional_str(params, "path").unwrap_or(".");
        let search_dir = base_dir.join(search_path);
        for comment_id in &ids {
            let matches = query::resolve_comment_id(system, &search_dir, comment_id)?;
            match matches.len() {
                0 => {
                    anyhow::bail!("comment {comment_id:?} not found");
                }
                1 => {
                    let id_refs: Vec<&str> = vec![comment_id.as_str()];
                    operations::ack_comments(system, &matches[0], cfg, &id_refs, remove)?;
                }
                n => {
                    let file_list: Vec<String> =
                        matches.iter().map(|p| p.display().to_string()).collect();
                    anyhow::bail!(
                        "ambiguous: comment {comment_id:?} found in {n} files: {}",
                        file_list.join(", ")
                    );
                }
            }
        }
    }

    Ok(responses::ack(&ids, remove))
}

/// Handle the `batch` tool: create multiple comments atomically.
/// MCP-only gate: a reply that explicitly opts out of acking *another author's*
/// comment (`auto_ack: false`) must justify it via `ack_skip_reason`. The smart
/// default and self-replies are exempt; the reason is validated, never stored.
fn check_ack_skip_reason(
    doc: &parser::ParsedDocument,
    identity: &str,
    parent_id: &str,
    auto_ack: Option<bool>,
    reason: Option<&str>,
) -> Result<()> {
    if auto_ack != Some(false) {
        return Ok(());
    }
    let Some(parent) = doc.find_comment(parent_id) else {
        return Ok(());
    };
    if parent.author == identity {
        return Ok(());
    }
    if reason.is_none_or(|r| r.trim().is_empty()) {
        bail!(
            "reply to comment {parent_id:?} sets auto_ack:false; provide ack_skip_reason explaining why you are not acknowledging it"
        );
    }
    Ok(())
}

/// Run [`check_ack_skip_reason`] over every reply op against a single file.
/// `ops` is `(reply_to, auto_ack, ack_skip_reason)` per op. The document is
/// parsed once, and only when at least one op opts out, so the common path
/// stays free. Parse/identity failures defer to the live op's canonical error.
fn enforce_ack_skip_reasons(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    file: &str,
    ops: &[(Option<&str>, Option<bool>, Option<&str>)],
) -> Result<()> {
    if !ops.iter().any(|(_, auto_ack, _)| *auto_ack == Some(false)) {
        return Ok(());
    }
    let Some(identity) = config.identity.as_deref() else {
        return Ok(());
    };
    let path = base_dir.join(file);
    let Ok(doc) = parser::parse_file(system, &path) else {
        return Ok(());
    };
    for (reply_to, auto_ack, reason) in ops {
        if let Some(parent_id) = reply_to {
            check_ack_skip_reason(&doc, identity, parent_id, *auto_ack, *reason)?;
        }
    }
    Ok(())
}

/// Single-op convenience over [`enforce_ack_skip_reasons`]: reads `auto_ack`
/// and `ack_skip_reason` from `params` for one reply.
fn gate_reply_ack(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    file: &str,
    reply_to: Option<&str>,
    params: &Map<String, Value>,
) -> Result<()> {
    enforce_ack_skip_reasons(
        system,
        base_dir,
        config,
        file,
        &[(
            reply_to,
            optional_bool_opt(params, "auto_ack"),
            optional_str(params, "ack_skip_reason"),
        )],
    )
}

/// Gate the `plan batch` projection exactly as the live `batch` tool, pulling
/// each sub-op's `ack_skip_reason` from the raw `ops` array.
fn enforce_plan_batch_ack_reasons(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    file: &str,
    ops: &[projections::ProjectBatchOp],
    params: &Map<String, Value>,
) -> Result<()> {
    let ack_gate: Vec<_> = ops
        .iter()
        .enumerate()
        .map(|(idx, op)| {
            let reason = params
                .get("ops")
                .and_then(Value::as_array)
                .and_then(|arr| arr.get(idx))
                .and_then(Value::as_object)
                .and_then(|obj| obj.get("ack_skip_reason"))
                .and_then(Value::as_str);
            (op.reply_to.as_deref(), op.auto_ack, reason)
        })
        .collect();
    enforce_ack_skip_reasons(system, base_dir, config, file, &ack_gate)
}

/// Parse the `plan batch` sub-ops and apply the MCP ack-reason gate before they
/// become a [`plan_ops::PlanRequest`].
fn gated_plan_batch_ops(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Vec<projections::ProjectBatchOp>> {
    let file = required_str(params, "file")?;
    let ops = parse_plan_batch_ops(params)?;
    enforce_plan_batch_ack_reasons(system, base_dir, config, file, &ops, params)?;
    Ok(ops)
}

/// Build the `plan comment` / `plan reply` request, applying the MCP ack-reason
/// gate. Owns nothing — the borrowed staging values live in the caller.
fn build_plan_comment_request<'plan>(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    op: &str,
    params: &'plan Map<String, Value>,
    staging: &'plan mut PlanCommentStaging,
    attach_refs: &'plan mut Vec<&'plan str>,
) -> Result<plan_ops::PlanRequest<'plan>> {
    let file = required_str(params, "file")?;
    let content = required_str(params, "content")?;
    staging.to_owned = string_array(params, "to");
    let parent = optional_str(params, "parent_id").or_else(|| optional_str(params, "reply_to"));
    if op == "reply" && parent.is_none() {
        bail!("plan reply: `parent_id` is required");
    }
    staging.reply_to_owned = parent.map(String::from);
    let reply_to = staging.reply_to_owned.as_deref();
    gate_reply_ack(system, base_dir, config, file, reply_to, params)?;
    staging.attach_names = string_array(params, "attach_names");
    staging.position = resolve_insert_position(params, staging.reply_to_owned.as_deref());
    *attach_refs = staging.attach_names.iter().map(String::as_str).collect();
    let project_params = projections::ProjectCommentParams::new(content, &staging.position)
        .with_attachment_filenames(attach_refs)
        .with_auto_ack(optional_bool_opt(params, "auto_ack"))
        .with_reply_to(staging.reply_to_owned.as_deref())
        .with_sandbox(optional_bool(params, "sandbox"))
        .with_to(&staging.to_owned);
    Ok(plan_ops::PlanRequest::Comment {
        path: base_dir.join(file),
        params: project_params,
    })
}

fn handle_batch(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let cfg = config;
    let ops_value = params
        .get("operations")
        .and_then(Value::as_array)
        .context("missing required field: operations")?;

    let mut batch_ops = Vec::with_capacity(ops_value.len());
    for (idx, op_value) in ops_value.iter().enumerate() {
        let op_obj = op_value
            .as_object()
            .with_context(|| format!("batch op[{idx}]: expected object"))?;
        batch_ops.push(BatchCommentOp::from_json_object(op_obj, idx)?);
    }

    let ack_gate: Vec<_> = batch_ops
        .iter()
        .enumerate()
        .map(|(idx, op)| {
            let reason = ops_value
                .get(idx)
                .and_then(Value::as_object)
                .and_then(|obj| obj.get("ack_skip_reason"))
                .and_then(Value::as_str);
            (op.reply_to.as_deref(), op.auto_ack, reason)
        })
        .collect();
    enforce_ack_skip_reasons(system, base_dir, cfg, file, &ack_gate)?;

    let path = base_dir.join(file);
    let ids = operations::batch::batch_comment(system, &path, cfg, &batch_ops)?;
    Ok(responses::batch(&ids))
}

/// Handle the `comment` tool: create a single comment.
fn handle_comment(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let content = required_str(params, "content")?;
    let to = string_array(params, "to");
    let reply_to = optional_str(params, "reply_to").map(String::from);
    let attachments: Vec<PathBuf> = string_array(params, "attachments")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    // MCP parity with the `--kind` CLI flag. Accepts either
    // `remargin_kind: ["question", "todo"]` or the more natural
    // `kind: [...]` alias; validation happens inside `create_comment`.
    let remargin_kind_raw = string_array(params, "remargin_kind");
    let remargin_kind = if remargin_kind_raw.is_empty() {
        string_array(params, "kind")
    } else {
        remargin_kind_raw
    };
    let cfg = config;

    let position = resolve_insert_position(params, reply_to.as_deref());

    let auto_ack = optional_bool_opt(params, "auto_ack");
    gate_reply_ack(system, base_dir, cfg, file, reply_to.as_deref(), params)?;

    let sandbox = optional_bool(params, "sandbox");
    let create_params = operations::CreateCommentParams {
        attachments: &attachments,
        auto_ack,
        content,
        position: &position,
        remargin_kind: &remargin_kind,
        reply_to: reply_to.as_deref(),
        sandbox,
        to: &to,
    };

    let path = base_dir.join(file);
    let new_id = operations::create_comment(system, &path, cfg, &create_params)?;
    Ok(responses::comment_created(&new_id))
}

/// Handle the `comments` tool: list all comments in a document.
fn handle_comments(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    // shared kind filter with the CLI path. Accepts either
    // `remargin_kind` or `kind` as the MCP key.
    let kind_filter = {
        let raw = string_array(params, "remargin_kind");
        if raw.is_empty() {
            string_array(params, "kind")
        } else {
            raw
        }
    };

    let path = base_dir.join(file);
    let doc = parser::parse_file(system, &path)?;
    let comments: Vec<&parser::Comment> = doc
        .comments()
        .into_iter()
        .filter(|cm| matches_kind_filter(cm.kinds(), &kind_filter))
        .collect();

    let items = comments
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<Value>, _>>()?;
    Ok(json!({ "comments": items }))
}

/// Handle the `delete` tool: delete one or more comments.
fn handle_delete(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let ids = string_array(params, "ids");
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let cfg = config;

    let path = base_dir.join(file);
    operations::delete_comments(system, &path, cfg, &id_refs)?;
    Ok(responses::comments_deleted(&ids))
}

/// Handle the `edit` tool: edit a comment's content.
fn handle_edit(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let comment_id = required_str(params, "id")?;
    let new_content = required_str(params, "content")?;
    // optional replacement kind list. When the key is absent
    // we pass `None` so the stored list is preserved; an empty array
    // explicitly clears (validate_kinds accepts `[]`).
    let remargin_kind_value = params.get("remargin_kind").or_else(|| params.get("kind"));
    let new_kinds: Option<Vec<String>> = match remargin_kind_value {
        Some(Value::Array(arr)) => Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
        ),
        Some(Value::Null) | None => None,
        _ => anyhow::bail!("`remargin_kind`/`kind` must be an array of strings"),
    };
    let cfg = config;

    let path = base_dir.join(file);
    operations::edit_comment(
        system,
        &path,
        cfg,
        comment_id,
        new_content,
        new_kinds.as_deref(),
    )?;
    Ok(responses::comment_edited(comment_id))
}

/// Handle the `get` tool: read a file's contents.
///
/// When `binary: true`, bytes are read through the shared `read_binary`
/// core helper (symmetric with CLI `get --binary`) and returned base64-
/// encoded alongside size + mime. Markdown files are rejected in this mode
/// so comment-preservation is never bypassed.
fn handle_get(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let binary = optional_bool(params, "binary");
    let end_line = optional_usize(params, "end_line");
    let line_numbers = optional_bool(params, "line_numbers");
    let start_line = optional_usize(params, "start_line");

    let target = Path::new(path_str);

    if binary {
        if start_line.is_some() || end_line.is_some() {
            bail!("start_line / end_line are not supported with binary: true");
        }
        if line_numbers {
            bail!("line_numbers is not supported with binary: true");
        }
        let payload =
            document::read_binary(system, base_dir, target, false, &config.trusted_roots)?;
        let encoded = BASE64_STANDARD.encode(&payload.bytes);
        return Ok(json!({
            "binary": true,
            "content": encoded,
            "mime": payload.mime,
            "path": payload.path,
            "size_bytes": payload.size_bytes,
        }));
    }

    let lines = match (start_line, end_line) {
        (Some(start), Some(end)) => Some((start, end)),
        _ => None,
    };

    let result = document::get_with_links(
        system,
        base_dir,
        target,
        lines,
        false,
        false,
        &config.trusted_roots,
    )?;

    if line_numbers {
        let start_num = lines.map_or(1, |(s, _)| s);
        let json_lines: Vec<Value> = result
            .content
            .split('\n')
            .enumerate()
            .map(|(i, text)| json!({ "line": start_num + i, "text": text }))
            .collect();
        Ok(json!({ "lines": json_lines, "links": result.links }))
    } else {
        Ok(json!({ "content": result.content, "links": result.links }))
    }
}

/// Handle the `identity_create` tool.
///
/// Mirrors the CLI `remargin identity create` surface: validates the
/// author type and returns both the rendered YAML text and the
/// structured fields so the caller can either paste the text verbatim
/// or consume the fields directly.
fn handle_identity_create(params: &Map<String, Value>) -> Result<Value> {
    let identity = required_str(params, "identity")?;
    let author_type = required_str(params, "type")?;
    parse_author_type(author_type)
        .with_context(|| format!("invalid `type` value: {author_type}"))?;
    let key = optional_str(params, "key");

    let mut yaml = format!("identity: {identity}\ntype: {author_type}\n");
    if let Some(k) = key {
        use core::fmt::Write as _;
        let _ = writeln!(yaml, "key: {k}");
    }
    Ok(json!({
        "identity": identity,
        "type": author_type,
        "key": key,
        "yaml": yaml,
    }))
}

/// Handle the `lint` tool: run structural lint checks.
fn handle_lint(system: &dyn System, base_dir: &Path, params: &Map<String, Value>) -> Result<Value> {
    let file = required_str(params, "file")?;
    let path = base_dir.join(file);
    Ok(linter::lint_doc(system, &path)?.to_json())
}

/// Handle the `ls` tool: list files and directories.
fn handle_ls(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = optional_str(params, "path").unwrap_or(".");
    let target = Path::new(path_str);

    let entries = document::ls(system, base_dir, target, config)?;

    Ok(json!({ "entries": entries }))
}

/// Handle the `metadata` tool: get document metadata.
fn handle_metadata(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let target = Path::new(path_str);

    let meta = document::metadata(system, base_dir, target, false, &config.trusted_roots)?;

    Ok(meta.to_json(true))
}

/// Build the canonical "this plan op is CLI-only" error returned when
/// `mcp__remargin__plan` is called with `op="claude_restrict"` or
/// `op="claude_unrestrict"`. Pulled out so [`handle_plan`] stays
/// under the adapter LOC cap.
fn plan_op_cli_only_error(op: &str) -> anyhow::Error {
    let cli = match op {
        "claude_restrict" => "remargin plan claude restrict",
        "claude_unrestrict" => "remargin plan claude unrestrict",
        _ => "remargin plan",
    };
    anyhow::anyhow!("plan op '{op}' is not available via MCP - use the CLI: '{cli}'")
}

/// Handle the `plan` tool: parse the request shape, build a
/// [`plan_ops::PlanRequest`], and delegate to the canonical
/// [`plan_ops::dispatch`]. The adapter-layer work is
/// limited to JSON field extraction and the final `serde_json::to_value`.
fn handle_plan(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let op = required_str(params, "op")?;
    let cfg = config;

    // `comment` stages owned values (position, attach refs, …) that the
    // projected `ProjectCommentParams` borrows — they must outlive dispatch.
    let mut staging = PlanCommentStaging::default();
    let mut attach_refs: Vec<&str> = Vec::new();

    let request = match op {
        "ack" => plan_ops::PlanRequest::Ack {
            path: base_dir.join(required_str(params, "file")?),
            ids: string_array(params, "ids"),
            remove: optional_bool(params, "remove"),
        },
        "comment" | "reply" => build_plan_comment_request(
            system,
            base_dir,
            cfg,
            op,
            params,
            &mut staging,
            &mut attach_refs,
        )?,
        "delete" => plan_ops::PlanRequest::Delete {
            path: base_dir.join(required_str(params, "file")?),
            ids: string_array(params, "ids"),
        },
        "edit" => plan_ops::PlanRequest::Edit {
            path: base_dir.join(required_str(params, "file")?),
            id: required_str(params, "id")?,
            content: required_str(params, "content")?,
        },
        "react" => plan_ops::PlanRequest::React {
            path: base_dir.join(required_str(params, "file")?),
            id: required_str(params, "id")?,
            emoji: required_str(params, "emoji")?,
            remove: optional_bool(params, "remove"),
        },
        "batch" => plan_ops::PlanRequest::Batch {
            path: base_dir.join(required_str(params, "file")?),
            ops: gated_plan_batch_ops(system, base_dir, cfg, params)?,
        },
        "cp" => {
            let (src, dst, force) = parse_plan_src_dst(params)?;
            plan_ops::PlanRequest::Cp { src, dst, force }
        }
        "mv" => {
            let (src, dst, force) = parse_plan_src_dst(params)?;
            plan_ops::PlanRequest::Mv { src, dst, force }
        }
        "purge" => plan_ops::PlanRequest::Purge {
            path: base_dir.join(required_str(params, "file")?),
            recursive: optional_bool(params, "recursive"),
        },
        "claude_restrict" | "claude_unrestrict" => return Err(plan_op_cli_only_error(op)),
        "sandbox-add" => plan_ops::PlanRequest::SandboxAdd {
            path: base_dir.join(required_str(params, "file")?),
        },
        "sandbox-remove" => plan_ops::PlanRequest::SandboxRemove {
            path: base_dir.join(required_str(params, "file")?),
        },
        "sign" => plan_ops::PlanRequest::Sign {
            path: base_dir.join(required_str(params, "file")?),
            selection: build_sign_selection(params, "plan sign")?,
        },
        "write" => {
            let file = required_str(params, "file")?;
            let content = required_str(params, "content")?;
            let lines = optional_str(params, "lines");
            let line_range = lines.map(parse_plan_line_range).transpose()?;
            let opts = document::WriteOptions::new()
                .binary(optional_bool(params, "binary"))
                .create(optional_bool(params, "create"))
                .lines(line_range)
                .raw(optional_bool(params, "raw"));
            plan_ops::PlanRequest::Write {
                path: PathBuf::from(file),
                content,
                opts,
            }
        }
        other => bail!("plan: unknown op {other:?}"),
    };

    let report = plan_ops::dispatch(system, base_dir, cfg, &request)?;
    serde_json::to_value(&report).context("serializing plan report")
}

/// Parse `src`, `dst`, and `force` from a plan-tool param map.
/// Used by the `cp` and `mv` plan arms.
fn parse_plan_src_dst(params: &Map<String, Value>) -> Result<(PathBuf, PathBuf, bool)> {
    Ok((
        PathBuf::from(required_str(params, "src")?),
        PathBuf::from(required_str(params, "dst")?),
        optional_bool(params, "force"),
    ))
}

/// Parse the `ops` array from a `plan.batch` MCP request into
/// [`projections::ProjectBatchOp`] values.
///
/// Each entry is an object with the same shape as the `batch` tool's sub-op
/// (`content`, `reply_to`, `after_comment`, `after_line`, `attach_names`,
/// `auto_ack`, `to`). Unknown fields are ignored; missing `content`
/// rejects the whole batch.
fn parse_plan_batch_ops(params: &Map<String, Value>) -> Result<Vec<projections::ProjectBatchOp>> {
    let ops_val = params
        .get("ops")
        .context("plan batch: `ops` array is required")?;
    let ops_arr = ops_val
        .as_array()
        .context("plan batch: `ops` must be an array")?;

    let mut ops: Vec<projections::ProjectBatchOp> = Vec::with_capacity(ops_arr.len());
    for (idx, entry) in ops_arr.iter().enumerate() {
        let obj = entry
            .as_object()
            .with_context(|| format!("plan batch op[{idx}]: expected object"))?;
        ops.push(projections::ProjectBatchOp::from_json_object(obj, idx)?);
    }
    Ok(ops)
}

/// Parse the `lines` field of a `plan.write` request into a
/// `(start, end)` 1-indexed tuple. Mirrors the CLI's `parse_line_range`.
fn parse_plan_line_range(raw: &str) -> Result<(usize, usize)> {
    let (start_str, end_str) = raw
        .split_once('-')
        .with_context(|| format!("lines expects START-END, got {raw:?}"))?;
    let start: usize = start_str
        .parse()
        .with_context(|| format!("lines: invalid start value {start_str:?}"))?;
    let end: usize = end_str
        .parse()
        .with_context(|| format!("lines: invalid end value {end_str:?}"))?;
    Ok((start, end))
}

/// Handle the `prompt_resolve` tool: walk-up the `.remargin.yaml`
/// chain looking for a `system_prompt:` block. Read-only.
fn handle_prompt_resolve(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let candidate = Path::new(file);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    };
    let resolved = resolve_system_prompt(system, &absolute)?;
    serde_json::to_value(&resolved).context("serializing prompt_resolve output")
}

fn resolve_folder_param(base_dir: &Path, params: &Map<String, Value>) -> PathBuf {
    let raw = optional_str(params, "folder").unwrap_or(".");
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    }
}

fn handle_prompt_set(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let folder = resolve_folder_param(base_dir, params);
    let name = required_str(params, "name")?;
    let prompt = required_str(params, "prompt")?;
    let cfg = config;
    let outcome = prompt_ops::set(system, &folder, Some(name), prompt, cfg)?;
    serde_json::to_value(&outcome).context("serializing prompt_set output")
}

fn handle_prompt_delete(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let folder = resolve_folder_param(base_dir, params);
    let cfg = config;
    let outcome = prompt_ops::delete(system, &folder, cfg)?;
    serde_json::to_value(&outcome).context("serializing prompt_delete output")
}

fn handle_prompt_list(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let folder = resolve_folder_param(base_dir, params);
    let entries = prompt_ops::list(system, &folder)?;
    let entries_value =
        serde_json::to_value(&entries).context("serializing prompt_list entries")?;
    Ok(json!({ "entries": entries_value }))
}

fn handle_purge(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let recursive = optional_bool(params, "recursive");
    let cfg = config;

    let path = base_dir.join(file);

    if recursive {
        let result = purge::purge_dir(system, &path, cfg)?;
        return Ok(result.to_json(base_dir));
    }

    if system.is_dir(&path).unwrap_or(false) {
        bail!(
            "target is a directory: {file} (pass `recursive: true` to purge every .md file under it)"
        );
    }

    let result = purge::purge(system, &path, cfg)?;
    Ok(result.to_json())
}

/// Handle the `query` tool: search across documents.
fn handle_query(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = optional_str(params, "path").unwrap_or(".");
    let filter = build_query_filter_from_params(params, config.identity.clone())?;
    let results = query::query(system, &base_dir.join(path_str), &filter)?;

    Ok(json!({
        "base_path": format!("{}/", path_str.trim_end_matches('/')),
        "results": results,
    }))
}

/// Translate `query` tool params into a [`QueryFilter`]. Pulled out so
/// `handle_query` stays under the adapter LOC cap.
fn build_query_filter_from_params(
    params: &Map<String, Value>,
    caller_identity: Option<String>,
) -> Result<QueryFilter> {
    let since = optional_str(params, "since")
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .with_context(|| format!("invalid timestamp: {s}"))
        })
        .transpose()?;
    let kind_filter = {
        let raw = string_array(params, "remargin_kind");
        if raw.is_empty() {
            string_array(params, "kind")
        } else {
            raw
        }
    };
    let mut filter = QueryFilter {
        author: optional_str(params, "author").map(String::from),
        comment_id: optional_str(params, "comment_id").map(String::from),
        expanded: optional_bool(params, "expanded"),
        pending: optional_bool(params, "pending"),
        pending_for: optional_str(params, "pending_for").map(String::from),
        remargin_kind: kind_filter,
        since,
        summary: optional_bool(params, "summary"),
        ..QueryFilter::default()
    };
    filter = filter.with_caller_identity(
        optional_bool(params, "pending_for_me"),
        optional_bool(params, "pending_broadcast"),
        caller_identity,
    )?;
    if let Some(pattern) = optional_str(params, "content_regex") {
        filter = filter.with_content_regex(pattern, optional_bool(params, "ignore_case"))?;
    }
    Ok(filter)
}

/// Handle the `permissions_show` tool.
///
/// Pure read-only inspection — no identity resolution, no config
/// load. Returns the parent-walked `.remargin.yaml` permissions tree
/// rooted at `base_dir` (the MCP server's working directory).
/// Handle the `doctor` tool.
fn handle_doctor(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let user_settings_file = params
        .get("user_settings_file")
        .and_then(Value::as_str)
        .map_or_else(
            || {
                use crate::path::expand_path;
                expand_path(system, "~/.claude/settings.json")
                    .unwrap_or_else(|_| PathBuf::from("~/.claude/settings.json"))
            },
            PathBuf::from,
        );
    let report = permissions_doctor::run_doctor(system, base_dir, &user_settings_file)?;
    serde_json::to_value(&report).context("serializing doctor report")
}

fn handle_permissions_show(system: &dyn System, base_dir: &Path) -> Result<Value> {
    let report = permissions_inspect::show(system, base_dir)?;
    serde_json::to_value(&report).context("serializing permissions show output")
}

/// Handle the `permissions_check` tool.
///
/// Returns `restricted=true` when the path is outside the
/// `trusted_roots` allow-list or covered by a `deny_ops` rule. With
/// `why=true`, the closest matching rule is included.
fn handle_permissions_check(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let why = optional_bool(params, "why");
    let candidate = Path::new(path_str);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    };
    let report = permissions_inspect::check(system, base_dir, &absolute, why)?;
    serde_json::to_value(&report).context("serializing permissions check output")
}

/// Handle the `react` tool: add or remove an emoji reaction.
fn handle_react(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let comment_id = required_str(params, "id")?;
    let emoji = required_str(params, "emoji")?;
    let remove = optional_bool(params, "remove");
    let cfg = config;

    let path = base_dir.join(file);
    operations::react(system, &path, cfg, comment_id, emoji, remove)?;
    Ok(responses::react(emoji, comment_id, remove))
}

/// Handle the `reply` tool: translate `parent_id` into `reply_to` and
/// delegate to [`handle_comment`].
fn handle_reply(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let parent_id = required_str(params, "parent_id")?.to_owned();
    let mut translated = params.clone();
    translated.remove("parent_id");
    translated.insert("reply_to".into(), Value::String(parent_id));
    handle_comment(system, base_dir, config, &translated)
}

/// Handle the `rm` tool: remove a file from the managed document tree.
fn handle_rm(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let target = Path::new(path_str);
    let result = document::rm(system, base_dir, target, config)?;
    Ok(result.to_json(path_str))
}

/// Handle the `mv` tool: move or rename a tracked file.
fn handle_mv(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let src = required_str(params, "src")?;
    let dst = required_str(params, "dst")?;
    let force = optional_bool(params, "force");

    let args = mv_op::MvArgs::new(PathBuf::from(src), PathBuf::from(dst)).with_force(force);
    let outcome = mv_op::mv(system, base_dir, config, &args)?;
    Ok(outcome.to_json())
}

/// Handle the `cp` tool: copy a tracked file.
fn handle_cp(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let src = required_str(params, "src")?;
    let dst = required_str(params, "dst")?;
    let force = optional_bool(params, "force");

    let args = cp_op::CpArgs::new(PathBuf::from(src), PathBuf::from(dst)).with_force(force);
    let outcome = cp_op::cp(system, base_dir, config, &args)?;
    Ok(serde_json::to_value(&outcome)?)
}

fn handle_get_image(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let target = Path::new(path_str);

    let crop = optional_str(params, "crop");
    let format = optional_str(params, "format");
    let max_bytes = params.get("max_bytes").and_then(Value::as_u64);
    let max_dimension = params
        .get("max_dimension")
        .and_then(Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let options =
        image_ops::GetImageOptions::from_optionals(crop, format, max_bytes, max_dimension)?;

    let result = image_ops::get_image(
        system,
        base_dir,
        target,
        config.unrestricted,
        &config.trusted_roots,
        &options,
    )?;

    let mut envelope = result.to_json_without_content();
    envelope["binary"] = Value::Bool(true);
    envelope["content"] = Value::String(BASE64_STANDARD.encode(&result.bytes));
    envelope["path"] = json!(result.source_path);
    Ok(envelope)
}

/// Handle the `sandbox_add` tool: stage files in the caller's sandbox.
fn handle_sandbox_add(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file_strs = string_array(params, "files");
    let files: Vec<PathBuf> = file_strs.iter().map(|f| base_dir.join(f)).collect();

    let cfg = config;
    let identity = cfg
        .identity
        .as_deref()
        .context("identity is required for sandbox_add")?;

    let result = sandbox_ops::add_to_files(system, &files, identity, cfg)?;
    Ok(result.to_json(base_dir, "added"))
}

/// Handle the `sandbox_remove` tool: unstage files from the caller's sandbox.
fn handle_sandbox_remove(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file_strs = string_array(params, "files");
    let files: Vec<PathBuf> = file_strs.iter().map(|f| base_dir.join(f)).collect();

    let cfg = config;
    let identity = cfg
        .identity
        .as_deref()
        .context("identity is required for sandbox_remove")?;

    let result = sandbox_ops::remove_from_files(system, &files, identity, cfg)?;
    Ok(result.to_json(base_dir, "removed"))
}

/// Handle the `sandbox_list` tool: list files staged for the caller.
fn handle_sandbox_list(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let root = base_dir.join(optional_str(params, "path").unwrap_or("."));

    let cfg = config;
    let identity = cfg
        .identity
        .as_deref()
        .context("identity is required for sandbox_list")?;

    let listings = sandbox_ops::list_for_identity(system, &root, identity)?;
    let files: Vec<sandbox_ops::SandboxListEntry> = listings
        .iter()
        .map(|l| sandbox_ops::SandboxListEntry::from_listing(l, &root, false))
        .collect();
    Ok(json!({ "files": files }))
}

/// Handle the `search` tool: search across documents for text matches.
fn handle_search(
    system: &dyn System,
    base_dir: &Path,
    spill_cap: usize,
    params: &Map<String, Value>,
) -> Result<Value> {
    let pattern = required_str(params, "pattern")?;
    let path_str = optional_str(params, "path").unwrap_or(".");
    let target = base_dir.join(path_str);
    let regex = optional_bool(params, "regex");
    let ignore_case = optional_bool(params, "ignore_case");
    let context = optional_usize(params, "context").unwrap_or(0);
    let limit = optional_usize(params, "limit");
    let offset = optional_usize(params, "offset").unwrap_or(0);

    let scope = match optional_str(params, "scope").unwrap_or("all") {
        "body" => search::SearchScope::Body,
        "comments" => search::SearchScope::Comments,
        _ => search::SearchScope::All,
    };

    let options = search::SearchOptions::new(String::from(pattern))
        .context_lines(context)
        .ignore_case(ignore_case)
        .limit(limit)
        .offset(offset)
        .regex(regex)
        .scope(scope);

    let results = search::search(system, base_dir, &target, &options)?;

    let window = results
        .matches
        .iter()
        .map(|m| serde_json::to_value(search::SearchHit::from_match(m)))
        .collect::<Result<Vec<Value>, _>>()?;

    // The caller's own `limit` already bounded the window; the cap only ever
    // narrows it further, never widens past what the caller asked for. When
    // the cap clamps below the window, `effective_limit` is surfaced so the
    // agent knows this is a bounded page and can offset for the rest — `total`
    // already tells it how many remain.
    let (matches, effective_limit) = size_search_page(window, spill_cap);

    let mut envelope = json!({ "matches": matches, "total": results.total });
    if let Some(effective) = effective_limit
        && let Some(obj) = envelope.as_object_mut()
    {
        obj.insert(String::from("effective_limit"), Value::from(effective));
    }
    Ok(envelope)
}

/// Trim a search page so its emitted size stays under `spill_cap`.
///
/// Greedily keeps hits while the running serialized size stays within the cap
/// (minus a margin for the envelope and pretty-print indentation). Always
/// keeps at least one hit when the window is non-empty, so a single oversized
/// match can never livelock paging. Returns `Some(effective_limit)` only when
/// the cap forced fewer rows than the window carried — that clamp is the
/// signal the agent should page for the rest. `spill_cap` is a self-consistent
/// byte proxy for the client's token limit, not an equivalent.
fn size_search_page(window: Vec<Value>, spill_cap: usize) -> (Vec<Value>, Option<usize>) {
    let window_len = window.len();
    let budget = spill_cap.saturating_sub(spill_cap / SPILL_MARGIN_DIVISOR);
    let mut running = 0_usize;
    let mut kept = 0_usize;
    for hit in &window {
        let size = serde_json::to_string_pretty(hit).map_or(usize::MAX, |s| s.len());
        // Admit the first hit unconditionally; a lone oversized match must
        // still ship or the caller can never page past it.
        if kept > 0 && running.saturating_add(size) > budget {
            break;
        }
        running = running.saturating_add(size);
        kept = kept.saturating_add(1);
    }
    if kept >= window_len {
        (window, None)
    } else {
        let mut page = window;
        page.truncate(kept);
        (page, Some(kept))
    }
}

/// Handle the `report_spill` tool: ratchet the session's `spill_cap` DOWN
/// after the client spilled a remargin result to a file.
///
/// remargin infers the offending size from `last_response_size` (the last
/// result it emitted); an explicit `size` supersedes that inference. The cap
/// only ever falls — a report can never raise it — and a reported size of 0
/// is a no-op (nothing meaningful was measured).
fn handle_report_spill(session: &mut SessionState, params: &Map<String, Value>) -> Value {
    let explicit = optional_usize(params, "size");
    let reported = explicit.unwrap_or(session.last_response_size);
    let previous_cap = session.spill_cap;
    if reported > 0 {
        session.spill_cap = session.spill_cap.min(reported);
    }
    json!({
        "inferred": explicit.is_none(),
        "previous_cap": previous_cap,
        "reported_size": reported,
        "spill_cap": session.spill_cap,
    })
}

/// Handle the `replace` tool: body-only find/replace over a file or
/// folder. `path` is required — there is no silent cwd fan-out for a
/// mutating folder op.
fn handle_replace(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let pattern = required_str(params, "pattern")?;
    let replacement = required_str(params, "replacement")?;
    let path_str = required_str(params, "path")?;
    let target = base_dir.join(path_str);

    // `dry_run` is deliberately not exposed on the MCP surface — preview
    // migrated to `plan` for every tool (see
    // `no_mode_or_dry_run_in_any_schema`); the CLI keeps `--dry-run`.
    let options = replace::ReplaceOptions::new(String::from(pattern), String::from(replacement))
        .regex(optional_bool(params, "regex"))
        .ignore_case(optional_bool(params, "ignore_case"));

    let report = replace::replace(system, base_dir, &target, &options, config)?;
    serde_json::to_value(&report).context("serializing replace report")
}

/// Resolve the `ids` / `all_mine` selection shared by `sign` and
/// `plan sign`. `op` labels the error messages so callers can
/// tell the two tools apart.
fn build_sign_selection(
    params: &Map<String, Value>,
    op: &str,
) -> Result<operations::sign::SignSelection> {
    let all_mine = optional_bool(params, "all_mine");
    let ids = string_array(params, "ids");
    if all_mine && !ids.is_empty() {
        bail!("{op}: `ids` and `all_mine` are mutually exclusive");
    }
    if !all_mine && ids.is_empty() {
        bail!("{op}: pass `ids` (array of comment ids) or `all_mine: true`");
    }
    Ok(if all_mine {
        operations::sign::SignSelection::AllMine
    } else {
        operations::sign::SignSelection::Ids(ids)
    })
}

/// Handle the `sign` tool: back-sign missing-signature comments authored
/// by the resolved identity.
fn handle_sign(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let selection = build_sign_selection(params, "sign")?;
    let repair_checksum = optional_bool(params, "repair_checksum");

    let cfg = config;

    let path = base_dir.join(file);
    let options = operations::sign::SignOptions { repair_checksum };
    let result = operations::sign::sign_comments(system, &path, cfg, &selection, options)?;
    Ok(result.to_json())
}

/// Handle the `verify` tool: verify comment integrity.
fn handle_verify(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    // `path` is canonical (matches `search`/`replace`); `file` is a
    // backward-compatible alias. Prefer `path`, fall back to `file`.
    let path_str = optional_str(params, "path")
        .or_else(|| optional_str(params, "file"))
        .with_context(|| String::from("missing required field: path"))?;
    let target = base_dir.join(path_str);

    // A directory target sweeps the tree and returns the folder report;
    // a single file keeps today's `VerifyReport::to_json` shape so
    // existing callers are unaffected.
    if system.is_dir(&target).unwrap_or(false) {
        let report = operations::verify::verify_path(system, base_dir, &target, config)?;
        return Ok(report.to_json());
    }
    let report = operations::verify::verify_and_refresh(system, &target, config)?;
    Ok(report.to_json())
}

/// Handle the `whoami` tool. Returns the MCP server's startup
/// identity. To project a different identity, use the CLI
/// (`remargin identity show --identity X --type Y`). Soft-misses
/// missing-config so polling clients don't see a hard error.
fn handle_whoami(system: &dyn System, base_dir: &Path) -> Result<Value> {
    let report = resolve_identity_report(system, base_dir, &IdentityFlags::default())?;
    serde_json::to_value(&report).context("serializing whoami report")
}

/// Handle the `write` tool: write document contents.
fn handle_write(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let content = required_str(params, "content")?;
    let binary = params
        .get("binary")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let create = params
        .get("create")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let raw = params.get("raw").and_then(Value::as_bool).unwrap_or(false);
    let start_line = optional_usize(params, "start_line");
    let end_line = optional_usize(params, "end_line");
    let lines = match (start_line, end_line) {
        (Some(s), Some(e)) => Some((s, e)),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("partial write requires both start_line and end_line")
        }
    };

    let cfg = config;

    let opts = document::WriteOptions::new()
        .binary(binary)
        .create(create)
        .lines(lines)
        .raw(raw);
    let target = Path::new(path_str);
    let outcome = document::write(system, base_dir, target, content, cfg, opts)?;

    Ok(outcome.to_json(path_str, binary, raw))
}

/// Process a single JSON-RPC request and return a response (or `None` for notifications).
fn process_message(
    system: &dyn System,
    base_dir: &Path,
    permissions: &ResolvedPermissions,
    config: &ResolvedConfig,
    session: &mut SessionState,
    message: &Value,
) -> Option<Value> {
    let id = message.get("id");
    let method = message.get("method").and_then(Value::as_str);

    // Notifications (no id) don't get a response.
    let request_id = id?;

    let Some(method_name) = method else {
        return Some(error_response(
            request_id,
            METHOD_NOT_FOUND,
            "missing method",
        ));
    };

    match method_name {
        "initialize" => {
            let result = json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": env!("CARGO_PKG_VERSION")
                }
            });
            Some(success_response(request_id, &result))
        }
        "tools/list" => {
            let tools: Vec<Value> = tool_descriptors()
                .iter()
                .map(|td| {
                    json!({
                        "name": td.name,
                        "description": td.description,
                        "inputSchema": td.schema
                    })
                })
                .collect();
            Some(success_response(request_id, &json!({ "tools": tools })))
        }
        "tools/call" => {
            let tool_params = message.get("params").and_then(Value::as_object);
            let Some(call_params) = tool_params else {
                return Some(error_response(request_id, INVALID_PARAMS, "missing params"));
            };

            let tool_name = call_params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");

            let arguments = call_params
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();

            let start = Instant::now();
            let mut result = dispatch_tool(
                system,
                base_dir,
                permissions,
                config,
                session,
                tool_name,
                &arguments,
            );
            let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

            // Inject elapsed_ms into the tool result's text JSON.
            if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut)
                && let Some(text_val) = content.first_mut().and_then(|c| c.get_mut("text"))
                && let Some(text_str) = text_val.as_str()
                && let Ok(mut parsed) = serde_json::from_str::<Value>(text_str)
                && let Some(obj) = parsed.as_object_mut()
            {
                obj.insert(String::from("elapsed_ms"), Value::from(elapsed_ms));
                *text_val =
                    Value::String(serde_json::to_string_pretty(&parsed).unwrap_or_default());
            }

            // Record the emitted result's measured size (UTF-8 bytes of the
            // final tool-result text) so a later `report_spill` can infer the
            // offending size. Recorded after elapsed_ms injection so it
            // reflects exactly what the client received.
            session.last_response_size = result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|c| c.first())
                .and_then(|first| first.get("text"))
                .and_then(Value::as_str)
                .map_or(0, str::len);

            Some(success_response(request_id, &result))
        }
        _ => Some(error_response(
            request_id,
            METHOD_NOT_FOUND,
            &format!("unknown method: {method_name}"),
        )),
    }
}

/// Run the MCP server, reading JSON-RPC messages from stdin and writing
/// responses to stdout.
///
/// The server runs until stdin is closed (EOF).
///
/// `startup_flags` and `startup_assets_dir` carry the identity
/// declaration and assets-dir value supplied on the `remargin mcp`
/// command line. They are re-applied to every request so the server
/// runs under its declared startup identity instead of falling back
/// to the walk-up's nearest `.remargin.yaml`. Per-tool identity
/// declarations are rejected on the MCP surface at the handler
/// layer; use the CLI for per-call identity projection.
///
/// # Errors
///
/// Returns an error if:
/// - Config or registry loading fails
/// - stdin/stdout I/O fails
pub fn run(
    system: &dyn System,
    base_dir: &Path,
    startup_flags: &IdentityFlags,
    startup_assets_dir: Option<&str>,
) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = stdin.lock();
    let mut writer = stdout.lock();

    // Adaptive spill cap lives for the life of the stdio session — it
    // ratchets down as the client reports spills and re-learns on restart.
    let mut session = SessionState::default();

    for raw_line in reader.lines() {
        let line = raw_line.context("reading from stdin")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let message: Value = match serde_json::from_str(trimmed) {
            Ok(val) => val,
            Err(err) => {
                let resp =
                    error_response(&Value::Null, PARSE_ERROR, &format!("invalid JSON: {err}"));
                writeln!(writer, "{resp}").context("writing to stdout")?;
                writer.flush().context("flushing stdout")?;
                continue;
            }
        };

        // Re-resolve on every request so changes to .remargin.yaml are
        // picked up without restarting the MCP server. The same walk
        // feeds the dispatch-time boundary check so the boundary
        // mirrors the per-op `op_guard` view of the world.
        let config = ResolvedConfig::resolve(system, base_dir, startup_flags, startup_assets_dir)?;
        let permissions = resolve_permissions(system, base_dir)?;

        if let Some(response) = process_message(
            system,
            base_dir,
            &permissions,
            &config,
            &mut session,
            &message,
        ) {
            writeln!(writer, "{response}").context("writing to stdout")?;
            writer.flush().context("flushing stdout")?;
        }
    }

    Ok(())
}

/// Process a single JSON-RPC request string and return the response string.
///
/// This is the testable core of the server -- no stdin/stdout involved.
/// Returns `None` for notifications.
///
/// # Errors
///
/// Returns an error if the input is not valid JSON.
pub fn process_request(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    request_json: &str,
) -> Result<Option<String>> {
    let mut session = SessionState::default();
    process_request_with_session(system, base_dir, config, &mut session, request_json)
}

/// Process a single JSON-RPC request against a caller-owned [`SessionState`],
/// so adaptive state (the spill cap, last-response size) persists across
/// several requests — the shape the run loop drives, and the seam the
/// spill-cap tests exercise.
///
/// # Errors
///
/// Returns an error if the input is not valid JSON.
fn process_request_with_session(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    session: &mut SessionState,
    request_json: &str,
) -> Result<Option<String>> {
    let permissions = resolve_permissions(system, base_dir)?;
    let message: Value = serde_json::from_str(request_json).context("parsing JSON-RPC request")?;
    let response = process_message(system, base_dir, &permissions, config, session, &message);
    Ok(response.map(|val| val.to_string()))
}
