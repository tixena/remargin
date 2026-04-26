//! Cross-document query engine.
//!
//! Search across documents in a directory tree to find pending reviews,
//! documents needing attention, comments by a specific author, etc.

#[cfg(test)]
mod tests;

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset};
use os_shim::System;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use tixschema::model_schema;

use crate::document::allowlist;
use crate::kind::matches_kind_filter;
use crate::parser::{self, Acknowledgment, AuthorType};
use crate::parser::{acknowledgment_schema, author_type_schema};
use crate::reactions::ReactionEntry;

/// Filter for cross-document queries.
///
/// The four pending-flavor fields — `pending`, `pending_for`,
/// `pending_for_me`, and `pending_broadcast` — compose as a union
/// (OR): when any are set, a comment is surfaced if it satisfies at
/// least one. The union is AND-combined with the non-pending filters
/// (`author`, `comment_id`, `content_regex`, `since`).
///
/// `pending` (the broad form) includes BOTH directed comments with
/// unacked recipients AND broadcast comments (empty `to`) that have
/// not been acked by anyone. Before rem-4j91 the broad form silently
/// excluded broadcasts; the bug-fix semantics match the help text
/// ("Only documents with pending (unacked) comments") without the
/// implicit directed-only carve-out.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct QueryFilter {
    /// Only include documents with comments by this author.
    pub author: Option<String>,
    /// Only include documents containing a comment with this structural ID.
    pub comment_id: Option<String>,
    /// Regex applied to comment content. Applied after all metadata filters;
    /// see [`QueryFilter::with_content_regex`] for a pre-compiled constructor
    /// helper.
    pub content_regex: Option<Regex>,
    /// Include individual matching comments in each result.
    pub expanded: bool,
    /// Only include documents with pending (unacked) comments. Matches
    /// both directed and broadcast comments (rem-4j91).
    pub pending: bool,
    /// Surface broadcast (empty-`to`) comments that the given identity
    /// has not acknowledged yet. Set by the CLI's `--pending-broadcast`
    /// and the MCP `pending_broadcast: true` flag, which carry the
    /// caller's identity through.
    pub pending_broadcast: Option<String>,
    /// Only include documents with pending comments for this recipient.
    pub pending_for: Option<String>,
    /// Sugar for `pending_for = Some(<caller identity>)`. Kept as a
    /// distinct field so CLI/MCP surfaces can expose a "pending for me"
    /// flag without needing the caller to repeat their identity.
    pub pending_for_me: Option<String>,
    /// OR-semantics filter: include a comment when its `remargin_kind`
    /// list contains at least one of these values. Empty = no filter.
    /// Shares a matcher with the `comments` CLI command via
    /// [`crate::kind::matches_kind_filter`], so both surfaces stay on
    /// par — divergence between them was explicitly called out in the
    /// rem-49w0 design.
    pub remargin_kind: Vec<String>,
    /// Only include documents with activity after this timestamp.
    pub since: Option<DateTime<FixedOffset>>,
    /// Return only counts/summary, suppress comment data.
    pub summary: bool,
}

impl QueryFilter {
    /// Any pending-flavor filter is active. When true, comments must
    /// satisfy at least one of `pending`, `pending_for`,
    /// `pending_for_me`, or `pending_broadcast`.
    const fn any_pending_active(&self) -> bool {
        self.pending
            || self.pending_broadcast.is_some()
            || self.pending_for.is_some()
            || self.pending_for_me.is_some()
    }

    /// A comment satisfies the pending-flavor union when any of the
    /// active pending filters matches it.
    fn matches_pending_union(&self, cm: &parser::Comment) -> bool {
        if self.pending && is_pending(cm) {
            return true;
        }
        if let Some(target) = &self.pending_for
            && is_pending_for(cm, target)
        {
            return true;
        }
        if let Some(me) = &self.pending_for_me
            && is_pending_for(cm, me)
        {
            return true;
        }
        if let Some(me) = &self.pending_broadcast
            && is_pending_broadcast(cm, me)
        {
            return true;
        }
        false
    }

    /// Identity-scoped pending-flavor label preferred for pretty-print
    /// headers. Returns the explicit `--pending-for` name when set,
    /// falling back to the caller identity attached by
    /// `--pending-for-me` / `--pending-broadcast` (rem-4j91).
    #[must_use]
    pub fn pending_label(&self) -> Option<&str> {
        self.pending_for
            .as_deref()
            .or(self.pending_for_me.as_deref())
            .or(self.pending_broadcast.as_deref())
    }

