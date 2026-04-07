//! Cross-document query engine.
//!
//! Search across documents in a directory tree to find pending reviews,
//! documents needing attention, comments by a specific author, etc.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset};
use os_shim::System;

use crate::document::allowlist;
use crate::parser::{self, Acknowledgment, AuthorType, Reactions};

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Filter for cross-document queries.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct QueryFilter {
    /// Only include documents with comments by this author.
    pub author: Option<String>,
    /// Only include documents containing a comment with this structural ID.
    pub comment_id: Option<String>,
    /// Include individual matching comments in each result.
    pub expanded: bool,
    /// Only include documents with pending (unacked) comments.
    pub pending: bool,
    /// Only include documents with pending comments for this recipient.
    pub pending_for: Option<String>,
    /// Only include documents with activity after this timestamp.
    pub since: Option<DateTime<FixedOffset>>,
}

/// Owned comment data for inclusion in expanded query results.
///
/// Cloned from parsed [`parser::Comment`] because the parsed document is
/// dropped after processing each file.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExpandedComment {
    /// Acknowledgments from other participants.
    pub ack: Vec<Acknowledgment>,
    /// Attached file references.
    pub attachments: Vec<String>,
    /// Author name or identifier.
    pub author: String,
    /// Whether the author is human or agent.
    pub author_type: AuthorType,
    /// Content integrity checksum.
    pub checksum: String,
    /// Comment body text.
    pub content: String,
    /// Unique short identifier.
    pub id: String,
    /// 1-indexed line number in the source document.
    pub line: usize,
    /// Emoji reactions mapped to lists of author IDs.
    pub reactions: Reactions,
    /// ID of the comment this is replying to.
    pub reply_to: Option<String>,
    /// Cryptographic signature.
    pub signature: Option<String>,
    /// Thread identifier grouping related comments.
    pub thread: Option<String>,
    /// Addressees of the comment.
    pub to: Vec<String>,
    /// Timestamp when the comment was created.
    pub ts: DateTime<FixedOffset>,
}

