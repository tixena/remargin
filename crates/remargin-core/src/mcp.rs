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

use crate::config::identity::{IdentityFlags, resolve_identity};
use crate::config::{ResolvedConfig, parse_author_type};
use crate::display;
use crate::document;
use crate::linter;
use crate::operations;
use crate::operations::batch::BatchCommentOp;
use crate::operations::migrate;
use crate::operations::plan as plan_ops;
use crate::operations::projections;
use crate::operations::purge;
use crate::operations::query::{self, QueryFilter};
use crate::operations::sandbox as sandbox_ops;
use crate::operations::search;
use crate::parser;
use crate::path::expand_path;
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
/// (rem-x2bw / rem-zlx3): they are pre-expanded here so the identity resolver sees the
/// same already-canonical paths the CLI feeds to [`resolve_identity`].
const SCALAR_PATH_FIELDS: &[&str] = &["config_path", "file", "key", "path"];

/// Array-valued path fields — each element is expanded independently.
const ARRAY_PATH_FIELDS: &[&str] = &["files", "attachments"];

/// Description of a single MCP tool.
struct ToolDesc {
    /// Human-readable description.
    description: &'static str,
    /// Tool name (short, no prefix).
    name: &'static str,
    /// JSON Schema for the tool's input parameters.
    schema: Value,
}

/// Merge the identity-flag schema fragment into a per-tool schema. The tool's
/// own `properties` map gets the four identity fields; its top-level
/// constraints get the exclusivity clause. Today no tool declares a
/// top-level `not` of its own; if that changes, the merge needs to
/// compose clauses rather than overwrite.
fn with_identity_flag_schema(mut base: Value) -> Value {
    let Some(base_obj) = base.as_object_mut() else {
        return base;
    };

    let base_props = base_obj
        .entry(String::from("properties"))
        .or_insert_with(|| json!({}));
    if let (Some(props_map), Value::Object(overlay_props)) = (
        base_props.as_object_mut(),
        json!({
            "config_path": {
                "type": "string",
                "description": "Path to a .remargin.yaml that declares a complete identity. Mutually exclusive with identity / type / key."
            },
            "identity": {
                "type": "string",
                "description": "Declare the identity for this operation. Use together with type (and key in strict mode), or alone to filter the identity walk."
            },
            "key": {
                "type": "string",
                "description": "Signing key path (strict mode). Shorthand: a bare name resolves to ~/.ssh/<name>."
            },
            "type": {
                "type": "string",
                "enum": ["human", "agent"],
                "description": "Author type for the declared identity: human or agent."
            }
        }),
    ) {
        for (k, v) in overlay_props {
            props_map.insert(k, v);
        }
    }

    debug_assert!(
        base_obj.get("not").is_none(),
        "with_identity_flag_schema would overwrite an existing top-level `not` clause"
    );
    base_obj.insert(
        String::from("not"),
        json!({
            "allOf": [
                { "required": ["config_path"] },
                {
                    "anyOf": [
                        { "required": ["identity"] },
                        { "required": ["type"] },
                        { "required": ["key"] }
                    ]
                }
            ]
        }),
    );

    base
}

/// Build the ack tool descriptor.
fn desc_ack() -> ToolDesc {
    ToolDesc {
        name: "ack",
        description: "Acknowledge one or more comments (or remove this identity's ack with remove=true). Omit file to resolve by ID across the folder tree.",
        schema: with_identity_flag_schema(json!({
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
        })),
    }
}

/// Build the batch tool descriptor.
fn desc_batch() -> ToolDesc {
    ToolDesc {
        name: "batch",
        description: "Create multiple comments atomically",
        schema: with_identity_flag_schema(json!({
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
                            "auto_ack": { "type": "boolean", "description": "Acknowledge the parent comment when replying", "default": false }
                        },
                        "required": ["content"]
                    },
                    "description": "List of comment operations"
                }
            },
            "required": ["file", "operations"]
        })),
    }
}

