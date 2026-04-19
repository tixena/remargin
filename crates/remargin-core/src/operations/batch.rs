//! Atomic batch operations.
//!
//! Apply multiple comment creation operations in a single document write.
//! All-or-nothing semantics: if any operation fails, nothing is written.

#[cfg(test)]
mod tests;

extern crate alloc;

use alloc::collections::BTreeMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use os_shim::System;
use serde_json::{Map, Value};

use crate::config::ResolvedConfig;
use crate::crypto::{compute_checksum, compute_signature};
use crate::frontmatter;
use crate::id;
use crate::linter;
use crate::operations::verify::commit_with_verify;
use crate::operations::{copy_attachments, find_comment_mut};
use crate::parser::{self, Acknowledgment, AuthorType, Comment, ParsedDocument};
use crate::writer::{self, InsertPosition};

/// A single comment creation operation within a batch.
#[derive(Debug)]
#[non_exhaustive]
pub struct BatchCommentOp {
    /// ID of a comment this should appear after (position).
    pub after_comment: Option<String>,
    /// Line number to insert after (1-indexed position).
    pub after_line: Option<usize>,
    /// Attachment file paths.
    pub attachments: Vec<PathBuf>,
    /// Automatically acknowledge the parent comment when replying.
    pub auto_ack: bool,
    /// Comment body text.
    pub content: String,
    /// ID of the comment this replies to.
    pub reply_to: Option<String>,
    /// Addressees of the comment.
    pub to: Vec<String>,
}

