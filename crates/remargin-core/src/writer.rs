//! Comment block writer: insert, update, and serialize remargin blocks.
//!
//! This module provides functions for serializing comments to markdown,
//! inserting them into parsed documents, and writing documents to disk
//! with comment preservation verification.

#[cfg(test)]
mod tests;

use core::fmt::Write as _;
use core::iter::repeat_n;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use os_shim::System;

use crate::parser::{self, Comment, ParsedDocument, Segment, required_fence_depth};

/// Filenames the writer refuses to modify under any circumstances.
///
/// Remargin (particularly agents driving the CLI) must never mutate its
/// own configuration or participant registry through the document access
/// layer. Doing so would let a caller silently grant itself registry
/// access, rotate keys, flip modes, or revoke other participants.
///
/// Match is by exact basename only — `backup.remargin.yaml`,
/// `old.remargin-registry.yaml`, etc. are not affected.
pub const FORBIDDEN_TARGETS: &[&str] = &[".remargin.yaml", ".remargin-registry.yaml"];

/// Where to insert a new comment in a document.
#[derive(Debug)]
#[non_exhaustive]
pub enum InsertPosition {
    /// Place after the comment with this ID.
    AfterComment(String),
    /// Place after this line number (1-indexed).
    AfterLine(usize),
    /// Place at the end of the document.
    Append,
}

impl InsertPosition {
    /// Canonical priority-based builder used by every adapter (CLI + MCP)
    /// to turn the three placement knobs (`reply_to`, `after_comment`,
    /// `after_line`) into an [`InsertPosition`] (rem-3a2).
    ///
    /// Precedence: replies always go after their parent (explicit
    /// placement is ignored for replies); `after_comment` beats
    /// `after_line`; absence of all three falls back to
    /// [`InsertPosition::Append`]. Keeping the rule in one place prevents
    /// adapter-layer drift on insertion semantics.
    #[must_use]
    pub fn from_hints(
        reply_to: Option<&str>,
        after_comment: Option<&str>,
        after_line: Option<usize>,
    ) -> Self {
        if let Some(parent_id) = reply_to {
            return Self::AfterComment(String::from(parent_id));
        }
        if let Some(after_id) = after_comment {
            return Self::AfterComment(String::from(after_id));
        }
        after_line.map_or(Self::Append, Self::AfterLine)
    }
}

/// Return an error if `path`'s basename matches [`FORBIDDEN_TARGETS`].
///
/// Every mutating op (write, comment, edit, delete, ack, react, batch,
/// sign, purge, migrate, rm, sandbox) funnels through this guard before
/// any bytes reach disk.
///
/// # Errors
///
/// Returns an error whose message is
/// `refusing to modify <basename>: remargin does not modify its own configuration files`
/// when the path's basename matches a forbidden target.
pub fn ensure_not_forbidden_target(path: &Path) -> Result<()> {
    if let Some(name) = path.file_name().and_then(|n| n.to_str())
        && FORBIDDEN_TARGETS.contains(&name)
    {
        bail!("refusing to modify {name}: remargin does not modify its own configuration files");
    }
    Ok(())
}

/// Serialize a `Comment` into a remargin fenced code block string.
///
/// The YAML fields are emitted in canonical order: id, author, type, ts,
/// to, reply-to, thread, attachments, reactions, ack, checksum, signature.
/// Optional fields are omitted when empty or `None`.
#[must_use]
pub fn serialize_comment(comment: &Comment) -> String {
    let fence_depth = required_fence_depth(&comment.content);
    let fence: String = repeat_n('`', fence_depth).collect();
    let mut out = String::new();

    let _ = writeln!(out, "{fence}remargin");
    out.push_str("---\n");

    // Required fields in canonical order.
    let _ = writeln!(out, "id: {}", comment.id);
    let _ = writeln!(out, "author: {}", comment.author);
    let _ = writeln!(out, "type: {}", comment.author_type.as_str());
    let _ = writeln!(out, "ts: {}", comment.ts.to_rfc3339());

    // Optional fields (only emit if non-default).
    if !comment.to.is_empty() {
        let _ = writeln!(out, "to: [{}]", comment.to.join(", "));
    }
    if let Some(reply_to) = &comment.reply_to {
        let _ = writeln!(out, "reply-to: {reply_to}");
    }
    if let Some(thread) = &comment.thread {
        let _ = writeln!(out, "thread: {thread}");
    }
    if !comment.attachments.is_empty() {
        let _ = writeln!(out, "attachments: [{}]", comment.attachments.join(", "));
    }
    if !comment.reactions.is_empty() {
        out.push_str("reactions:\n");
        for (emoji, authors) in &comment.reactions {
            let _ = writeln!(out, "  {emoji}: [{}]", authors.join(", "));
        }
    }
    if !comment.ack.is_empty() {
        out.push_str("ack:\n");
        for ack_entry in &comment.ack {
            let _ = writeln!(
                out,
                "  - {}@{}",
                ack_entry.author,
                ack_entry.ts.to_rfc3339()
            );
        }
    }

    // Checksum and signature come last.
    let _ = writeln!(out, "checksum: {}", comment.checksum);
    if let Some(sig) = &comment.signature {
        let _ = writeln!(out, "signature: {sig}");
    }

    out.push_str("---\n");

    // Content.
    if !comment.content.is_empty() {
        out.push_str(&comment.content);
        if !comment.content.ends_with('\n') {
            out.push('\n');
        }
    }

    let _ = writeln!(out, "{fence}");
    out
}

