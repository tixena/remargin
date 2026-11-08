//! One argument struct per MCP tool, each declaring the tool's input schema as a Rust type.

extern crate alloc;

use alloc::collections::BTreeMap;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tixschema::model_schema;

#[cfg(test)]
mod tests;

/// The arguments of `ack`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AckArgs {
    /// Path to the document (omit to resolve by ID across the folder tree)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Comment IDs to acknowledge
    pub ids: Vec<String>,
    /// Base directory to search when resolving by ID (default: .)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Remove this identity's ack instead of adding one
    #[serde(default)]
    pub remove: bool,
}

/// The arguments of `activity`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ActivityArgs {
    /// Return at most this many changes. Omit for as many as fit the size budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1)]
    pub limit: Option<u64>,
    /// Number of changes to skip before this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 0)]
    pub offset: Option<u64>,
    /// File or directory to scan; defaults to the MCP server's working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// ISO 8601 cutoff. Omit to derive per-file from caller's last action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

/// The arguments of `batch`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BatchArgs {
    /// Path to the document
    pub file: String,
    /// List of comment operations
    pub operations: Vec<BatchOpArgs>,
}

/// One operation of the `batch` tool's `operations` argument.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BatchOpArgs {
    /// Required when `auto_ack:false` skips acking another author's comment: explain why you are not acknowledging it. Not needed for self-replies or the smart default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_skip_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_comment: Option<String>,
    /// ATX heading path; resolved at write time. Mutually exclusive with `after_line/after_comment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_line: Option<u64>,
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Acknowledge the parent comment when replying. If omitted, the parent is auto-acked iff its author differs from the caller (replies to your own comment don't auto-ack). Pass true to force the ack, false to skip it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_ack: Option<bool>,
    /// Comment body text in markdown (bold/italic/links/code-blocks/lists render). Record observed or completed state - not future-tense announcements ("I'll do X"). Write it the way you'd want to re-read it in three months.
    pub content: String,
    /// Classification tags. Each entry must match [A-Za-z0-9_ \-]{1,15}; at most 8 entries.
    #[serde(default)]
    pub kind: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Atomically stage the file in the caller's sandbox (see `sandbox_add`). The file is staged once when any operation sets it.
    #[serde(default)]
    pub sandbox: bool,
    /// Names from the registry who must see this in their pending queue. Omit for a non-actionable note. Use a parent author's name only when your reply requires their response - replies do NOT auto-include the parent's author.
    #[serde(default)]
    pub to: Vec<String>,
}

/// The arguments of `comment`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CommentArgs {
    /// Required when `auto_ack:false` skips acking another author's comment: explain why you are not acknowledging it. Not needed for self-replies or the smart default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_skip_reason: Option<String>,
    /// Insert after this comment ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_comment: Option<String>,
    /// Insert after the ATX heading addressed by this `>`-separated path. Mutually exclusive with `after_line/after_comment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_heading: Option<String>,
    /// Insert after this line number (1-indexed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_line: Option<u64>,
    /// File paths to upload. Each file is copied beside the document and listed in the comment's `attachments` header - this stores the file, it does not display it. To show an image inline, also write `![alt](assets/<filename>)` in the comment body.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Acknowledge the parent comment when replying. If omitted, the parent is auto-acked iff its author differs from the caller (replies to your own comment don't auto-ack). Pass true to force the ack, false to skip it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_ack: Option<bool>,
    /// Comment body text in markdown (bold/italic/links/code-blocks/lists render). Record observed or completed state - not future-tense announcements ("I'll do X"). Write it the way you'd want to re-read it in three months.
    pub content: String,
    /// Path to the document
    pub file: String,
    /// Classification tags. Each entry must match [A-Za-z0-9_ \-]{1,15}; at most 8 entries.
    #[serde(default)]
    pub kind: Vec<String>,
    /// ID of the comment to reply to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Atomically stage the file in the caller's sandbox (see `sandbox_add`)
    #[serde(default)]
    pub sandbox: bool,
    /// Names from the registry who must see this in their pending queue. Omit for a non-actionable note. Use a parent author's name only when your reply requires their response - replies do NOT auto-include the parent's author.
    #[serde(default)]
    pub to: Vec<String>,
}