/// Build the comment tool descriptor.
fn desc_comment() -> ToolDesc {
    ToolDesc {
        name: "comment",
        description: "Create a comment in a document",
        schema: with_identity_flag_schema(json!({
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
                "sandbox": { "type": "boolean", "description": "Atomically stage the file in the caller's sandbox (see sandbox_add)", "default": false }
            },
            "required": ["file", "content"]
        })),
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
                "pretty": { "type": "boolean", "description": "Return human-readable threaded display instead of JSON", "default": false }
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
        schema: with_identity_flag_schema(json!({
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
        })),
    }
}

/// Build the edit tool descriptor.
fn desc_edit() -> ToolDesc {
    ToolDesc {
        name: "edit",
        description: "Edit a comment (cascading ack clear)",
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "id": { "type": "string", "description": "Comment ID to edit" },
                "content": { "type": "string", "description": "New comment body" }
            },
            "required": ["file", "id", "content"]
        })),
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

/// Build the migrate tool descriptor.
fn desc_migrate() -> ToolDesc {
    ToolDesc {
        name: "migrate",
        description: "Convert old-format comments to remargin format. Optional `human_config` / `agent_config` point at .remargin.yaml files used to attribute and sign migrated comments per legacy role (required for strict mode). To preview without writing, use `plan` with op=\"migrate\" (same fields).",
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "backup": { "type": "boolean", "description": "Create .bak backup", "default": false },
                "human_config": { "type": "string", "description": "Path to .remargin.yaml whose identity attributes/signs migrated user comments" },
                "agent_config": { "type": "string", "description": "Path to .remargin.yaml whose identity attributes/signs migrated agent comments" }
            },
            "required": ["file"]
        })),
    }
}

/// Build the plan tool descriptor.
fn desc_plan() -> ToolDesc {
    ToolDesc {
        name: "plan",
        description: "Dry-run projection for mutating ops (rem-bhk). Returns a PlanReport (noop/would_commit/reject_reason/checksums/changed_line_ranges/comment diff) without touching disk. All ops are wired: ack, batch, comment, delete, edit, migrate, purge, react, sandbox-add, sandbox-remove, sign, write.",
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "description": "Op to project: ack | comment | delete | edit | react | batch | migrate | purge | sandbox-add | sandbox-remove | sign | write" },
                "file": { "type": "string", "description": "Path to the document (required for wired ops)" },
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
                    "description": "Sub-ops for the batch projection. Each entry has the same shape as a `batch` sub-op: content (required), reply_to, after_comment, after_line, attach_names, auto_ack, to.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "reply_to": { "type": "string" },
                            "after_comment": { "type": "string" },
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
        })),
    }
}

/// Build the purge tool descriptor.
fn desc_purge() -> ToolDesc {
    ToolDesc {
        name: "purge",
        description: "Strip all comments from a document. To preview without writing, use `plan` with op=\"purge\".",
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" }
            },
            "required": ["file"]
        })),
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
             server's configured identity (rem-4j91).",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Base directory to search", "default": "." },
                "comment_id": { "type": "string", "description": "Only documents containing a comment with this structural ID" },
                "content_regex": { "type": "string", "description": "Regex applied to comment content; composes with metadata filters" },
                "expanded": { "type": "boolean", "description": "Include individual matching comments in each result (default: true via non-summary mode)", "default": false },
                "ignore_case": { "type": "boolean", "description": "Case-insensitive match for content_regex", "default": false },
                "pending": { "type": "boolean", "description": "Only documents with pending (unacked) comments. Matches both directed and broadcast shapes (rem-4j91).", "default": false },
                "pending_broadcast": { "type": "boolean", "description": "Only surface broadcast (no-`to`) comments the server identity has not acked yet.", "default": false },
                "pending_for": { "type": "string", "description": "Only pending for this recipient" },
                "pending_for_me": { "type": "boolean", "description": "Sugar for pending_for=<server identity>. Surfaces directed comments addressed to the caller and not yet acked.", "default": false },
                "pretty": { "type": "boolean", "description": "Pretty-print results grouped by file with structured display", "default": false },
                "author": { "type": "string", "description": "Only documents with comments by this author" },
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
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "id": { "type": "string", "description": "Comment ID" },
                "emoji": { "type": "string", "description": "Emoji to add/remove" },
                "remove": { "type": "boolean", "description": "Remove instead of add", "default": false }
            },
            "required": ["file", "id", "emoji"]
        })),
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
        schema: with_identity_flag_schema(json!({
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
        })),
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

