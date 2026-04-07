//! MCP server: stdio transport for tool integration.
//!
//! Implements the Model Context Protocol (MCP) over stdio transport using
//! JSON-RPC 2.0. Each tool maps 1:1 to a library function.

#[cfg(test)]
mod tests;

use std::io::{self, BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_json::{Map, Value, json};

use crate::config::{CliOverrides, ResolvedConfig, load_config, load_registry};
use crate::crypto;
use crate::display;
use crate::document;
use crate::linter;
use crate::operations;
use crate::operations::batch::BatchCommentOp;
use crate::operations::migrate;
use crate::operations::purge;
use crate::operations::query::{self, QueryFilter};
use crate::operations::search;
use crate::parser::{self, AuthorType};
use crate::writer::InsertPosition;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tool descriptors
// ---------------------------------------------------------------------------

/// Description of a single MCP tool.
struct ToolDesc {
    /// Human-readable description.
    description: &'static str,
    /// Tool name (short, no prefix).
    name: &'static str,
    /// JSON Schema for the tool's input parameters.
    schema: Value,
}

/// Build the ack tool descriptor.
fn desc_ack() -> ToolDesc {
    ToolDesc {
        name: "ack",
        description: "Acknowledge one or more comments. Omit file to resolve by ID across the folder tree.",
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
                "identity": { "type": "string", "description": "Override identity for this operation" },
                "author_type": { "type": "string", "description": "Override author type: human or agent" }
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
                            "auto_ack": { "type": "boolean", "description": "Acknowledge the parent comment when replying", "default": false }
                        },
                        "required": ["content"]
                    },
                    "description": "List of comment operations"
                },
                "identity": { "type": "string", "description": "Override identity for this operation" },
                "author_type": { "type": "string", "description": "Override author type: human or agent" }
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
                "identity": { "type": "string", "description": "Override identity for this operation" },
                "author_type": { "type": "string", "description": "Override author type: human or agent" }
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
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Comment IDs to delete"
                },
                "identity": { "type": "string", "description": "Override identity for this operation" },
                "author_type": { "type": "string", "description": "Override author type: human or agent" }
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
                "identity": { "type": "string", "description": "Override identity for this operation" },
                "author_type": { "type": "string", "description": "Override author type: human or agent" }
            },
            "required": ["file", "id", "content"]
        }),
    }
}

/// Build the get tool descriptor.
fn desc_get() -> ToolDesc {
    ToolDesc {
        name: "get",
        description: "Read a file's contents",
        schema: json!({
            "type": "object",
            "properties": {
                "end_line": { "type": "integer", "description": "End line (1-indexed)" },
                "line_numbers": { "type": "boolean", "description": "Prefix each line with its 1-indexed line number", "default": false },
                "path": { "type": "string", "description": "Path to the file" },
                "start_line": { "type": "integer", "description": "Start line (1-indexed)" }
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
        description: "Get document metadata",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the document" }
            },
            "required": ["path"]
        }),
    }
}

/// Build the migrate tool descriptor.
fn desc_migrate() -> ToolDesc {
    ToolDesc {
        name: "migrate",
        description: "Convert old-format comments to remargin format",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "dry_run": { "type": "boolean", "description": "Preview without writing", "default": false },
                "backup": { "type": "boolean", "description": "Create .bak backup", "default": false }
            },
            "required": ["file"]
        }),
    }
}

/// Build the purge tool descriptor.
fn desc_purge() -> ToolDesc {
    ToolDesc {
        name: "purge",
        description: "Strip all comments from a document",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "dry_run": { "type": "boolean", "description": "Preview without writing", "default": false }
            },
            "required": ["file"]
        }),
    }
}