/// The arguments of `comments`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CommentsArgs {
    /// Path to the document
    pub file: String,
    /// Add the checksum and signature columns immediately before content.
    #[serde(default)]
    pub include_integrity: bool,
    /// Keep only comments whose kind tags include at least one of these values. Empty = no filter.
    #[serde(default)]
    pub kind: Vec<String>,
    /// Return at most this many comments. Omit for as many as fit the size budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1)]
    pub limit: Option<u64>,
    /// Number of matching comments to skip before this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 0)]
    pub offset: Option<u64>,
    /// Keep only pending comments: directed comments some recipient has not acknowledged, and broadcast comments nobody has acknowledged.
    #[serde(default)]
    pub pending: bool,
    /// Keep only comments addressed to this identity that it has not acknowledged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_for: Option<String>,
    /// Same as `pending_for` set to the MCP server's own identity.
    #[serde(default)]
    pub pending_for_me: bool,
}

/// The arguments of `cp`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CpArgs {
    /// Destination path.
    pub dst: String,
    /// Overwrite destination if it exists.
    #[serde(default)]
    pub force: bool,
    /// Source path.
    pub src: String,
}

/// The arguments of `delete`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DeleteArgs {
    /// Path to the document
    pub file: String,
    /// Comment IDs to delete
    pub ids: Vec<String>,
}

/// The arguments of `doctor`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DoctorArgs {
    /// Comma-separated check names to run (default: all). e.g. "hook,trusted-root-escape". The hook-installed gate always runs regardless of this selection and still skips every later check when the hook is missing, but a `hook_missing` finding is reported only when "hook" is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// Path to the user-scope Claude settings file (default: ~/.claude/settings.json).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_settings_file: Option<String>,
}

/// The arguments of `edit`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct EditArgs {
    /// New comment body
    pub content: String,
    /// Path to the document
    pub file: String,
    /// Comment ID to edit
    pub id: String,
    /// Replacement classification tags. Omit to preserve the stored list; pass [] to clear. Each entry must match [A-Za-z0-9_ \-]{1,15}; at most 8 entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<String>>,
}

/// The arguments of `get`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GetArgs {
    /// Return raw bytes base64-encoded; rejects .md
    #[serde(default)]
    pub binary: bool,
    /// End line (1-indexed). Text mode only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u64>,
    /// Prefix each line with its 1-indexed line number. Text mode only.
    #[serde(default)]
    pub line_numbers: bool,
    /// Path to the file
    pub path: String,
    /// Start line (1-indexed). Text mode only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
}

/// The arguments of `get_image`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GetImageArgs {
    /// Optional pixel crop applied before scaling, formatted X,Y,W,H (origin top-left). Clamped to the image bounds; an origin outside the image is rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<String>,
    /// Output format. Defaults to jpeg for photographic source images (JPEG / WebP) and png for lossless source images (PNG / GIF).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ImageFormat>,
    /// Target ceiling on the encoded output size in bytes. JPEG quality is stepped down (and then the dimension cap halved) until this fits. Defaults to 256 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1024)]
    pub max_bytes: Option<u64>,
    /// Upper bound (in pixels) on the longer edge of the output. Defaults to 1024.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1)]
    pub max_dimension: Option<u64>,
    /// Path to the image attachment.
    pub path: String,
}

/// The arguments of `identity_create`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdentityCreateArgs {
    /// Identity (author name) to record.
    pub identity: String,
    /// Optional path to the signing key (emitted verbatim; no existence check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Author type.
    pub r#type: IdentityKind,
}

/// The arguments of `lint`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LintArgs {
    /// Path to the document
    pub file: String,
}

