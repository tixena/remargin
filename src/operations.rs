//! Comment operations: create, ack, react, delete, edit.
//!
//! The five core write operations that agents and users perform on remargin
//! documents. Each operation enforces mode rules, computes checksums,
//! optionally signs, and maintains the comment preservation invariant.

pub mod batch;
pub mod migrate;
pub mod purge;
pub mod query;
pub mod threading;

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
use crate::crypto::{compute_checksum, compute_reaction_checksum, compute_signature};
use crate::frontmatter;
use crate::id;
use crate::linter;
use crate::parser::{self, Acknowledgment, AuthorType, Comment, ParsedDocument, Segment};
use crate::writer::{self, InsertPosition};

// ---------------------------------------------------------------------------
// Create comment input
// ---------------------------------------------------------------------------

/// Parameters for creating a new comment.
#[non_exhaustive]
pub struct CreateCommentParams<'params> {
    /// File attachments to include.
    pub attachments: &'params [PathBuf],
    /// Comment body text.
    pub content: &'params str,
    /// Where to insert the comment in the document.
    pub position: &'params InsertPosition,
    /// ID of the comment this replies to.
    pub reply_to: Option<&'params str>,
    /// Addressees of the comment.
    pub to: &'params [String],
}

// ---------------------------------------------------------------------------
// Create comment
// ---------------------------------------------------------------------------

/// Create a new comment in a document.
///
/// Returns the generated comment ID.
///
/// # Errors
///
/// Returns an error if:
/// - The author is not allowed to post (mode enforcement)
/// - Attachment files do not exist
/// - The file cannot be read or written
/// - The linter detects structural issues
pub fn create_comment(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    params: &CreateCommentParams<'_>,
) -> Result<String> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to create a comment")?;

    config.can_post(identity)?;

    let author_type = config.author_type.clone().unwrap_or(AuthorType::Human);

    let mut doc = parser::parse_file(system, path)?;

    // Lint before.
    let markdown_before = doc.to_markdown();
    linter::lint_or_fail(&markdown_before)
        .context("document has structural issues before write")?;

    // Generate unique ID.
    let existing_ids = doc.comment_ids();
    let new_id = id::generate(&existing_ids);

    // Compute checksum.
    let checksum = compute_checksum(params.content);

    // Determine thread field from reply_to.
    let thread = params
        .reply_to
        .map(|parent_id| resolve_thread(&doc, parent_id));

    // Copy attachments to assets directory.
    let resolved_attachments = copy_attachments(system, path, config, params.attachments)
        .context("copying attachments")?;

    // Build the comment.
    let now = Utc::now().fixed_offset();
    let mut comment = Comment {
        ack: Vec::new(),
        attachments: resolved_attachments,
        author: String::from(identity),
        author_type,
        checksum,
        content: String::from(params.content),
        fence_depth: 3, // Will be recalculated by the writer serializer.
        id: new_id.clone(),
        reactions: BTreeMap::default(),
        reply_to: params.reply_to.map(String::from),
        signature: None,
        thread,
        to: params.to.to_vec(),
        ts: now,
    };

    // Sign if required.
    if config.requires_signature(identity) {
        if let Some(key_path) = &config.key_path {
            let sig = compute_signature(&comment, key_path, system)?;
            comment.signature = Some(sig);
        } else {
            bail!("strict mode requires a signing key but none is configured");
        }
    }

    // Insert comment.
    writer::insert_comment(&mut doc, comment, params.position)?;

    // Update frontmatter.
    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Write with preservation check.
    let expected_added: HashSet<String> = HashSet::from([new_id.clone()]);
    let expected_removed: HashSet<String> = HashSet::new();

    // Lint after.
    let markdown_after = doc.to_markdown();
    linter::lint_or_fail(&markdown_after).context("document has structural issues after write")?;

    writer::write_document(system, path, &doc, &expected_added, &expected_removed)?;

    Ok(new_id)
}

// ---------------------------------------------------------------------------
// Ack comments
// ---------------------------------------------------------------------------