/// Build the `sandbox_add` tool descriptor.
fn desc_sandbox_add() -> ToolDesc {
    ToolDesc {
        name: "sandbox_add",
        description: "Stage one or more markdown files in the caller's sandbox. Idempotent per identity.",
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Paths to markdown files to stage"
                }
            },
            "required": ["files"]
        })),
    }
}

/// Build the `sandbox_remove` tool descriptor.
fn desc_sandbox_remove() -> ToolDesc {
    ToolDesc {
        name: "sandbox_remove",
        description: "Remove the caller's sandbox entry from one or more markdown files. Idempotent per identity.",
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Paths to markdown files to unstage"
                }
            },
            "required": ["files"]
        })),
    }
}

/// Build the `sandbox_list` tool descriptor.
fn desc_sandbox_list() -> ToolDesc {
    ToolDesc {
        name: "sandbox_list",
        description: "Return all markdown files in the given path that are staged for the caller's identity.",
        schema: with_identity_flag_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Base directory to walk (default: .)", "default": "." }
            }
        })),
    }
}

/// Build the write tool descriptor.
fn desc_write() -> ToolDesc {
    ToolDesc {
        name: "write",
        description: "Write document contents (comment-preserving)",
        schema: with_identity_flag_schema(json!({
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
        })),
    }
}