/// The arguments of `ls`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LsArgs {
    /// Directory path to list
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// The arguments of `metadata`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct MetadataArgs {
    /// Path to the file (any allowlisted extension)
    pub path: String,
}

/// The arguments of `mv`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct MvArgs {
    /// Destination path.
    pub dst: String,
    /// Overwrite an existing destination (for directories: removes the existing destination subtree before the rename).
    #[serde(default)]
    pub force: bool,
    /// Source path (file or directory).
    pub src: String,
}

/// The arguments of `permissions_check`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PermissionsCheckArgs {
    /// Absolute or relative path to test.
    pub path: String,
    /// Include the matching rule details when restricted.
    #[serde(default)]
    pub why: bool,
}

/// The arguments of `permissions_show`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct PermissionsShowArgs;

impl<'de> Deserialize<'de> for PermissionsShowArgs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        read_no_fields(deserializer).map(|()| Self)
    }

    fn deserialize_in_place<D>(deserializer: D, place: &mut Self) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        *place = Self::deserialize(deserializer)?;
        Ok(())
    }
}

/// The arguments of `plan`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PlanArgs {
    /// For comment/reply: required when `auto_ack:false` skips acking another author's comment. Mirrors the live reply gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_skip_reason: Option<String>,
    /// Insert after this comment ID (used by comment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_comment: Option<String>,
    /// Insert after the ATX heading addressed by this `>`-separated path (used by comment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_heading: Option<String>,
    /// Insert after this 1-indexed line (used by comment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_line: Option<u64>,
    /// For sign: project signing every unsigned comment authored by the current identity. Mutually exclusive with `ids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_mine: Option<bool>,
    /// Attachment basenames to record on the projected comment. Bytes are NOT copied by plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_names: Option<Vec<String>>,
    /// For comment replies: auto-ack the parent. If omitted, the parent is auto-acked iff its author differs from the caller. Pass true to force the ack, false to skip it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_ack: Option<bool>,
    /// For write: content is base64 binary (implies raw)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<bool>,
    /// Body text (used by comment / edit)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// For write: create the file if missing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<bool>,
    /// For mv: destination path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst: Option<String>,
    /// Emoji for react op
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// Path to the document (required for wired document ops)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// For mv: project the --force overwrite semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    /// Single comment ID (used by edit / react)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Comment IDs (used by ack / delete / sign)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    /// For write: partial-write line range as START-END (1-indexed inclusive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<String>,
    /// Op to project: ack | batch | comment | reply | delete | edit | react | cp | mv | purge | sandbox-add | sandbox-remove | sign | write
    pub op: String,
    /// Sub-ops for the batch projection. Each entry has the same shape as a `batch` sub-op: content (required), `reply_to`, `after_comment`, `after_heading`, `after_line`, `attach_names`, `auto_ack`, `ack_skip_reason`, kind, sandbox, to. An unknown field refuses the projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<PlanOpArgs>>,
    /// For reply: the comment being replied to (required for op=reply; the same as `reply_to`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// For write: treat content as raw bytes; plan returns a non-Markdown `reject_reason`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<bool>,
    /// For purge: project a directory-level purge of every document under `file`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    /// For ack / react: remove instead of add
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,
    /// Parent comment ID (used by comment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// For comment: atomically project a sandbox entry
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<bool>,
    /// For mv: source path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// Addressees (used by comment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<String>>,
}

/// One operation of the `plan` tool's `ops` argument.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PlanOpArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_skip_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_ack: Option<bool>,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<String>>,
}

/// The arguments of `prompt_delete`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PromptDeleteArgs {
    /// Folder containing the `.remargin.yaml`. Defaults to the MCP root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// The arguments of `prompt_list`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PromptListArgs {
    /// Root folder. Defaults to the MCP root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// The arguments of `prompt_resolve`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PromptResolveArgs {
    /// Path to the file (or directory) to resolve a prompt for.
    pub file: String,
}