impl BatchCommentOp {
    /// Decode a single batch-op JSON object into a [`BatchCommentOp`].
    ///
    /// Shared between the CLI (which receives the ops as a top-level
    /// array) and the MCP adapter (which nests them under
    /// `params.operations`). Both surfaces feed one object per call so
    /// accepted field names and error messages stay in one place and
    /// cannot drift.
    ///
    /// `idx` is the zero-based position of the op in its enclosing
    /// array; it is only used to make error messages actionable.
    ///
    /// # Errors
    ///
    /// Returns an error if the required `content` field is missing or
    /// not a string.
    pub fn from_json_object(obj: &Map<String, Value>, idx: usize) -> Result<Self> {
        let content = obj
            .get("content")
            .and_then(Value::as_str)
            .with_context(|| format!("batch op[{idx}]: missing required field `content`"))?;

        Ok(Self {
            after_comment: obj
                .get("after_comment")
                .and_then(Value::as_str)
                .map(String::from),
            after_line: obj
                .get("after_line")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok()),
            attachments: obj
                .get("attachments")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            auto_ack: obj
                .get("auto_ack")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            content: String::from(content),
            reply_to: obj
                .get("reply_to")
                .and_then(Value::as_str)
                .map(String::from),
            to: obj
                .get("to")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Create a new batch operation with the given content.
    #[must_use]
    pub const fn new(content: String) -> Self {
        Self {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            auto_ack: false,
            content,
            reply_to: None,
            to: Vec::new(),
        }
    }
}

/// Execute a batch of comment operations atomically.
///
/// Parses the document once, applies all operations, writes once.
/// If any operation fails, nothing is written.
///
/// Returns the list of created IDs in order.
///
/// # Errors
///
/// Returns an error if:
/// - The author is not allowed to post
/// - Any attachment does not exist
/// - A reply-to reference cannot be resolved
/// - Writing fails
pub fn batch_comment(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    operations: &[BatchCommentOp],
) -> Result<Vec<String>> {
    writer::ensure_not_forbidden_target(path)?;

    let identity = config
        .identity
        .as_deref()
        .context("identity is required to create comments")?;

    // Registry + strict-mode key presence are validated at resolve time
    // (rem-xc8x); this just fetches the signing key when the op needs one.
    let signing_key = config.resolve_signing_key(identity);

    let author_type = config.author_type.clone().unwrap_or(AuthorType::Human);

    let mut doc = parser::parse_file(system, path)?;

    let markdown_before = doc.to_markdown();
    linter::lint_or_fail(&markdown_before)
        .context("document has structural issues before write")?;

    // Validate auto_ack requires reply_to before any modifications.
    for (idx, op) in operations.iter().enumerate() {
        if op.auto_ack && op.reply_to.is_none() {
            bail!("batch operation {idx}: auto_ack requires reply_to");
        }
    }

    let mut created_ids: Vec<String> = Vec::new();

    // Track line shifts from previous AfterLine insertions so subsequent
    // AfterLine targets can be adjusted.  Each entry is (original_target_line,
    // number_of_lines_added_by_that_insertion).
    let mut line_shifts: Vec<(usize, usize)> = Vec::new();

    for (idx, op) in operations.iter().enumerate() {
        let existing_ids = doc.comment_ids();
        let new_id = id::generate(&existing_ids);

        let checksum = compute_checksum(&op.content);

        // Resolve reply-to (may reference an earlier comment in this batch).
        let reply_to = op.reply_to.as_deref();

        // Resolve thread from reply_to.
        let thread = reply_to.map(|parent_id| {
            doc.find_comment(parent_id)
                .and_then(|parent| parent.thread.clone())
                .unwrap_or_else(|| String::from(parent_id))
        });

        // Copy attachments.
        let resolved_attachments = copy_attachments(system, path, config, &op.attachments)
            .with_context(|| format!("batch operation {idx}: copying attachments"))?;

        // Auto-populate `to` from the parent comment's author when replying
        // without an explicit recipient list.
        let effective_to: Vec<String> = if op.to.is_empty() {
            reply_to
                .and_then(|pid| doc.find_comment(pid))
                .map_or_else(Vec::new, |parent| vec![parent.author.clone()])
        } else {
            op.to.clone()
        };

        let now = Utc::now().fixed_offset();
        let mut comment = Comment {
            ack: Vec::new(),
            attachments: resolved_attachments,
            author: String::from(identity),
            author_type: author_type.clone(),
            checksum,
            content: op.content.clone(),
            id: new_id.clone(),
            line: 0, // Placeholder; updated after document write and re-parse.
            reactions: BTreeMap::default(),
            reply_to: reply_to.map(String::from),
            signature: None,
            thread,
            to: effective_to,
            ts: now,
        };

        if let Some(key_path) = signing_key {
            let sig = compute_signature(&comment, key_path, system)?;
            comment.signature = Some(sig);
        }

        // Determine insertion position, adjusting AfterLine targets for
        // lines added by previous insertions in this batch.
        let position = resolve_position_adjusted(op, &line_shifts);

        let lines_before = doc.to_markdown().matches('\n').count();

        writer::insert_comment(&mut doc, comment, &position)
            .with_context(|| format!("batch operation {idx}: inserting comment"))?;

        // Auto-ack the parent comment in the same document write cycle.
        if op.auto_ack
            && let Some(parent_id) = &op.reply_to
        {
            let lines_before_ack = doc.to_markdown().matches('\n').count();

            let parent = find_comment_mut(&mut doc, parent_id).with_context(|| {
                format!("batch operation {idx}: auto-ack parent {parent_id:?} not found")
            })?;
            parent.ack.push(Acknowledgment {
                author: String::from(identity),
                ts: now,
            });

            // Track ack-induced line shift for subsequent AfterLine targets.
            let lines_after_ack = doc.to_markdown().matches('\n').count();
            let ack_lines_added = lines_after_ack.saturating_sub(lines_before_ack);
            if ack_lines_added > 0
                && let Some(parent_cm) = doc.find_comment(parent_id)
            {
                // Use the parent's original line as the shift anchor.
                // The ack metadata is added to the parent's fence, so any
                // AfterLine target at or after the parent's position shifts.
                line_shifts.push((parent_cm.line, ack_lines_added));
            }
        }

        // Record the line shift if this was an AfterLine insertion.
        if let Some(original_target) = op.after_line {
            let lines_after = doc.to_markdown().matches('\n').count();
            let lines_added = lines_after.saturating_sub(lines_before);
            line_shifts.push((original_target, lines_added));
        }

        created_ids.push(new_id);
    }

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    let markdown_after = doc.to_markdown();
    linter::lint_or_fail(&markdown_after)
        .context("document has structural issues after batch write")?;

    write_batch_result(system, path, config, &doc, &created_ids)?;

    Ok(created_ids)
}

/// Write the batch result with preservation check + post-mutation verify gate.
///
/// Per the "verify runs once after all ops complete in-memory" rule in
/// rem-ef1 — batch is atomic end-to-end.
fn write_batch_result(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    doc: &ParsedDocument,
    created_ids: &[String],
) -> Result<()> {
    let expected_added: HashSet<String> = created_ids.iter().cloned().collect();
    let expected_removed: HashSet<String> = HashSet::new();
    commit_with_verify(doc, config, |verified_doc| {
        writer::write_document(
            system,
            path,
            verified_doc,
            &expected_added,
            &expected_removed,
        )
    })
}

/// Resolve the insertion position for a batch operation, adjusting
/// `AfterLine` targets for lines added by previous insertions.
///
/// `line_shifts` contains `(original_target_line, lines_added)` from
/// prior `AfterLine` insertions in this batch.  Any new `AfterLine`
/// whose target is >= a prior target gets shifted by that insertion's
/// line count.
fn resolve_position_adjusted(
    op: &BatchCommentOp,
    line_shifts: &[(usize, usize)],
) -> InsertPosition {
    // Replies always go after their parent — explicit placement is ignored.
    if let Some(parent_id) = &op.reply_to {
        return InsertPosition::AfterComment(parent_id.clone());
    }
    if let Some(after_comment) = &op.after_comment {
        return InsertPosition::AfterComment(after_comment.clone());
    }
    if let Some(target) = op.after_line {
        let mut adjusted = target;
        for &(prev_target, lines_added) in line_shifts {
            if target >= prev_target {
                adjusted += lines_added;
            }
        }
        return InsertPosition::AfterLine(adjusted);
    }
    InsertPosition::Append
}