/// Build the list of all tool descriptors.
fn tool_descriptors() -> Vec<ToolDesc> {
    vec![
        desc_ack(),
        desc_batch(),
        desc_comment(),
        desc_comments(),
        desc_delete(),
        desc_edit(),
        desc_get(),
        desc_lint(),
        desc_ls(),
        desc_metadata(),
        desc_migrate(),
        desc_plan(),
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
/// `text` field without JSON-encoding it.  Use this for pre-formatted,
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

/// Build the CLI-equivalent [`IdentityFlags`] from a tool params map
/// (rem-x2bw). Returns `None` when no identity-declaration field is
/// present, so handlers can fast-path the "use the base config as-is"
/// case.
///
/// Enforces the same exclusivity rule clap enforces on the CLI: when
/// `config_path` is supplied, none of `identity`, `type`, `key` may be.
/// This duplicates the schema-level `not/allOf` clause on purpose —
/// JSON-Schema enforcement varies by MCP client, and the handler is
/// the last defensible checkpoint before `resolve_identity` sees the
/// flags.
fn identity_flags_from_params(params: &Map<String, Value>) -> Result<Option<IdentityFlags>> {
    let config_path = optional_str(params, "config_path");
    let identity = optional_str(params, "identity");
    let type_str = optional_str(params, "type");
    let key = optional_str(params, "key");

    if config_path.is_none() && identity.is_none() && type_str.is_none() && key.is_none() {
        return Ok(None);
    }

    if config_path.is_some() && (identity.is_some() || type_str.is_some() || key.is_some()) {
        bail!(
            "config_path conflicts with identity / type / key: \
             pass one complete identity declaration, not a mix"
        );
    }

    let author_type = type_str.map(parse_author_type).transpose()?;

    Ok(Some(IdentityFlags {
        author_type,
        config_path: config_path.map(PathBuf::from),
        identity: identity.map(String::from),
        key: key.map(String::from),
    }))
}

/// Resolve a per-tool identity declaration from tool parameters and
/// replace the identity fields on a cloned base [`ResolvedConfig`]
/// (rem-x2bw).
///
/// Extracts `{config_path | identity, type, key}` via
/// [`identity_flags_from_params`] and — when any field is present —
/// hands them to [`resolve_identity`], the same three-branch resolver the
/// CLI uses. The resolved identity then replaces the `identity`,
/// `author_type`, and (when present) `key_path` fields on a cloned
/// config; mode, registry, and the rest are preserved from the walk-up
/// resolution that served the MCP request. The returned config is
/// revalidated via the same registry + strict-key gate that fires on
/// construction so no branch can skip enforcement.
fn resolve_identity_from_params(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Option<ResolvedConfig>> {
    let Some(flags) = identity_flags_from_params(params)? else {
        return Ok(None);
    };

    let resolved = resolve_identity(
        system,
        base_dir,
        &config.mode,
        &flags,
        config.registry.as_ref(),
    )?;

    let mut declared = config.clone();
    declared.identity = Some(resolved.identity);
    declared.author_type = Some(resolved.author_type);
    if let Some(key_path) = resolved.key_path {
        declared.key_path = Some(key_path);
    }

    Ok(Some(declared))
}

/// Pick the per-tool declared identity when present, otherwise the
/// server-level default. Used everywhere the handler needs "the config
/// this specific call runs under".
fn effective_config<'cfg>(
    config: &'cfg ResolvedConfig,
    declared: Option<&'cfg ResolvedConfig>,
) -> &'cfg ResolvedConfig {
    declared.unwrap_or(config)
}

/// Resolve insertion position from tool parameters.
///
/// Shim around [`InsertPosition::from_hints`] (rem-3a2): extracts the
/// three placement fields from the JSON params map and delegates the
/// actual precedence rule to core so CLI + MCP cannot disagree on where
/// a comment lands.
fn resolve_insert_position(params: &Map<String, Value>, reply_to: Option<&str>) -> InsertPosition {
    InsertPosition::from_hints(
        reply_to,
        optional_str(params, "after_comment"),
        optional_usize(params, "after_line"),
    )
}

/// Dispatch a tool call to the appropriate library function.
///
/// Returns the tool result as a JSON value (either success or error content).
fn dispatch_tool(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    tool_name: &str,
    params: &Map<String, Value>,
) -> Value {
    // Normalize path-like fields (`~`, `$VAR`, `${VAR}`) before dispatch
    // so every downstream handler sees already-expanded paths. Keeps CLI +
    // MCP in lockstep (rem-3a2). A normalization failure is reported as a
    // tool-level error with the same surface as any other invalid param.
    let normalized = match normalize_path_fields(system, params) {
        Ok(map) => map,
        Err(err) => return tool_result_error(&format!("{err:#}")),
    };
    let p = &normalized;
    let result = match tool_name {
        "ack" => handle_ack(system, base_dir, config, p),
        "batch" => handle_batch(system, base_dir, config, p),
        "comment" => handle_comment(system, base_dir, config, p),
        "comments" => handle_comments(system, base_dir, p),
        "delete" => handle_delete(system, base_dir, config, p),
        "edit" => handle_edit(system, base_dir, config, p),
        "get" => handle_get(system, base_dir, p),
        "lint" => handle_lint(system, base_dir, p),
        "ls" => handle_ls(system, base_dir, config, p),
        "metadata" => handle_metadata(system, base_dir, p),
        "migrate" => handle_migrate(system, base_dir, config, p),
        "plan" => handle_plan(system, base_dir, config, p),
        "purge" => handle_purge(system, base_dir, config, p),
        "query" => handle_query(system, base_dir, config, p),
        "react" => handle_react(system, base_dir, config, p),
        "rm" => handle_rm(system, base_dir, config, p),
        "sandbox_add" => handle_sandbox_add(system, base_dir, config, p),
        "sandbox_list" => handle_sandbox_list(system, base_dir, config, p),
        "sandbox_remove" => handle_sandbox_remove(system, base_dir, config, p),
        "search" => handle_search(system, base_dir, p),
        "sign" => handle_sign(system, base_dir, config, p),
        "verify" => handle_verify(system, base_dir, config, p),
        "write" => handle_write(system, base_dir, config, p),
        _ => return tool_result_error(&format!("unknown tool: {tool_name}")),
    };

    match result {
        Ok(value) => {
            // If the handler returned a pre-built MCP response (has "content"
            // array), pass it through unchanged.  Otherwise wrap it.
            if value.get("content").is_some_and(Value::is_array) {
                value
            } else {
                tool_result_success(&value)
            }
        }
        Err(err) => tool_result_error(&format!("{err:#}")),
    }
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
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

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

    let key = if remove {
        "unacknowledged"
    } else {
        "acknowledged"
    };
    Ok(json!({ key: ids }))
}

/// Handle the `batch` tool: create multiple comments atomically.
fn handle_batch(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());
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

    Ok(json!({ "ids": ids }))
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
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let position = resolve_insert_position(params, reply_to.as_deref());

    let auto_ack = optional_bool(params, "auto_ack");

    let sandbox = optional_bool(params, "sandbox");
    let create_params = operations::CreateCommentParams {
        attachments: &attachments,
        auto_ack,
        content,
        position: &position,
        reply_to: reply_to.as_deref(),
        sandbox,
        to: &to,
    };

    let path = base_dir.join(file);
    let new_id = operations::create_comment(system, &path, cfg, &create_params)?;

    Ok(json!({ "id": new_id }))
}