/// The arguments of `prompt_set`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PromptSetArgs {
    /// Folder containing the `.remargin.yaml`. Defaults to the MCP root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Human-readable display label. Required.
    pub name: String,
    /// Prompt body text. Required.
    pub prompt: String,
    /// Single-line command the composed prompt is piped into. Omitting it clears any existing runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
}

/// The arguments of `purge`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PurgeArgs {
    /// Path to the document (or directory when `recursive` is true)
    pub file: String,
    /// Recursively purge every visible .md file under `file` (treats `file` as a directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
}

/// The arguments of `query`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct QueryArgs {
    /// Only documents with comments by this author
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Only documents containing a comment with this structural ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<String>,
    /// Regex applied to comment content; composes with metadata filters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_regex: Option<String>,
    /// Include individual matching comments in each result (default: true via non-summary mode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// Case-insensitive match for `content_regex`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_case: Option<bool>,
    /// Add checksum + signature columns to each comment row, immediately before content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_integrity: Option<bool>,
    /// Keep only comments whose kind tags include at least one of these values. Empty = no filter.
    #[serde(default)]
    pub kind: Vec<String>,
    /// Return at most this many comments (files in summary mode). Omit for as many as fit the size budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1)]
    pub limit: Option<u64>,
    /// Number of matching comments (files in summary mode) to skip before this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 0)]
    pub offset: Option<u64>,
    /// Base directory to search
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Only documents with pending (unacked) comments. Matches both directed and broadcast shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<bool>,
    /// Only surface broadcast (no-`to`) comments the server identity has not acked yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_broadcast: Option<bool>,
    /// Only pending for this recipient
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_for: Option<String>,
    /// Sugar for `pending_for`=<server identity>. Surfaces directed comments addressed to the caller and not yet acked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_for_me: Option<bool>,
    /// Only activity after this ISO 8601 timestamp
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Return only counts/summary, suppress comment data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<bool>,
}

/// The arguments of `react`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReactArgs {
    /// Emoji to add/remove
    pub emoji: String,
    /// Path to the document
    pub file: String,
    /// Comment ID
    pub id: String,
    /// Remove instead of add
    #[serde(default)]
    pub remove: bool,
}

/// The arguments of `replace`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReplaceArgs {
    /// Case-insensitive matching
    #[serde(default)]
    pub ignore_case: bool,
    /// Target file or directory (required)
    pub path: String,
    /// Text or regex to find
    pub pattern: String,
    /// Treat pattern as a regex
    #[serde(default)]
    pub regex: bool,
    /// Replacement text ($1/${name} in regex mode; literal otherwise)
    pub replacement: String,
}

/// The arguments of `reply`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReplyArgs {
    /// Required when `auto_ack:false` skips acking another author's comment: explain why you are not acknowledging it. Not needed for self-replies or the smart default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_skip_reason: Option<String>,
    /// File paths to upload. Each file is copied beside the document and listed in the comment's `attachments` header - this stores the file, it does not display it. To show an image inline, also write `![alt](assets/<filename>)` in the comment body.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Force the smart default off. true = always ack the parent; false = never ack. Omit for the smart default: ack iff parent.author differs from caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_ack: Option<bool>,
    /// Reply body text in markdown. Past-tense / observed-state only - no future-tense promises.
    pub content: String,
    /// Path to the document containing the parent comment
    pub file: String,
    /// Classification tags. Each entry must match [A-Za-z0-9_ \-]{1,15}; at most 8 entries.
    #[serde(default)]
    pub kind: Vec<String>,
    /// ID of the comment you're replying to. Must exist in `file`.
    pub parent_id: String,
    /// Atomically stage the file in the caller's sandbox (see `sandbox_add`)
    #[serde(default)]
    pub sandbox: bool,
    /// Additional recipients beyond the parent's author. NOT auto-populated with the parent's author - be explicit if you want them paged.
    #[serde(default)]
    pub to: Vec<String>,
}

