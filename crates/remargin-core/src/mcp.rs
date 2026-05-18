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
use crate::display;
use crate::document;
use crate::kind::matches_kind_filter;
use crate::linter;
use crate::operations;
use crate::operations::batch::BatchCommentOp;
use crate::operations::mv as mv_op;
use crate::operations::plan as plan_ops;
use crate::operations::projections;
use crate::operations::prompt as prompt_ops;
use crate::operations::purge;
use crate::operations::query::{self, QueryFilter};
use crate::operations::sandbox as sandbox_ops;
use crate::operations::search;
use crate::parser;
use crate::path::expand_path;
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
    "identity_create",
    "permissions_check",
    "permissions_show",
    "prompt_resolve",
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

/// Reject any identity-declaration flag in `params`. Returns the
/// error message on the first hit. Defense against clients that
/// ignore the schema (which no longer advertises these flags).
fn reject_identity_flags(tool: &str, params: &Map<String, Value>) -> Option<String> {
    // `identity_create` is exempt: its `identity` / `type` / `key`
    // params name the NEW identity being rendered, not a per-call
    // re-declaration of the caller's identity.
    if tool == "identity_create" {
        return None;
    }
    for &flag in REJECTED_IDENTITY_FLAGS {
        if params.contains_key(flag) {
            return Some(format!(
                "identity flag '{flag}' is not supported on MCP tool '{tool}'; use the CLI for \
                 per-call identity projection"
            ));
        }
    }
    None
}