/// Acknowledge one or more comments.
///
/// # Errors
///
/// Returns an error if:
/// - The author is not allowed to post
/// - A comment ID does not exist
/// - Writing fails
pub fn ack_comments(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_ids: &[&str],
) -> Result<()> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to ack")?;

    config.can_post(identity)?;

    let mut doc = parser::parse_file(system, path)?;
    let now = Utc::now().fixed_offset();

    for comment_id in comment_ids {
        let found = find_comment_mut(&mut doc, comment_id);
        let Some(cm) = found else {
            bail!("comment {comment_id:?} not found");
        };
        cm.ack.push(Acknowledgment {
            author: String::from(identity),
            ts: now,
        });
    }

    // Update frontmatter.
    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Write with preservation check (no ID changes).
    let empty: HashSet<String> = HashSet::new();
    writer::write_document(system, path, &doc, &empty, &empty)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// React
// ---------------------------------------------------------------------------

/// Add or remove an emoji reaction.
///
/// # Errors
///
/// Returns an error if:
/// - The author is not allowed to post
/// - The comment ID does not exist
/// - Writing fails
pub fn react(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_id: &str,
    emoji: &str,
    remove: bool,
) -> Result<()> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to react")?;

    config.can_post(identity)?;

    let mut doc = parser::parse_file(system, path)?;

    let cm = find_comment_mut(&mut doc, comment_id)
        .with_context(|| format!("comment {comment_id:?} not found"))?;

    if remove {
        if let Some(authors) = cm.reactions.get_mut(emoji) {
            authors.retain(|author| author != identity);
            if authors.is_empty() {
                cm.reactions.remove(emoji);
            }
        }
    } else {
        let authors = cm
            .reactions
            .entry(String::from(emoji))
            .or_insert_with(Vec::new);
        if !authors.contains(&String::from(identity)) {
            authors.push(String::from(identity));
        }
    }

    // Recompute reaction checksum (content checksum stays the same).
    let _reaction_checksum = compute_reaction_checksum(&cm.reactions);

    // Update frontmatter.
    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Write with preservation check (no ID changes).
    let empty: HashSet<String> = HashSet::new();
    writer::write_document(system, path, &doc, &empty, &empty)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Delete comments
// ---------------------------------------------------------------------------

