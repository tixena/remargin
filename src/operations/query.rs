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
use crate::parser;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Filter for cross-document queries.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct QueryFilter {
    /// Only include documents with comments by this author.
    pub author: Option<String>,
    /// Only include documents with pending (unacked) comments.
    pub pending: bool,
    /// Only include documents with pending comments for this recipient.
    pub pending_for: Option<String>,
    /// Only include documents with activity after this timestamp.
    pub since: Option<DateTime<FixedOffset>>,
}

/// A single result from a cross-document query.
#[derive(Debug)]
#[non_exhaustive]
pub struct QueryResult {
    /// Total number of comments in the document.
    pub comment_count: u32,
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

        let relative = entry
            .path
            .strip_prefix(base_dir)
            .unwrap_or(&entry.path)
            .to_path_buf();

        results.push(QueryResult {
            comment_count,
            last_activity,
            path: relative,
            pending_count,
            pending_for,
        });
    }

    Ok(results)
}