/// Build the `activity` tool descriptor.
fn desc_activity() -> ToolDesc {
    ToolDesc {
        name: "activity",
        description: "Show what's new since X across managed .md files. Walks <path> (file or \
             directory; defaults to the MCP server's working directory) and returns per-file \
             change records (comments, acks, sandbox-adds) sorted by ts. With `since` omitted, \
             the per-file cutoff is the caller's last action in that file; files where the \
             caller has never acted return everything.",
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
        description: "Create multiple comments atomically",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "to": { "type": "array", "items": { "type": "string" }, "default": [] },
                            "reply_to": { "type": "string" },
                            "attachments": { "type": "array", "items": { "type": "string" }, "default": [] },
                            "after_line": { "type": "integer" },
                            "after_comment": { "type": "string" },
                            "after_heading": { "type": "string", "description": "ATX heading path; resolved at write time. Mutually exclusive with after_line/after_comment." },
                            "auto_ack": { "type": "boolean", "description": "Acknowledge the parent comment when replying", "default": false }
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

/// Build the comment tool descriptor.
fn desc_comment() -> ToolDesc {
    ToolDesc {
        name: "comment",
        description: "Create a comment in a document",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "content": { "type": "string", "description": "Comment body text" },
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Addressees of the comment",
                    "default": []
                },
                "reply_to": { "type": "string", "description": "ID of the comment to reply to" },
                "auto_ack": { "type": "boolean", "description": "Acknowledge the parent comment when replying", "default": false },
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
                "pretty": { "type": "boolean", "description": "Return human-readable threaded display instead of JSON", "default": false },
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
        description: "Dry-run projection for mutating ops. Returns a PlanReport (noop/would_commit/reject_reason/checksums/changed_line_ranges/comment diff) without touching disk. Document ops: ack, batch, comment, delete, edit, purge, react, sandbox-add, sandbox-remove, sign, write. File-relocation op: mv - surfaces an `mv_diff` describing canonical src/dst, dst_exists, noop_same_path, idempotent_already_settled. Config ops (claude_restrict / claude_unrestrict) are CLI-only - use `remargin plan claude restrict` / `remargin plan claude unrestrict`.",
        schema: json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "description": "Op to project: ack | comment | delete | edit | react | batch | mv | purge | sandbox-add | sandbox-remove | sign | write" },
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
                "auto_ack": { "type": "boolean", "description": "For comment replies: auto-ack the parent", "default": false },
                "sandbox": { "type": "boolean", "description": "For comment: atomically project a sandbox entry", "default": false },
                "emoji": { "type": "string", "description": "Emoji for react op" },
                "remove": { "type": "boolean", "description": "For ack / react: remove instead of add", "default": false },
                "ops": {
                    "type": "array",
                    "description": "Sub-ops for the batch projection. Each entry has the same shape as a `batch` sub-op: content (required), reply_to, after_comment, after_heading, after_line, attach_names, auto_ack, to.",
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
                "pretty": { "type": "boolean", "description": "Pretty-print results grouped by file with structured display", "default": false },
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

/// Build the rm tool descriptor.
fn desc_rm() -> ToolDesc {
    ToolDesc {
        name: "rm",
        description: "Remove a file from the managed document tree (idempotent)",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to delete" }
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
                "ignore_case": { "type": "boolean", "description": "Case-insensitive matching", "default": false }
            },
            "required": ["pattern"]
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
             Per-comment status plus an aggregate `ok` flag driven by the active mode.",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" }
            },
            "required": ["file"]
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
        description: "Write document contents (comment-preserving)",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file" },
                "content": { "type": "string", "description": "File content to write (base64-encoded when binary=true)" },
                "binary": { "type": "boolean", "description": "Content is base64-encoded binary data. Implies raw mode. Not supported for markdown (.md) files.", "default": false },
                "create": { "type": "boolean", "description": "Create a new file (parent directory must exist, file must not)", "default": false },
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
        desc_delete(),
        desc_edit(),
        desc_get(),
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

/// Build an MCP tool result (success) with raw text content.
///
/// Unlike [`tool_result_success`], this puts the string directly into the
/// `text` field without JSON-encoding it. Use this for pre-formatted,
/// human-readable output.
fn tool_result_text(text: &str) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": text
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
    if let Some(msg) = reject_identity_flags(tool_name, p) {
        return tool_result_error(&msg);
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
        "delete" => handle_delete(system, base_dir, config, p),
        "edit" => handle_edit(system, base_dir, config, p),
        "get" => handle_get(system, base_dir, config, p),
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
        "search" => handle_search(system, base_dir, p),
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

    let auto_ack = optional_bool(params, "auto_ack");

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
    let pretty = optional_bool(params, "pretty");
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
    let comments: Vec<_> = doc
        .comments()
        .into_iter()
        .filter(|cm| matches_kind_filter(cm.kinds(), &kind_filter))
        .collect();

    if pretty {
        let formatted = display::format_comments_pretty(file, &comments);
        Ok(tool_result_text(&formatted))
    } else {
        Ok(json!({ "comments": comments }))
    }
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

    let content = document::get(
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
        let json_lines: Vec<Value> = content
            .split('\n')
            .enumerate()
            .map(|(i, text)| json!({ "line": start_num + i, "text": text }))
            .collect();
        Ok(json!({ "lines": json_lines }))
    } else {
        Ok(json!({ "content": content }))
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

    // `comment` needs an owned `InsertPosition` + attach refs that outlive
    // the `ProjectCommentParams` it feeds into — we stage them here so the
    // borrow survives long enough for the dispatch call below.
    let reply_to_owned;
    let to_owned;
    let attach_names;
    let attach_refs: Vec<&str>;
    let position;

    let request = match op {
        "ack" => plan_ops::PlanRequest::Ack {
            path: base_dir.join(required_str(params, "file")?),
            ids: string_array(params, "ids"),
            remove: optional_bool(params, "remove"),
        },
        "comment" => {
            let file = required_str(params, "file")?;
            let content = required_str(params, "content")?;
            to_owned = string_array(params, "to");
            reply_to_owned = optional_str(params, "reply_to").map(String::from);
            attach_names = string_array(params, "attach_names");
            attach_refs = attach_names.iter().map(String::as_str).collect();
            position = resolve_insert_position(params, reply_to_owned.as_deref());
            let project_params = projections::ProjectCommentParams::new(content, &position)
                .with_attachment_filenames(&attach_refs)
                .with_auto_ack(optional_bool(params, "auto_ack"))
                .with_reply_to(reply_to_owned.as_deref())
                .with_sandbox(optional_bool(params, "sandbox"))
                .with_to(&to_owned);
            plan_ops::PlanRequest::Comment {
                path: base_dir.join(file),
                params: project_params,
            }
        }
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
            ops: parse_plan_batch_ops(params)?,
        },
        "mv" => plan_ops::PlanRequest::Mv {
            src: PathBuf::from(required_str(params, "src")?),
            dst: PathBuf::from(required_str(params, "dst")?),
            force: optional_bool(params, "force"),
        },
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

    if optional_bool(params, "pretty") {
        let output = display::format_query_pretty(&results, filter.pending_label());
        Ok(json!({ "text": output }))
    } else {
        Ok(json!({
            "base_path": format!("{}/", path_str.trim_end_matches('/')),
            "results": results,
        }))
    }
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
    let items: Vec<Value> = listings
        .iter()
        .map(|l| {
            let display_path = l
                .path
                .strip_prefix(&root)
                .unwrap_or(&l.path)
                .display()
                .to_string();
            json!({
                "path": display_path,
                "since": l.since.to_rfc3339(),
            })
        })
        .collect();
    Ok(json!({ "files": items }))
}

/// Handle the `search` tool: search across documents for text matches.
fn handle_search(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let pattern = required_str(params, "pattern")?;
    let path_str = optional_str(params, "path").unwrap_or(".");
    let target = base_dir.join(path_str);
    let regex = optional_bool(params, "regex");
    let ignore_case = optional_bool(params, "ignore_case");
    let context = optional_usize(params, "context").unwrap_or(0);

    let scope = match optional_str(params, "scope").unwrap_or("all") {
        "body" => search::SearchScope::Body,
        "comments" => search::SearchScope::Comments,
        _ => search::SearchScope::All,
    };

    let options = search::SearchOptions::new(String::from(pattern))
        .context_lines(context)
        .ignore_case(ignore_case)
        .regex(regex)
        .scope(scope);

    let results = search::search(system, base_dir, &target, &options)?;

    let matches: Vec<Value> = results
        .iter()
        .map(|m| {
            let mut obj = json!({
                "path": m.path.display().to_string(),
                "line": m.line,
                "text": m.text,
                "location": match m.location {
                    search::MatchLocation::Body => "body",
                    search::MatchLocation::Comment => "comment",
                },
            });
            let map = obj.as_object_mut().unwrap();
            if let Some(id) = &m.comment_id {
                map.insert("comment_id".into(), json!(id));
            }
            if !m.before.is_empty() {
                map.insert("before".into(), json!(m.before));
            }
            if !m.after.is_empty() {
                map.insert("after".into(), json!(m.after));
            }
            obj
        })
        .collect();

    Ok(json!({ "matches": matches }))
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
    let file = required_str(params, "file")?;

    let path = base_dir.join(file);
    let report = operations::verify::verify_and_refresh(system, &path, config)?;
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
            let mut result =
                dispatch_tool(system, base_dir, permissions, config, tool_name, &arguments);
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

        if let Some(response) = process_message(system, base_dir, &permissions, &config, &message) {
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
    let permissions = resolve_permissions(system, base_dir)?;
    let message: Value = serde_json::from_str(request_json).context("parsing JSON-RPC request")?;
    let response = process_message(system, base_dir, &permissions, config, &message);
    Ok(response.map(|val| val.to_string()))
}