/// Delete one or more comments.
///
/// # Errors
///
/// Returns an error if:
/// - A comment ID does not exist
/// - Writing fails
pub fn delete_comments(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_ids: &[&str],
) -> Result<()> {
    let mut doc = parser::parse_file(system, path)?;

    // Collect attachment paths from comments to be deleted.
    let deleted_attachments: Vec<String> = comment_ids
        .iter()
        .filter_map(|cid| doc.find_comment(cid))
        .flat_map(|cm| cm.attachments.clone())
        .collect();

    // Verify all IDs exist.
    for comment_id in comment_ids {
        if doc.find_comment(comment_id).is_none() {
            bail!("comment {comment_id:?} not found for deletion");
        }
    }

    // Remove the comment segments.
    let id_set: HashSet<&str> = comment_ids.iter().copied().collect();
    doc.segments
        .retain(|seg| !matches!(seg, Segment::Comment(cm) if id_set.contains(cm.id.as_str())));

    // Clean up orphaned attachments.
    let remaining_attachments: HashSet<String> = doc
        .comments()
        .iter()
        .flat_map(|cm| cm.attachments.clone())
        .collect();

    for attachment in &deleted_attachments {
        if !remaining_attachments.contains(attachment) {
            let attachment_path = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(attachment);
            // Best-effort deletion; ignore errors if the file is already gone.
            let _: Result<(), _> = system.remove_file(&attachment_path);
        }
    }

    // Update frontmatter.
    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Write with preservation check.
    let expected_added: HashSet<String> = HashSet::new();
    let expected_removed: HashSet<String> = comment_ids.iter().map(|s| String::from(*s)).collect();
    writer::write_document(system, path, &doc, &expected_added, &expected_removed)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Edit comment
// ---------------------------------------------------------------------------

/// Edit a comment's content. Cascading consequences:
/// - Recomputes checksum and signature
/// - Clears all ack entries on the edited comment
/// - Clears all ack entries on all child comments (entire reply chain)
///
/// # Errors
///
/// Returns an error if:
/// - The comment ID does not exist
/// - Writing fails
pub fn edit_comment(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_id: &str,
    new_content: &str,
) -> Result<()> {
    let identity = config.identity.as_deref();

    let mut doc = parser::parse_file(system, path)?;

    // Find the comment to edit.
    let cm = find_comment_mut(&mut doc, comment_id)
        .with_context(|| format!("comment {comment_id:?} not found"))?;

    // Update content and recompute checksum.
    cm.content = String::from(new_content);
    cm.checksum = compute_checksum(new_content);

    // Clear ack on the edited comment.
    cm.ack.clear();

    // Recompute signature if needed.
    if let Some(author) = identity
        && config.requires_signature(author)
        && let Some(key_path) = &config.key_path
    {
        let sig = compute_signature(cm, key_path, system)?;
        cm.signature = Some(sig);
    }

    // Cascade ack invalidation through reply chain.
    let descendants = collect_descendants(&doc, comment_id);
    for descendant_id in &descendants {
        if let Some(child) = find_comment_mut(&mut doc, descendant_id) {
            child.ack.clear();
        }
    }

    // Update frontmatter.
    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Write with preservation check (no ID changes).
    let empty: HashSet<String> = HashSet::new();
    writer::write_document(system, path, &doc, &empty, &empty)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect all descendant comment IDs in the reply chain (depth-first).
fn collect_descendants(doc: &ParsedDocument, root_id: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut stack = vec![String::from(root_id)];

    while let Some(parent_id) = stack.pop() {
        for cm in doc.comments() {
            if cm.reply_to.as_deref() == Some(parent_id.as_str()) && !result.contains(&cm.id) {
                result.push(cm.id.clone());
                stack.push(cm.id.clone());
            }
        }
    }

    result
}

/// Copy attachment files to the assets directory.
fn copy_attachments(
    system: &dyn System,
    doc_path: &Path,
    config: &ResolvedConfig,
    attachments: &[PathBuf],
) -> Result<Vec<String>> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    let doc_dir = doc_path.parent().unwrap_or_else(|| Path::new("."));
    let assets_dir = doc_dir.join(&config.assets_dir);

    system
        .create_dir_all(&assets_dir)
        .context("creating assets directory")?;

    let mut result = Vec::new();
    for src_path in attachments {
        // Validate the source file exists.
        if !system.exists(src_path).unwrap_or(false) {
            bail!("attachment not found: {}", src_path.display());
        }

        let filename = src_path
            .file_name()
            .context("attachment has no filename")?
            .to_str()
            .context("attachment filename is not valid UTF-8")?;

        let dest_path = assets_dir.join(filename);
        system
            .copy(src_path, &dest_path)
            .with_context(|| format!("copying attachment {}", src_path.display()))?;

        // Store relative path from document directory.
        let relative = format!("{}/{filename}", config.assets_dir);
        result.push(relative);
    }

    Ok(result)
}

/// Find a mutable reference to a comment by ID.
fn find_comment_mut<'doc>(doc: &'doc mut ParsedDocument, id: &str) -> Option<&'doc mut Comment> {
    doc.segments.iter_mut().find_map(|seg| match seg {
        Segment::Comment(cm) if cm.id == id => Some(cm.as_mut()),
        Segment::Body(_) | Segment::Comment(_) | Segment::LegacyComment(_) => None,
    })
}

/// Resolve the thread field for a reply comment.
///
/// If the parent has a thread, inherit it. Otherwise, use the parent's ID
/// as the thread root.
fn resolve_thread(doc: &ParsedDocument, parent_id: &str) -> String {
    doc.find_comment(parent_id)
        .and_then(|parent| parent.thread.clone())
        .unwrap_or_else(|| String::from(parent_id))
}