/// Handle the `comments` tool: list all comments in a document.
fn handle_comments(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let pretty = optional_bool(params, "pretty");

    let path = base_dir.join(file);
    let doc = parser::parse_file(system, &path)?;
    let comments = doc.comments();

    if pretty {
        let formatted = display::format_comments_pretty(file, &comments);
        Ok(tool_result_text(&formatted))
    } else {
        let result: Vec<Value> = comments.iter().map(|cm| serialize_comment(cm)).collect();
        Ok(json!({ "comments": result }))
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
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let path = base_dir.join(file);
    operations::delete_comments(system, &path, cfg, &id_refs)?;

    Ok(json!({ "deleted": ids }))
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
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let path = base_dir.join(file);
    operations::edit_comment(system, &path, cfg, comment_id, new_content)?;

    Ok(json!({ "edited": comment_id }))
}

/// Handle the `get` tool: read a file's contents.
///
/// When `binary: true`, bytes are read through the shared `read_binary`
/// core helper (symmetric with CLI `get --binary`) and returned base64-
/// encoded alongside size + mime. Markdown files are rejected in this mode
/// so comment-preservation is never bypassed (rem-cdr).
fn handle_get(system: &dyn System, base_dir: &Path, params: &Map<String, Value>) -> Result<Value> {
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
        let payload = document::read_binary(system, base_dir, target, false)?;
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

    let content = document::get(system, base_dir, target, lines, false, false)?;

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

/// Handle the `lint` tool: run structural lint checks.
fn handle_lint(system: &dyn System, base_dir: &Path, params: &Map<String, Value>) -> Result<Value> {
    let file = required_str(params, "file")?;

    let path = base_dir.join(file);
    let content = system
        .read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;

    let errors = linter::lint(&content)?;

    let results: Vec<Value> = errors
        .iter()
        .map(|err| {
            json!({
                "line": err.line,
                "message": err.message
            })
        })
        .collect();

    Ok(json!({
        "errors": results,
        "ok": results.is_empty()
    }))
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

    let results: Vec<Value> = entries.iter().map(serialize_list_entry).collect();

    Ok(json!({ "entries": results }))
}

/// Handle the `metadata` tool: get document metadata.
fn handle_metadata(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let target = Path::new(path_str);

    let meta = document::metadata(system, base_dir, target, false)?;

    // File-level fields are always present; markdown fields are emitted only
    // when the file was parsed (rem-lqz).
    let mut result = json!({
        "binary": meta.binary,
        "mime": meta.mime,
        "path": meta.path,
        "size_bytes": meta.size_bytes,
    });
    let map = result.as_object_mut().unwrap();

    if let Some(count) = meta.comment_count {
        map.insert("comment_count".into(), json!(count));
    }
    if let Some(count) = meta.line_count {
        map.insert("line_count".into(), json!(count));
    }
    if let Some(count) = meta.pending_count {
        map.insert("pending_count".into(), json!(count));
    }
    if !meta.pending_for.is_empty() {
        map.insert("pending_for".into(), json!(meta.pending_for));
    }
    if let Some(last) = &meta.last_activity {
        map.insert("last_activity".into(), json!(last));
    }
    if let Some(fm) = &meta.frontmatter {
        map.insert("frontmatter".into(), json!(fm));
    }

    Ok(result)
}

/// Handle the `migrate` tool: convert old-format comments.
fn handle_migrate(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let backup = optional_bool(params, "backup");
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let identities = migrate_identities_from_params(system, base_dir, cfg, params)?;
    let path = base_dir.join(file);
    let migrated = migrate::migrate(system, &path, cfg, &identities, backup)?;

    let results: Vec<Value> = migrated
        .iter()
        .map(|m| {
            json!({
                "new_id": m.new_id,
                "original_role": m.original_role
            })
        })
        .collect();

    Ok(json!({ "migrated": results }))
}

/// Resolve the per-role identity flags out of an MCP params map.
///
/// Mirrors the CLI's `resolve_migrate_identities` helper but reads
/// JSON: `human_config` / `agent_config` are optional string paths,
/// each interpreted as branch 1 of `config::identity::resolve_identity`.
/// The resolved `author_type` must match the role; a mismatch is a hard
/// error.
fn migrate_identities_from_params(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<migrate::MigrateIdentities> {
    let human = match optional_str(params, "human_config") {
        None => None,
        Some(s) => Some(resolve_role_identity_for_mcp(
            system,
            base_dir,
            cfg,
            Path::new(s),
            &parser::AuthorType::Human,
            "human_config",
        )?),
    };
    let agent = match optional_str(params, "agent_config") {
        None => None,
        Some(s) => Some(resolve_role_identity_for_mcp(
            system,
            base_dir,
            cfg,
            Path::new(s),
            &parser::AuthorType::Agent,
            "agent_config",
        )?),
    };
    Ok(migrate::MigrateIdentities::new(human, agent))
}

fn resolve_role_identity_for_mcp(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    config_path: &Path,
    expected_type: &parser::AuthorType,
    field_name: &str,
) -> Result<migrate::MigrateRoleIdentity> {
    let resolved_path = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        base_dir.join(config_path)
    };
    let flags = IdentityFlags::for_config_path(resolved_path.clone());
    let resolved = resolve_identity(system, base_dir, &cfg.mode, &flags, cfg.registry.as_ref())
        .with_context(|| format!("resolving {field_name} {}", resolved_path.display()))?;
    if &resolved.author_type != expected_type {
        bail!(
            "{field_name} resolved {:?} as type {:?}, but the field requires type {:?}",
            resolved.identity,
            resolved.author_type,
            expected_type,
        );
    }
    Ok(migrate::MigrateRoleIdentity::new(
        resolved.identity,
        resolved.key_path,
    ))
}

/// Handle the `plan` tool: parse the request shape, build a
/// [`plan_ops::PlanRequest`], and delegate to the canonical
/// [`plan_ops::dispatch`] (rem-oqv / rem-3a2). The adapter-layer work is
/// limited to JSON field extraction and the final `serde_json::to_value`.
fn handle_plan(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let op = required_str(params, "op")?;
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

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
        "migrate" => {
            let identities = migrate_identities_from_params(system, base_dir, cfg, params)?;
            plan_ops::PlanRequest::Migrate {
                path: base_dir.join(required_str(params, "file")?),
                identities,
            }
        }
        "purge" => plan_ops::PlanRequest::Purge {
            path: base_dir.join(required_str(params, "file")?),
        },
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

/// Handle the `purge` tool: strip all comments from a document.
fn handle_purge(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let path = base_dir.join(file);
    let result = purge::purge(system, &path, cfg)?;

    Ok(json!({
        "comments_removed": result.comments_removed,
        "attachments_cleaned": result.attachments_cleaned
    }))
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
        let entries: Vec<Value> = results.iter().map(serialize_query_result).collect();
        Ok(
            json!({ "base_path": format!("{}/", path_str.trim_end_matches('/')), "results": entries }),
        )
    }
}

/// Translate `query` tool params into a [`QueryFilter`]. Pulled out so
/// `handle_query` stays under the adapter LOC cap (rem-wpq).
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
    let mut filter = QueryFilter {
        author: optional_str(params, "author").map(String::from),
        comment_id: optional_str(params, "comment_id").map(String::from),
        expanded: optional_bool(params, "expanded"),
        pending: optional_bool(params, "pending"),
        pending_for: optional_str(params, "pending_for").map(String::from),
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
    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let path = base_dir.join(file);
    operations::react(system, &path, cfg, comment_id, emoji, remove)?;

    let action = if remove { "removed" } else { "added" };
    Ok(json!({ "action": action, "emoji": emoji, "comment_id": comment_id }))
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

    Ok(json!({
        "deleted": path_str,
        "existed": result.existed,
    }))
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

    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());
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

    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());
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

    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());
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
/// `plan sign` (rem-7y3). `op` labels the error messages so callers can
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

    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let path = base_dir.join(file);
    let options = operations::sign::SignOptions { repair_checksum };
    let result = operations::sign::sign_comments(system, &path, cfg, &selection, options)?;

    let signed: Vec<Value> = result
        .signed
        .iter()
        .map(|e| json!({ "id": e.id, "ts": e.ts }))
        .collect();
    let skipped: Vec<Value> = result
        .skipped
        .iter()
        .map(|e| json!({ "id": e.id, "reason": e.reason }))
        .collect();
    let repaired: Vec<Value> = result
        .repaired
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "old_checksum": e.old_checksum,
                "new_checksum": e.new_checksum,
            })
        })
        .collect();

    Ok(json!({
        "repaired": repaired,
        "signed": signed,
        "skipped": skipped,
    }))
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
    let doc = parser::parse_file(system, &path)?;

    let report = operations::verify::verify_document(&doc, config);
    let results: Vec<Value> = report
        .results
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "checksum_ok": row.checksum_ok,
                "signature": row.signature.as_str(),
            })
        })
        .collect();

    Ok(json!({ "results": results, "ok": report.ok }))
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

    let declared = resolve_identity_from_params(system, base_dir, config, params)?;
    let cfg = effective_config(config, declared.as_ref());

    let opts = document::WriteOptions::new()
        .binary(binary)
        .create(create)
        .lines(lines)
        .raw(raw);
    let target = Path::new(path_str);
    let outcome = document::write(system, base_dir, target, content, cfg, opts)?;

    Ok(outcome.to_json(path_str, binary, raw))
}