/// Insert a new comment into the parsed document at the given position.
///
/// # Errors
///
/// Returns an error if `AfterComment` references a non-existent comment ID.
/// `AfterLine` beyond the document length is clamped to append.
pub fn insert_comment(
    doc: &mut ParsedDocument,
    comment: Comment,
    position: &InsertPosition,
) -> Result<()> {
    let segment = Segment::Comment(Box::new(comment));

    match position {
        InsertPosition::Append => {
            // Ensure there's a newline separator before the new comment.
            if let Some(Segment::Body(text)) = doc.segments.last()
                && !text.ends_with('\n')
            {
                doc.segments.push(Segment::Body(String::from("\n")));
            }
            doc.segments.push(segment);
            doc.segments.push(Segment::Body(String::from("\n")));
        }

        InsertPosition::AfterComment(target_id) => {
            let position_idx = doc
                .segments
                .iter()
                .position(|seg| matches!(seg, Segment::Comment(cm) if cm.id == *target_id))
                .with_context(|| format!("comment with id {target_id:?} not found"))?;

            // Insert after the target comment.
            let insert_at = position_idx + 1;
            doc.segments
                .insert(insert_at, Segment::Body(String::from("\n")));
            doc.segments.insert(insert_at + 1, segment);
            doc.segments
                .insert(insert_at + 2, Segment::Body(String::from("\n")));
        }

        InsertPosition::AfterLine(target_line) => {
            let markdown = doc.to_markdown();
            let lines: Vec<&str> = markdown.split('\n').collect();

            // Clamp to document length so after-line beyond the end
            // behaves as append rather than erroring.
            let clamped = (*target_line).min(lines.len());

            // Find the byte offset after the target line.
            let mut byte_offset: usize = 0;
            for line in lines.iter().take(clamped) {
                byte_offset += line.len() + 1; // +1 for the newline
            }
            // Clamp to string length: split('\n') on a trailing-newline
            // string produces an empty final element whose +1 overshoots.
            byte_offset = byte_offset.min(markdown.len());

            // Rebuild: text before + new comment + text after.
            let before = &markdown[..byte_offset];
            let after = &markdown[byte_offset..];
            let serialized = serialize_comment_from_segment(&segment);

            // Invariant: whatever precedes the opening fence must be empty or
            // end with a newline — otherwise ``` glues onto the last body
            // line and no longer parses as a code fence. The `Append` branch
            // has an analogous guard; files with a trailing newline land in
            // the else arm and output is byte-identical to before this fix.
            let sep = if !before.is_empty() && !before.ends_with('\n') {
                "\n"
            } else {
                ""
            };

            // `serialize_comment()` already ends with `\n` (the closing
            // fence uses `writeln!`), so adding another `\n` after would
            // produce a surplus blank line that becomes an artifact when
            // the comment is later deleted.
            let new_markdown = format!("{before}{sep}{serialized}{after}");
            let reparsed = parser::parse(&new_markdown)
                .context("re-parsing after AfterLine insertion failed")?;
            doc.segments = reparsed.segments;
        }
    }

    Ok(())
}

/// Helper to serialize a `Segment::Comment` using the writer's serializer.
fn serialize_comment_from_segment(segment: &Segment) -> String {
    match segment {
        Segment::Comment(cm) => serialize_comment(cm),
        Segment::Body(_) | Segment::LegacyComment(_) => String::new(),
    }
}

/// Verify that a modification preserved comment integrity.
///
/// The expected state is: `after = (before - expected_removed) + expected_added`.
/// Any deviation is an error.
///
/// # Errors
///
/// Returns an error if:
/// - Comments were unexpectedly added (not in `expected_added`)
/// - Comments were unexpectedly removed (not in `expected_removed`)
pub fn verify_preservation(
    before_ids: &HashSet<String>,
    after_ids: &HashSet<String>,
    expected_added: &HashSet<String>,
    expected_removed: &HashSet<String>,
) -> Result<()> {
    // Compute what we expect after_ids to be.
    let mut expected: HashSet<&str> = before_ids.iter().map(String::as_str).collect();
    for removed in expected_removed {
        expected.remove(removed.as_str());
    }
    for added in expected_added {
        expected.insert(added.as_str());
    }

    let actual: HashSet<&str> = after_ids.iter().map(String::as_str).collect();

    // Check for unexpected additions.
    for id in &actual {
        if !expected.contains(id) {
            bail!("unexpected comment appeared: {id:?}");
        }
    }

    // Check for unexpected removals.
    for id in &expected {
        if !actual.contains(id) {
            bail!("comment unexpectedly disappeared: {id:?}");
        }
    }

    Ok(())
}

/// Write a `ParsedDocument` to disk, enforcing the preservation invariant.
///
/// 1. Snapshot IDs before serialization
/// 2. Serialize to markdown
/// 3. Re-parse to snapshot IDs after
/// 4. Verify the preservation invariant
/// 5. Write to disk via os-shim
///
/// # Errors
///
/// Returns an error if:
/// - The serialized document cannot be re-parsed
/// - The preservation invariant is violated
/// - Writing to disk fails
pub fn write_document(
    system: &dyn System,
    path: &Path,
    doc: &ParsedDocument,
    expected_added: &HashSet<String>,
    expected_removed: &HashSet<String>,
) -> Result<()> {
    ensure_not_forbidden_target(path)?;

    let before_ids: HashSet<String> = doc.comment_ids().into_iter().map(String::from).collect();

    let markdown = doc.to_markdown();

    // Re-parse to verify integrity.
    let reparsed = parser::parse(&markdown).context("re-parsing serialized document failed")?;
    let after_ids: HashSet<String> = reparsed
        .comment_ids()
        .into_iter()
        .map(String::from)
        .collect();

    verify_preservation(&before_ids, &after_ids, expected_added, expected_removed)
        .context("comment preservation invariant violated")?;

    system
        .write(path, markdown.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;

    Ok(())
}