/// Build the query tool descriptor.
fn desc_query() -> ToolDesc {
    ToolDesc {
        name: "query",
        description: "Search across documents for comments",
        schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Base directory to search", "default": "." },
                "comment_id": { "type": "string", "description": "Only documents containing a comment with this structural ID" },
                "expanded": { "type": "boolean", "description": "Include individual matching comments in each result (default: true via non-summary mode)", "default": false },
                "pending": { "type": "boolean", "description": "Only documents with pending comments", "default": false },
                "pending_for": { "type": "string", "description": "Only pending for this recipient" },
                "author": { "type": "string", "description": "Only documents with comments by this author" },
                "since": { "type": "string", "description": "Only activity after this ISO 8601 timestamp" },
                "summary": { "type": "boolean", "description": "Return only counts/summary, suppress comment data", "default": false }
            },
            "required": []
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
                "remove": { "type": "boolean", "description": "Remove instead of add", "default": false },
                "identity": { "type": "string", "description": "Override identity for this operation" },
                "author_type": { "type": "string", "description": "Override author type: human or agent" }
            },
            "required": ["file", "id", "emoji"]
        }),
    }
}

/// Build the verify tool descriptor.
fn desc_verify() -> ToolDesc {
    ToolDesc {
        name: "verify",
        description: "Verify comment integrity (checksums and signatures)",
        schema: json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the document" },
                "public_key": { "type": "string", "description": "OpenSSH public key for signature verification" }
            },
            "required": ["file"]
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
                "content": { "type": "string", "description": "File content to write" },
                "create": { "type": "boolean", "description": "Create a new file (parent directory must exist, file must not)", "default": false }
            },
            "required": ["path", "content"]
        }),
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
        desc_purge(),
        desc_query(),
        desc_search(),
        desc_react(),
        desc_verify(),
        desc_write(),
    ]
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Parameter extraction helpers
// ---------------------------------------------------------------------------

/// Extract an optional bool field from a JSON object.
fn optional_bool(params: &Map<String, Value>, field: &str) -> bool {
    params.get(field).and_then(Value::as_bool).unwrap_or(false)
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

// ---------------------------------------------------------------------------
// Identity override helper
// ---------------------------------------------------------------------------

/// Apply `identity` and `author_type` overrides from tool parameters to a config.
///
/// Returns a cloned config with overrides applied if either `identity` or
/// `author_type` is present in params. Returns `None` if no overrides.
fn apply_identity_overrides(
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Option<ResolvedConfig>> {
    let identity_override = optional_str(params, "identity");
    let type_override = optional_str(params, "author_type");

    if identity_override.is_none() && type_override.is_none() {
        return Ok(None);
    }

    let mut overridden = config.clone();

    if let Some(id) = identity_override {
        overridden.identity = Some(String::from(id));
    }

    if let Some(type_str) = type_override {
        overridden.author_type = Some(match type_str {
            "human" => AuthorType::Human,
            "agent" => AuthorType::Agent,
            other => anyhow::bail!("unknown author type: {other:?}"),
        });
    }

    Ok(Some(overridden))
}

/// Get the effective config, applying identity overrides if present.
fn effective_config<'cfg>(
    config: &'cfg ResolvedConfig,
    overridden: Option<&'cfg ResolvedConfig>,
) -> &'cfg ResolvedConfig {
    overridden.unwrap_or(config)
}

// ---------------------------------------------------------------------------
// Insert position helper
// ---------------------------------------------------------------------------