/// A single result from a cross-document query.
#[derive(Debug)]
#[non_exhaustive]
pub struct QueryResult {
    /// Total number of comments in the document.
    pub comment_count: u32,
    /// Individual matching comments (populated only when `expanded` is set).
    pub comments: Vec<ExpandedComment>,
    /// Most recent activity timestamp.
    pub last_activity: Option<DateTime<FixedOffset>>,
    /// Relative path to the document.
    pub path: PathBuf,
    /// Number of pending (unacked) comments.
    pub pending_count: u32,
    /// Unique recipients on unacked comments.
    pub pending_for: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Query across documents in a directory tree.
///
/// Walks the directory tree, parses markdown files, and filters based on
/// the provided query criteria.
///
/// # Errors
///
/// Returns an error if:
/// - The directory cannot be walked
/// - A file cannot be parsed
pub fn query(
    system: &dyn System,
    base_dir: &Path,
    filter: &QueryFilter,
) -> Result<Vec<QueryResult>> {
    let entries = system
        .walk_dir(base_dir, false, false)
        .with_context(|| format!("walking directory {}", base_dir.display()))?;

    let mut results = Vec::new();

    for entry in &entries {
        if !entry.is_file {
            continue;
        }

        // Only process visible markdown files.
        let has_md_ext = entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !has_md_ext || !allowlist::is_visible(&entry.path, false) {
            continue;
        }

        let Ok(content) = system.read_to_string(&entry.path) else {
            continue;
        };

        let Ok(doc) = parser::parse(&content) else {
            continue;
        };

        let comments = doc.comments();
        if comments.is_empty() {
            continue;
        }

        // Filter by comment ID if specified.
        if let Some(target_id) = &filter.comment_id
            && !comments.iter().any(|cm| cm.id == *target_id)
        {
            continue;
        }

        let comment_count = u32::try_from(comments.len()).unwrap_or(u32::MAX);
        let pending: Vec<&&parser::Comment> =
            comments.iter().filter(|cm| cm.ack.is_empty()).collect();
        let pending_count = u32::try_from(pending.len()).unwrap_or(u32::MAX);

        let mut pending_for: Vec<String> = Vec::new();
        for cm in &pending {
            for recipient in &cm.to {
                if !pending_for.contains(recipient) {
                    pending_for.push(recipient.clone());
                }
            }
        }
        pending_for.sort();

        let last_activity = comments.iter().map(|cm| cm.ts).max();

        // Apply filters.
        if filter.pending && pending_count == 0 {
            continue;
        }

        if let Some(target) = &filter.pending_for
            && !pending_for.contains(target)
        {
            continue;
        }

        if let Some(target_author) = &filter.author {
            let has_author = comments.iter().any(|cm| cm.author == *target_author);
            if !has_author {
                continue;
            }
        }

        if let Some(since) = &filter.since {
            let has_recent = last_activity.is_some_and(|ts| ts >= *since);
            if !has_recent {
                continue;
            }
        }

        // Collect expanded comments when requested.
        let expanded_comments = if filter.expanded {
            let matched: Vec<ExpandedComment> = comments
                .iter()
                .filter(|cm| comment_matches_filters(cm, filter))
                .map(|cm| expanded_from_comment(cm))
                .collect();
            // If no individual comments match, skip this file entirely.
            if matched.is_empty() {
                continue;
            }
            matched
        } else {
            Vec::new()
        };

        let relative = entry
            .path
            .strip_prefix(base_dir)
            .unwrap_or(&entry.path)
            .to_path_buf();

        results.push(QueryResult {
            comment_count,
            comments: expanded_comments,
            last_activity,
            path: relative,
            pending_count,
            pending_for,
        });
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Test whether a single comment matches all active filters.
fn comment_matches_filters(cm: &parser::Comment, filter: &QueryFilter) -> bool {
    if filter.pending && !cm.ack.is_empty() {
        return false;
    }
    if let Some(target) = &filter.pending_for
        && (!cm.ack.is_empty() || !cm.to.contains(target))
    {
        return false;
    }
    if let Some(target_author) = &filter.author
        && cm.author != *target_author
    {
        return false;
    }
    if let Some(since) = &filter.since
        && cm.ts < *since
    {
        return false;
    }
    if let Some(target_id) = &filter.comment_id
        && cm.id != *target_id
    {
        return false;
    }
    true
}

/// Convert a parsed comment reference into an owned `ExpandedComment`.
fn expanded_from_comment(cm: &parser::Comment) -> ExpandedComment {
    ExpandedComment {
        ack: cm.ack.clone(),
        attachments: cm.attachments.clone(),
        author: cm.author.clone(),
        author_type: cm.author_type.clone(),
        checksum: cm.checksum.clone(),
        content: cm.content.clone(),
        id: cm.id.clone(),
        line: cm.line,
        reactions: cm.reactions.clone(),
        reply_to: cm.reply_to.clone(),
        signature: cm.signature.clone(),
        thread: cm.thread.clone(),
        to: cm.to.clone(),
        ts: cm.ts,
    }
}

// ---------------------------------------------------------------------------
// Shared helper: resolve a comment ID across a folder tree
// ---------------------------------------------------------------------------

/// Walk a directory tree and return all document paths that contain a comment
/// with the given structural ID.
///
/// # Errors
///
/// Returns an error if the directory cannot be walked.
pub fn resolve_comment_id(
    system: &dyn System,
    base_dir: &Path,
    comment_id: &str,
) -> Result<Vec<PathBuf>> {
    let entries = system
        .walk_dir(base_dir, false, false)
        .with_context(|| format!("walking directory {}", base_dir.display()))?;

    let mut matches = Vec::new();

    for entry in &entries {
        if !entry.is_file {
            continue;
        }

        let has_md_ext = entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !has_md_ext || !allowlist::is_visible(&entry.path, false) {
            continue;
        }

        let Ok(content) = system.read_to_string(&entry.path) else {
            continue;
        };

        let Ok(doc) = parser::parse(&content) else {
            continue;
        };

        if doc.find_comment(comment_id).is_some() {
            matches.push(entry.path.clone());
        }
    }

    Ok(matches)
}