/// Serialize a comment to a JSON value for the `comments` tool response.
fn serialize_comment(cm: &parser::Comment) -> Value {
    let author_type = cm.author_type.as_str();
    let mut obj = json!({
        "id": cm.id,
        "author": cm.author,
        "type": author_type,
        "ts": cm.ts.to_rfc3339(),
        "checksum": cm.checksum,
        "content": cm.content,
        "line": cm.line,
    });

    let map = obj.as_object_mut().unwrap();

    if !cm.to.is_empty() {
        map.insert("to".into(), json!(cm.to));
    }
    if let Some(reply_to) = &cm.reply_to {
        map.insert("reply_to".into(), json!(reply_to));
    }
    if let Some(thread) = &cm.thread {
        map.insert("thread".into(), json!(thread));
    }
    if !cm.ack.is_empty() {
        let acks: Vec<Value> = cm
            .ack
            .iter()
            .map(|a| json!({ "author": a.author, "ts": a.ts.to_rfc3339() }))
            .collect();
        map.insert("ack".into(), json!(acks));
    }
    if !cm.reactions.is_empty() {
        map.insert("reactions".into(), json!(cm.reactions));
    }
    if !cm.attachments.is_empty() {
        map.insert("attachments".into(), json!(cm.attachments));
    }
    if let Some(sig) = &cm.signature {
        map.insert("signature".into(), json!(sig));
    }

    obj
}