/// Resolve insertion position from tool parameters.
fn resolve_insert_position(params: &Map<String, Value>, reply_to: Option<&str>) -> InsertPosition {
    // Replies always go after their parent — explicit placement is ignored.
    if let Some(parent_id) = reply_to {
        return InsertPosition::AfterComment(String::from(parent_id));
    }
    if let Some(after_comment) = optional_str(params, "after_comment") {
        return InsertPosition::AfterComment(String::from(after_comment));
    }
    if let Some(after_line) = optional_usize(params, "after_line") {
        return InsertPosition::AfterLine(after_line);
    }
    InsertPosition::Append
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

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
    let result = match tool_name {
        "ack" => handle_ack(system, base_dir, config, params),
        "batch" => handle_batch(system, base_dir, config, params),
        "comment" => handle_comment(system, base_dir, config, params),
        "comments" => handle_comments(system, base_dir, params),
        "delete" => handle_delete(system, base_dir, config, params),
        "edit" => handle_edit(system, base_dir, config, params),
        "get" => handle_get(system, base_dir, params),
        "lint" => handle_lint(system, base_dir, params),
        "ls" => handle_ls(system, base_dir, config, params),
        "metadata" => handle_metadata(system, base_dir, params),
        "migrate" => handle_migrate(system, base_dir, config, params),
        "purge" => handle_purge(system, base_dir, config, params),
        "query" => handle_query(system, base_dir, params),
        "search" => handle_search(system, base_dir, params),
        "react" => handle_react(system, base_dir, config, params),
        "verify" => handle_verify(system, base_dir, params),
        "write" => handle_write(system, base_dir, config, params),
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

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

/// Handle the `ack` tool: acknowledge one or more comments.
fn handle_ack(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let ids = string_array(params, "ids");
    let overridden = apply_identity_overrides(config, params)?;
    let cfg = effective_config(config, overridden.as_ref());

    if let Some(file) = optional_str(params, "file") {
        // Direct file path provided.
        let path = base_dir.join(file);
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        operations::ack_comments(system, &path, cfg, &id_refs)?;
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
                    operations::ack_comments(system, &matches[0], cfg, &id_refs)?;
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

    Ok(json!({ "acknowledged": ids }))
}

/// Handle the `batch` tool: create multiple comments atomically.
fn handle_batch(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let overridden = apply_identity_overrides(config, params)?;
    let cfg = effective_config(config, overridden.as_ref());
    let ops_value = params
        .get("operations")
        .and_then(Value::as_array)
        .context("missing required field: operations")?;

    let mut batch_ops = Vec::new();
    for (idx, op_value) in ops_value.iter().enumerate() {
        let op_obj = op_value
            .as_object()
            .with_context(|| format!("batch operation {idx}: expected object"))?;

        let content =
            required_str(op_obj, "content").with_context(|| format!("batch operation {idx}"))?;

        batch_ops.push(BatchCommentOp {
            after_comment: optional_str(op_obj, "after_comment").map(String::from),
            after_line: optional_usize(op_obj, "after_line"),
            attachments: string_array(op_obj, "attachments")
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            auto_ack: optional_bool(op_obj, "auto_ack"),
            content: String::from(content),
            reply_to: optional_str(op_obj, "reply_to").map(String::from),
            to: string_array(op_obj, "to"),
        });
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
    let overridden = apply_identity_overrides(config, params)?;
    let cfg = effective_config(config, overridden.as_ref());

    let position = resolve_insert_position(params, reply_to.as_deref());

    let auto_ack = optional_bool(params, "auto_ack");

    let create_params = operations::CreateCommentParams {
        attachments: &attachments,
        auto_ack,
        content,
        position: &position,
        reply_to: reply_to.as_deref(),
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
    let overridden = apply_identity_overrides(config, params)?;
    let cfg = effective_config(config, overridden.as_ref());

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
    let overridden = apply_identity_overrides(config, params)?;
    let cfg = effective_config(config, overridden.as_ref());

    let path = base_dir.join(file);
    operations::edit_comment(system, &path, cfg, comment_id, new_content)?;

    Ok(json!({ "edited": comment_id }))
}

/// Handle the `get` tool: read a file's contents.
fn handle_get(system: &dyn System, base_dir: &Path, params: &Map<String, Value>) -> Result<Value> {
    let path_str = required_str(params, "path")?;
    let end_line = optional_usize(params, "end_line");
    let line_numbers = optional_bool(params, "line_numbers");
    let start_line = optional_usize(params, "start_line");

    let target = Path::new(path_str);
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

    let mut result = json!({
        "comment_count": meta.comment_count,
        "line_count": meta.line_count,
        "pending_count": meta.pending_count,
    });
    let map = result.as_object_mut().unwrap();

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
    let dry_run = optional_bool(params, "dry_run");
    let backup = optional_bool(params, "backup");

    let path = base_dir.join(file);
    let migrated = migrate::migrate(system, &path, config, dry_run, backup)?;

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

/// Handle the `purge` tool: strip all comments from a document.
fn handle_purge(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let dry_run = optional_bool(params, "dry_run");

    let path = base_dir.join(file);
    let result = purge::purge(system, &path, config, dry_run)?;

    Ok(json!({
        "comments_removed": result.comments_removed,
        "attachments_cleaned": result.attachments_cleaned
    }))
}

/// Handle the `query` tool: search across documents.
fn handle_query(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let path_str = optional_str(params, "path").unwrap_or(".");
    let target = base_dir.join(path_str);

    let since = optional_str(params, "since")
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .with_context(|| format!("invalid timestamp: {s}"))
        })
        .transpose()?;

    let filter = QueryFilter {
        author: optional_str(params, "author").map(String::from),
        comment_id: optional_str(params, "comment_id").map(String::from),
        expanded: optional_bool(params, "expanded"),
        pending: optional_bool(params, "pending"),
        pending_for: optional_str(params, "pending_for").map(String::from),
        since,
        summary: optional_bool(params, "summary"),
    };

    let results = query::query(system, &target, &filter)?;

    let entries: Vec<Value> = results.iter().map(serialize_query_result).collect();

    Ok(json!({ "results": entries }))
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
    let overridden = apply_identity_overrides(config, params)?;
    let cfg = effective_config(config, overridden.as_ref());

    let path = base_dir.join(file);
    operations::react(system, &path, cfg, comment_id, emoji, remove)?;

    let action = if remove { "removed" } else { "added" };
    Ok(json!({ "action": action, "emoji": emoji, "comment_id": comment_id }))
}

/// Handle the `verify` tool: verify comment integrity.
fn handle_verify(
    system: &dyn System,
    base_dir: &Path,
    params: &Map<String, Value>,
) -> Result<Value> {
    let file = required_str(params, "file")?;
    let public_key = optional_str(params, "public_key");

    let path = base_dir.join(file);
    let doc = parser::parse_file(system, &path)?;
    let comments = doc.comments();

    let mut results: Vec<Value> = Vec::new();

    for cm in &comments {
        let checksum_ok = crypto::verify_checksum(cm);

        let signature_status = public_key.map_or("not_checked", |pubkey| {
            if cm.signature.is_some() {
                match crypto::verify_signature(cm, pubkey) {
                    Ok(true) => "valid",
                    Ok(false) => "invalid",
                    Err(_) => "error",
                }
            } else {
                "missing"
            }
        });

        results.push(json!({
            "id": cm.id,
            "checksum_ok": checksum_ok,
            "signature": signature_status
        }));
    }

    Ok(json!({ "results": results }))
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
    let create = params
        .get("create")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let target = Path::new(path_str);
    document::write(system, base_dir, target, content, config, create)?;

    Ok(json!({ "written": path_str }))
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

/// Serialize a comment to a JSON value for the `comments` tool response.
fn serialize_comment(cm: &parser::Comment) -> Value {
    let author_type = match cm.author_type {
        parser::AuthorType::Human => "human",
        parser::AuthorType::Agent => "agent",
    };
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
    let author_type = match cm.author_type {
        parser::AuthorType::Agent => "agent",
        parser::AuthorType::Human => "human",
    };
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

// ---------------------------------------------------------------------------
// Message processing
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Public API: run the server
// ---------------------------------------------------------------------------

/// Run the MCP server, reading JSON-RPC messages from stdin and writing
/// responses to stdout.
///
/// The server runs until stdin is closed (EOF).
///
/// # Errors
///
/// Returns an error if:
/// - Config or registry loading fails
/// - stdin/stdout I/O fails
pub fn run(system: &dyn System, base_dir: &Path, overrides: &CliOverrides<'_>) -> Result<()> {
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

        // Reload config on every request so changes to .remargin.yaml
        // are picked up without restarting the MCP server.
        let config_data = load_config(system, base_dir)?;
        let registry = load_registry(system, base_dir)?;
        let config = ResolvedConfig::resolve(system, config_data, registry, overrides)?;

        if let Some(response) = process_message(system, base_dir, &config, &message) {
            writeln!(writer, "{response}").context("writing to stdout")?;
            writer.flush().context("flushing stdout")?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Testable process function (no I/O)
// ---------------------------------------------------------------------------

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