    /// Attach the caller's identity to the identity-scoped pending
    /// flavors (`pending_for_me`, `pending_broadcast`) when those flags
    /// were requested. Returns an error when a flag is set but no
    /// identity was provided (rem-4j91).
    ///
    /// # Errors
    ///
    /// Returns an error if `want_for_me` or `want_broadcast` is true
    /// but `caller_identity` is `None`.
    pub fn with_caller_identity(
        mut self,
        want_for_me: bool,
        want_broadcast: bool,
        caller_identity: Option<String>,
    ) -> Result<Self> {
        if !want_for_me && !want_broadcast {
            return Ok(self);
        }
        let me = caller_identity
            .context("pending_for_me / pending_broadcast require a configured identity")?;
        if want_for_me {
            self.pending_for_me = Some(me.clone());
        }
        if want_broadcast {
            self.pending_broadcast = Some(me);
        }
        Ok(self)
    }

    /// Compile `pattern` with optional case-insensitivity and attach it as the
    /// content regex. Returns a structured error (with the caller-provided
    /// pattern) when compilation fails.
    ///
    /// # Errors
    ///
    /// Returns an error if `pattern` cannot be compiled as a regex.
    pub fn with_content_regex(mut self, pattern: &str, ignore_case: bool) -> Result<Self> {
        let compiled = RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .with_context(|| format!("invalid content regex: {pattern}"))?;
        self.content_regex = Some(compiled);
        Ok(self)
    }
}

/// Owned comment data for inclusion in expanded query results.
///
/// Cloned from parsed [`parser::Comment`] because the parsed document is
/// dropped after processing each file.
///
/// The serde [`Serialize`] implementation emits JSON that matches the
/// `ExpandedComment` schema generated by `tixschema`: `snake_case` field
/// names, `PascalCase` `author_type` variants, `file` as a string, and
/// `Option` fields skipped when `None`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
#[model_schema]
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
    /// Edit timestamp set by [`crate::operations::edit_comment`]
    /// (rem-g3sy.2 / T32). `None` for comments that have never been
    /// edited. Pretty-print and the activity command both surface
    /// this when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<DateTime<FixedOffset>>,
    /// Relative path to the file this comment belongs to.
    pub file: PathBuf,
    /// Unique short identifier.
    pub id: String,
    /// 1-indexed line number in the source document.
    pub line: usize,
    /// Emoji reactions, each carrying per-author timestamps.
    pub reactions: BTreeMap<String, Vec<ReactionEntry>>,
    /// Comment classification tags (rem-n4x7). Absent when the
    /// underlying comment had no `remargin_kind:` line so pre-field
    /// comments round-trip without a visible change on the JSON wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remargin_kind: Option<Vec<String>>,
    /// ID of the comment this is replying to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Cryptographic signature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Thread identifier grouping related comments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// Addressees of the comment.
    pub to: Vec<String>,
    /// Timestamp when the comment was created.
    pub ts: DateTime<FixedOffset>,
}

/// A single result from a cross-document query.
///
/// Serializes to JSON that matches the `QueryResult` tixschema.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct QueryResult {
    /// Total number of comments in the document.
    pub comment_count: u32,
    /// Individual matching comments (populated by default; empty in summary mode).
    pub comments: Vec<ExpandedComment>,
    /// Most recent activity timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<DateTime<FixedOffset>>,
    /// Relative path to the document.
    pub path: PathBuf,
    /// Number of pending (unacked) comments.
    pub pending_count: u32,
    /// Unique recipients on unacked comments.
    pub pending_for: Vec<String>,
}

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
        let pending: Vec<&&parser::Comment> = comments.iter().filter(|cm| is_pending(cm)).collect();
        let pending_count = u32::try_from(pending.len()).unwrap_or(u32::MAX);
        let pending_for = collect_pending_recipients(&pending);

        let last_activity = comments.iter().map(|cm| cm.ts).max();

        // Apply the pending-flavor union filter at the file level: when
        // any of `pending`, `pending_for`, `pending_for_me`, or
        // `pending_broadcast` is set, the document must have at least
        // one comment that matches the union (rem-4j91).
        if filter.any_pending_active()
            && !comments.iter().any(|cm| filter.matches_pending_union(cm))
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

        // Collect expanded comments unless summary-only mode is requested.
        // When `expanded` is true OR `summary` is false, include comment data.
        let include_comments = !filter.summary || filter.expanded;
        let expanded_comments = if include_comments {
            let matched: Vec<ExpandedComment> = comments
                .iter()
                .filter(|cm| comment_matches_filters(cm, filter))
                .map(|cm| expanded_from_comment(cm, &relative))
                .collect();
            // If no individual comments match, skip this file entirely.
            if matched.is_empty() {
                continue;
            }
            matched
        } else {
            Vec::new()
        };

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