/// Serialize a list entry to a JSON value for the `ls` tool response.
fn serialize_list_entry(entry: &document::ListEntry) -> Value {
    let mut obj = json!({
        "path": entry.path.display().to_string(),
        "is_dir": entry.is_dir,
    });
    let map = obj.as_object_mut().unwrap();
    if let Some(size) = entry.size {
        map.insert("size".into(), json!(size));
    }
    if let Some(pending) = entry.remargin_pending {
        map.insert("remargin_pending".into(), json!(pending));
    }
    if let Some(last) = &entry.remargin_last_activity {
        map.insert("remargin_last_activity".into(), json!(last));
    }
    obj
}

/// Serialize a query result to a JSON value for the `query` tool response.
fn serialize_query_result(r: &query::QueryResult) -> Value {
    let mut obj = json!({
        "path": r.path.display().to_string(),
        "comment_count": r.comment_count,
        "pending_count": r.pending_count,
    });
    let map = obj.as_object_mut().unwrap();
    if !r.pending_for.is_empty() {
        map.insert("pending_for".into(), json!(r.pending_for));
    }
    if let Some(ts) = &r.last_activity {
        map.insert("last_activity".into(), json!(ts.to_rfc3339()));
    }
    if !r.comments.is_empty() {
        map.insert(
            "comments".into(),
            json!(
                r.comments
                    .iter()
                    .map(serialize_expanded_comment)
                    .collect::<Vec<_>>()
            ),
        );
    }
    obj
}

