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

use crate::config::ResolvedConfig;
use crate::crypto::{compute_checksum, compute_signature};
use crate::frontmatter;
use crate::id;
use crate::linter;
use crate::operations::copy_attachments;
use crate::parser::{self, AuthorType, Comment};
use crate::writer::{self, InsertPosition};

// ---------------------------------------------------------------------------
// Batch input
// ---------------------------------------------------------------------------

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
    /// Comment body text.
    pub content: String,
    /// ID of the comment this replies to.
    pub reply_to: Option<String>,
    /// Addressees of the comment.
    pub to: Vec<String>,
}

impl BatchCommentOp {
    /// Create a new batch operation with the given content.
    #[must_use]
    pub const fn new(content: String) -> Self {
        Self {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            content,
            reply_to: None,
            to: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

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
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to create comments")?;

    config.can_post(identity)?;

    let author_type = config.author_type.clone().unwrap_or(AuthorType::Human);

    let mut doc = parser::parse_file(system, path)?;

    // Lint before.
    let markdown_before = doc.to_markdown();
    linter::lint_or_fail(&markdown_before)
        .context("document has structural issues before write")?;

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

        let now = Utc::now().fixed_offset();
        let mut comment = Comment {
            ack: Vec::new(),
            attachments: resolved_attachments,
            author: String::from(identity),
            author_type: author_type.clone(),
            checksum,
            content: op.content.clone(),
            fence_depth: 3,
            id: new_id.clone(),
            line: 0, // Placeholder; updated after document write and re-parse.
            reactions: BTreeMap::default(),
            reply_to: reply_to.map(String::from),
            signature: None,
            thread,
            to: op.to.clone(),
            ts: now,
        };

        // Sign if required.
        if config.requires_signature(identity) {
            if let Some(key_path) = &config.key_path {
                let sig = compute_signature(&comment, key_path, system)?;
                comment.signature = Some(sig);
            } else {
                bail!("batch operation {idx}: strict mode requires signing key");
            }
        }

        // Determine insertion position, adjusting AfterLine targets for
        // lines added by previous insertions in this batch.
        let position = resolve_position_adjusted(op, &line_shifts);

        let lines_before = doc.to_markdown().matches('\n').count();

        writer::insert_comment(&mut doc, comment, &position)
            .with_context(|| format!("batch operation {idx}: inserting comment"))?;

        // Record the line shift if this was an AfterLine insertion.
        if let Some(original_target) = op.after_line {
            let lines_after = doc.to_markdown().matches('\n').count();
            let lines_added = lines_after.saturating_sub(lines_before);
            line_shifts.push((original_target, lines_added));
        }

        created_ids.push(new_id);
    }

    // Update frontmatter.
    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Lint after.
    let markdown_after = doc.to_markdown();
    linter::lint_or_fail(&markdown_after)
        .context("document has structural issues after batch write")?;

    // Write with preservation check.
    let expected_added: HashSet<String> = created_ids.iter().cloned().collect();
    let expected_removed: HashSet<String> = HashSet::new();
    writer::write_document(system, path, &doc, &expected_added, &expected_removed)?;

    Ok(created_ids)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