/// Test whether a single comment matches all active filters.
///
/// The pending-flavor fields (`pending`, `pending_for`,
/// `pending_for_me`, `pending_broadcast`) compose as a union: when
/// any are set the comment must satisfy at least one of them. The
/// union is AND-combined with author/since/comment-id/content_regex.
/// The `content_regex` check runs last so the regex only executes
/// against the already-filtered subset.
fn comment_matches_filters(cm: &parser::Comment, filter: &QueryFilter) -> bool {
    if filter.any_pending_active() && !filter.matches_pending_union(cm) {
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
    if let Some(re) = &filter.content_regex
        && !re.is_match(&cm.content)
    {
        return false;
    }
    if !matches_kind_filter(cm.kinds(), &filter.remargin_kind) {
        return false;
    }
    true
}

/// Collect unique recipients who still have unacked comments, sorted.
fn collect_pending_recipients(pending: &[&&parser::Comment]) -> Vec<String> {
    let mut recipients: Vec<String> = Vec::new();
    for cm in pending {
        let ack_authors: Vec<&str> = cm.ack.iter().map(|a| a.author.as_str()).collect();
        for recipient in &cm.to {
            if !ack_authors.contains(&recipient.as_str()) && !recipients.contains(recipient) {
                recipients.push(recipient.clone());
            }
        }
    }
    recipients.sort();
    recipients
}

/// A comment is pending when the conversation is still open.
///
/// Directed comments (`to` non-empty) are pending when at least one
/// named recipient has not acknowledged. Broadcast comments (`to`
/// empty) are pending when nobody has acknowledged yet — any ack is
/// enough to close a broadcast conversation. Before rem-4j91 the
/// broad form silently excluded broadcasts; the current semantics
/// match the documented "pending (unacked) comments" language.
fn is_pending(cm: &parser::Comment) -> bool {
    if cm.to.is_empty() {
        return cm.ack.is_empty();
    }
    let ack_authors: Vec<&str> = cm.ack.iter().map(|a| a.author.as_str()).collect();
    cm.to
        .iter()
        .any(|recipient| !ack_authors.contains(&recipient.as_str()))
}

/// A comment is pending for a specific `target` if `target` is in `to` and
/// has not acknowledged it.
fn is_pending_for(cm: &parser::Comment, target: &str) -> bool {
    if !cm.to.contains(&String::from(target)) {
        return false;
    }
    !cm.ack.iter().any(|a| a.author == target)
}

/// A broadcast comment is pending for `me` when `to` is empty AND `me`
/// has not acknowledged yet. The caller's ack "closes" the broadcast
/// from their personal perspective even when other participants have
/// not acked (unlike the broad `is_pending`, which considers any ack
/// enough to close the conversation).
fn is_pending_broadcast(cm: &parser::Comment, me: &str) -> bool {
    if !cm.to.is_empty() {
        return false;
    }
    !cm.ack.iter().any(|a| a.author == me)
}

/// Convert a parsed comment reference into an owned `ExpandedComment`.
fn expanded_from_comment(cm: &parser::Comment, file: &Path) -> ExpandedComment {
    ExpandedComment {
        ack: cm.ack.clone(),
        attachments: cm.attachments.clone(),
        author: cm.author.clone(),
        author_type: cm.author_type.clone(),
        checksum: cm.checksum.clone(),
        content: cm.content.clone(),
        edited_at: cm.edited_at,
        file: file.to_path_buf(),
        id: cm.id.clone(),
        line: cm.line,
        reactions: cm.reactions.clone(),
        remargin_kind: cm.remargin_kind.clone(),
        reply_to: cm.reply_to.clone(),
        signature: cm.signature.clone(),
        thread: cm.thread.clone(),
        to: cm.to.clone(),
        ts: cm.ts,
    }
}

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