/// The arguments of `report_spill`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReportSpillArgs {
    /// Optional explicit size (bytes) of the spilled result. Omit to let remargin infer it from the last emitted result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 0)]
    pub size: Option<u64>,
}

/// The arguments of `rm`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RmArgs {
    /// Path to the file or directory to delete
    pub path: String,
}

/// The arguments of `sandbox_add`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SandboxAddArgs {
    /// Paths to markdown files to stage
    pub files: Vec<String>,
}

/// The arguments of `sandbox_list`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SandboxListArgs {
    /// Base directory to walk (default: .)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// The arguments of `sandbox_remove`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SandboxRemoveArgs {
    /// Paths to markdown files to unstage
    pub files: Vec<String>,
}

/// The arguments of `search`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SearchArgs {
    /// Lines of context around matches
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// Case-insensitive matching
    #[serde(default)]
    pub ignore_case: bool,
    /// Page size: return at most this many matches. Omit for all matches. The response always carries the exact total, so a bounded page is never a silent truncation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1)]
    pub limit: Option<u64>,
    /// Number of matches to skip before the returned page
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 0)]
    pub offset: Option<u64>,
    /// Base directory to search
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Text or regex pattern to search for
    pub pattern: String,
    /// Treat pattern as a regex
    #[serde(default)]
    pub regex: bool,
    /// Search scope: all, body, or comments
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// The arguments of `sign`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SignArgs {
    /// Sign every unsigned comment authored by the current identity.
    #[serde(default)]
    pub all_mine: bool,
    /// Path to the document
    pub file: String,
    /// Comment ids to sign. Mutually exclusive with `all_mine`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    /// Recompute each target comment's checksum from its current content before signing. Scoped by the forgery guard: only the caller's own comments can be repaired.
    #[serde(default)]
    pub repair_checksum: bool,
}

/// The arguments of `verify`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VerifyArgs {
    /// Backward-compatible alias for `path` (single document). Prefer `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Path to a document or a directory to verify recursively.
    pub path: String,
}

/// The arguments of `whoami`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct WhoamiArgs;

impl<'de> Deserialize<'de> for WhoamiArgs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        read_no_fields(deserializer).map(|()| Self)
    }

    fn deserialize_in_place<D>(deserializer: D, place: &mut Self) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        *place = Self::deserialize(deserializer)?;
        Ok(())
    }
}

/// The arguments of `write`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WriteArgs {
    /// Content is base64-encoded binary data. Implies raw mode. Not supported for markdown (.md) files.
    #[serde(default)]
    pub binary: bool,
    /// File content to write (base64-encoded when binary=true)
    pub content: String,
    /// Create a new file, creating any missing parent directories (within the sandbox); the file itself must not already exist
    #[serde(default)]
    pub create: bool,
    /// Partial write: last line of the range to replace (1-indexed, inclusive). Must be paired with `start_line`. Incompatible with create/raw/binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1)]
    pub end_line: Option<u64>,
    /// Path to the file
    pub path: String,
    /// Write content exactly as provided, skipping frontmatter injection and comment preservation. Not supported for markdown (.md) files.
    #[serde(default)]
    pub raw: bool,
    /// Partial write: first line of the range to replace (1-indexed, inclusive). Must be paired with `end_line`. Incompatible with create/raw/binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[model_schema_prop(minimum = 1)]
    pub start_line: Option<u64>,
}

/// The author type an identity created by `identity_create` carries.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum IdentityKind {
    Agent,
    Human,
}

/// The encodings `get_image` accepts.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ImageFormat {
    Jpeg,
    Jpg,
    Png,
}

/// Reads an object that must be empty, naming the first key of any other.
fn read_no_fields<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    let fields = BTreeMap::<String, Value>::deserialize(deserializer)?;
    fields.into_keys().next().map_or(Ok(()), |key| {
        Err(D::Error::custom(format_args!(
            "unknown field `{key}`, there are no fields"
        )))
    })
}