/// Serialize an [`ExpandedComment`](query::ExpandedComment) to a JSON value.
fn serialize_expanded_comment(cm: &query::ExpandedComment) -> Value {
    let author_type = cm.author_type.as_str();
    json!({
        "id": cm.id,
        "author": cm.author,
        "author_type": author_type,
        "content": cm.content,
        "file": cm.file.display().to_string(),
        "ts": cm.ts.to_rfc3339(),
        "line": cm.line,
        "to": cm.to,
        "ack": cm.ack.iter().map(|a| json!({
            "author": a.author,
            "ts": a.ts.to_rfc3339(),
        })).collect::<Vec<_>>(),
        "reply_to": cm.reply_to,
        "thread": cm.thread,
        "reactions": cm.reactions,
        "attachments": cm.attachments,
        "checksum": cm.checksum,
        "signature": cm.signature,
    })
}

/// Process a single JSON-RPC request and return a response (or `None` for notifications).
fn process_message(
    system: &dyn System,
    base_dir: &Path,
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
            let mut result = dispatch_tool(system, base_dir, config, tool_name, &arguments);
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
/// command line. They are re-applied to every request so clients that
/// don't pass per-tool identity fields inherit the server's declared
/// default (instead of silently falling back to the walk-up's nearest
/// `.remargin.yaml`). Per-tool identity declarations supplied in a
/// tool-call's params supersede this default via
/// [`resolve_identity_from_params`].
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
        // picked up without restarting the MCP server.
        let config = ResolvedConfig::resolve(system, base_dir, startup_flags, startup_assets_dir)?;

        if let Some(response) = process_message(system, base_dir, &config, &message) {
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
    let message: Value = serde_json::from_str(request_json).context("parsing JSON-RPC request")?;
    let response = process_message(system, base_dir, config, &message);
    Ok(response.map(|val| val.to_string()))
}
