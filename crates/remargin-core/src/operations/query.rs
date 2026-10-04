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
use serde_json::{Value, json};
use tixschema::model_schema;

use crate::config::ResolvedConfig;
use crate::document::allowlist;
use crate::kind::matches_kind_filter;
use crate::parser::{self, Acknowledgment, AuthorType};
use crate::parser::{acknowledgment_schema, author_type_schema};
use crate::reactions::{ReactionEntry, reaction_entry_schema};

/// Compact comment-row column names, `content` last.
///
/// Emitted once per response in the envelope's `comment_cols` header;
/// [`to_compact_row`] fills the positions in this order. The verbose
/// per-comment `checksum` / `signature` and the redundant `file` are
/// dropped.
pub const COMMENT_COLS: [&str; 14] = [
    "id",
    "line",
    "author",
    "author_type",
    "ts",
    "reply_to",
    "thread",
    "to",
    "ack",
    "reactions",
    "kind",
    "edited_at",
    "attachments",
    "content",
];

/// [`COMMENT_COLS`] widened with `checksum`, `signature` inserted
/// immediately before `content`. Selected when `include_integrity` is set.
pub const COMMENT_COLS_INTEGRITY: [&str; 16] = [
    "id",
    "line",
    "author",
    "author_type",
    "ts",
    "reply_to",
    "thread",
    "to",
    "ack",
    "reactions",
    "kind",
    "edited_at",
    "attachments",
    "checksum",
    "signature",
    "content",
];

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
/// not been acked by anyone. the broad form silently
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
    /// both directed and broadcast comments.
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
    /// design.
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

    /// Identity-scoped pending-flavor label for pretty-print headers,
    /// phrased after "N pending": `for <name>` when only a directed
    /// flavor (`--pending-for` / `--pending-for-me`) is active,
    /// `broadcast, unacked by <name>` when only `--pending-broadcast`
    /// is, and `for <name> or broadcast` for the union — so a match
    /// that came from the broadcast flavor is never labeled as if it
    /// were addressed to the caller.
    #[must_use]
    pub fn pending_label(&self) -> Option<String> {
        let directed = self
            .pending_for
            .as_deref()
            .or(self.pending_for_me.as_deref());
        match (directed, self.pending_broadcast.as_deref()) {
            (Some(name), Some(_)) => Some(format!("for {name} or broadcast")),
            (Some(name), None) => Some(format!("for {name}")),
            (None, Some(me)) => Some(format!("broadcast, unacked by {me}")),
            (None, None) => None,
        }
    }

    /// Attach the caller's identity to the identity-scoped pending
    /// flavors (`pending_for_me`, `pending_broadcast`) when those flags
    /// were requested. Returns an error when a flag is set but no
    /// identity was provided.
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
    /// Edit timestamp set by [`crate::operations::edit_comment`].
    /// `None` for comments that have never been edited. Pretty-print
    /// and the activity command both surface this when present.
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
    /// Comment classification tags. Absent when the
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
    /// `None` in summary mode; otherwise the (always non-empty)
    /// list of matching comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<ExpandedComment>>,
    /// Most recent activity timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<DateTime<FixedOffset>>,
    /// Number of comments matching the active filters; equals
    /// `comment_count` when no comment-level filter is set.
    pub matched_count: u32,
    /// Relative path to the document.
    pub path: PathBuf,
    /// Number of pending (unacked) comments.
    pub pending_count: u32,
    /// `None` when no recipients have outstanding acks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_for: Option<Vec<String>>,
}

/// One compact comment row: positional columns named by [`COMMENT_COLS`].
///
/// Only the base (14-column) arity is codegen'd; the integrity form adds
/// `checksum`, `signature` before `content` at runtime, and the
/// self-describing `comment_cols` header covers the widened shape. Acks
/// compact to `author@ts` strings; `author_type` keeps its verbose
/// serialization; `reactions` stays a map. Nullable columns (`reply_to`,
/// `thread`, `remargin_kind`, `edited_at`) serialize as `null`.
#[model_schema(name = "CompactCommentRow")]
pub type CompactCommentRow = (
    String,
    usize,
    String,
    AuthorType,
    DateTime<FixedOffset>,
    Option<String>,
    Option<String>,
    Vec<String>,
    Vec<String>,
    BTreeMap<String, Vec<ReactionEntry>>,
    Option<Vec<String>>,
    Option<DateTime<FixedOffset>>,
    Vec<String>,
    String,
);

/// Schema anchor for the compact per-file query result.
///
/// Mirrors [`QueryResult`] but its `comments` are positional
/// [`CompactCommentRow`]s. Exists so xtask emits the TS / Zod types the
/// LLM consumer reads; the runtime builds the shape in [`to_compact_result`]
/// and the enclosing envelope (`base_path`, `comment_cols`, `results`) is
/// assembled by each surface.
// A 14-element `CompactCommentRow` exceeds std's `Debug` impls (max arity
// 12); this schema anchor is never printed, so `Debug` is omitted.
#[model_schema]
#[derive(Serialize)]
#[non_exhaustive]
pub struct CompactQueryResult {
    pub comment_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<CompactCommentRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<DateTime<FixedOffset>>,
    /// Number of comments matching the active filters; equals
    /// `comment_count` when no comment-level filter is set.
    pub matched_count: u32,
    pub path: PathBuf,
    pub pending_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_for: Option<Vec<String>>,
}

/// Column header naming the [`to_compact_row`] positions, widened when
/// `include_integrity` is set.
#[must_use]
pub const fn comment_cols(include_integrity: bool) -> &'static [&'static str] {
    if include_integrity {
        &COMMENT_COLS_INTEGRITY
    } else {
        &COMMENT_COLS
    }
}

/// Render the envelope `base_path` for a query argument: the argument
/// itself for a directory, its parent for a file. Always ends in `/`; an
/// empty parent renders as `./`.
///
/// A file argument yields a filename-only result path (see [`query`]), so
/// only the parent makes `base_path` ⊕ `path` reconstruct the real path.
#[must_use]
pub fn display_base_path(raw: &str, is_file: bool) -> String {
    let trimmed = raw.trim_end_matches('/');
    let base = if is_file {
        Path::new(trimmed)
            .parent()
            .map_or("", |parent| parent.to_str().unwrap_or(""))
    } else {
        trimmed
    };
    if base.is_empty() {
        String::from("./")
    } else {
        format!("{base}/")
    }
}

/// Project one verbose [`ExpandedComment`] onto its compact positional row.
///
/// `checksum` / `signature` are added before `content` only when
/// `include_integrity` is set; the redundant `file` is always dropped.
#[must_use]
pub fn to_compact_row(comment: &ExpandedComment, include_integrity: bool) -> Value {
    let ack: Vec<String> = comment
        .ack
        .iter()
        .map(|a| format!("{}@{}", a.author, parser::rfc3339_z(&a.ts)))
        .collect();
    let mut row = vec![
        json!(comment.id),
        json!(comment.line),
        json!(comment.author),
        json!(comment.author_type),
        json!(comment.ts),
        json!(comment.reply_to),
        json!(comment.thread),
        json!(comment.to),
        json!(ack),
        json!(comment.reactions),
        json!(comment.remargin_kind),
        json!(comment.edited_at),
        json!(comment.attachments),
    ];
    if include_integrity {
        row.push(json!(comment.checksum));
        row.push(json!(comment.signature));
    }
    row.push(json!(comment.content));
    Value::Array(row)
}

/// Project one verbose [`QueryResult`] onto the compact per-file shape:
/// summary fields stay named, comments become positional rows. Comments
/// are omitted in summary mode (mirrors the verbose skip).
#[must_use]
pub fn to_compact_result(result: &QueryResult, include_integrity: bool) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert(String::from("path"), json!(result.path));
    obj.insert(String::from("comment_count"), json!(result.comment_count));
    obj.insert(String::from("matched_count"), json!(result.matched_count));
    obj.insert(String::from("pending_count"), json!(result.pending_count));
    if let Some(pending_for) = &result.pending_for {
        obj.insert(String::from("pending_for"), json!(pending_for));
    }
    if let Some(last_activity) = &result.last_activity {
        obj.insert(String::from("last_activity"), json!(last_activity));
    }
    if let Some(comments) = &result.comments {
        let rows: Vec<Value> = comments
            .iter()
            .map(|comment| to_compact_row(comment, include_integrity))
            .collect();
        obj.insert(String::from("comments"), Value::Array(rows));
    }
    Value::Object(obj)
}

/// The comments of one parsed document that pass `filter`, in document
/// order, as compact rows named by [`comment_cols`].
#[must_use]
pub fn compact_rows_for(
    doc: &parser::ParsedDocument,
    file: &Path,
    filter: &QueryFilter,
    include_integrity: bool,
) -> Vec<Value> {
    doc.comments()
        .into_iter()
        .filter(|cm| comment_matches_filters(cm, filter))
        .map(|cm| to_compact_row(&expanded_from_comment(cm, file), include_integrity))
        .collect()
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
    config: &ResolvedConfig,
) -> Result<Vec<QueryResult>> {
    config.ensure_can_read(system, base_dir)?;

    // File-path branch: the user named one file explicitly, so honor it
    // and skip the `.md`-extension and visibility gates the walk applies.
    // The relative path is the file name, matching how a directory query
    // of the parent would render this file.
    if system.is_file(base_dir).unwrap_or(false) {
        let relative = base_dir
            .file_name()
            .map_or_else(|| base_dir.to_path_buf(), PathBuf::from);
        return Ok(process_document(system, base_dir, &relative, filter)
            .into_iter()
            .collect());
    }

    let entries = system
        .walk_dir(base_dir, false, false)
        .with_context(|| format!("walking directory {}", base_dir.display()))?;

    let mut gate = config.read_gate();
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

        if !gate.admits(system, &entry.path)? {
            continue;
        }

        let relative = entry
            .path
            .strip_prefix(base_dir)
            .unwrap_or(&entry.path)
            .to_path_buf();

        if let Some(result) = process_document(system, &entry.path, &relative, filter) {
            results.push(result);
        }
    }

    Ok(results)
}

/// Read, parse, and filter a single document, producing a [`QueryResult`]
/// when it survives every active filter.
///
/// Shared by both `query` call paths (the directory walk and the
/// explicit file-path branch) so filter semantics stay identical.
/// Returns `None` when the file cannot be read or parsed, has no
/// comments, or is excluded by a filter — these are not errors.
fn process_document(
    system: &dyn System,
    file_path: &Path,
    relative: &Path,
    filter: &QueryFilter,
) -> Option<QueryResult> {
    let content = system.read_to_string(file_path).ok()?;

    let doc = parser::parse(&content).ok()?;

    let comments = doc.comments();
    if comments.is_empty() {
        return None;
    }

    // Filter by comment ID if specified.
    if let Some(target_id) = &filter.comment_id
        && !comments.iter().any(|cm| cm.id == *target_id)
    {
        return None;
    }

    let comment_count = u32::try_from(comments.len()).unwrap_or(u32::MAX);
    let pending: Vec<&&parser::Comment> = comments.iter().filter(|cm| is_pending(cm)).collect();
    let pending_count = u32::try_from(pending.len()).unwrap_or(u32::MAX);
    let pending_for = collect_pending_recipients(&pending);

    let last_activity = comments.iter().map(|cm| cm.ts).max();

    // Apply the pending-flavor union filter at the file level: when
    // any of `pending`, `pending_for`, `pending_for_me`, or
    // `pending_broadcast` is set, the document must have at least
    // one comment that matches the union.
    if filter.any_pending_active() && !comments.iter().any(|cm| filter.matches_pending_union(cm)) {
        return None;
    }

    if let Some(target_author) = &filter.author {
        let has_author = comments.iter().any(|cm| cm.author == *target_author);
        if !has_author {
            return None;
        }
    }

    if let Some(since) = &filter.since {
        let has_recent = last_activity.is_some_and(|ts| ts >= *since);
        if !has_recent {
            return None;
        }
    }

    // Both the count and the rows derive from this one pass, so they can
    // never disagree about what "matched" means.
    let matched: Vec<&&parser::Comment> = comments
        .iter()
        .filter(|cm| comment_matches_filters(cm, filter))
        .collect();
    let matched_count = u32::try_from(matched.len()).unwrap_or(u32::MAX);

    // Collect expanded comments unless summary-only mode is requested.
    // When `expanded` is true OR `summary` is false, include comment data.
    let include_comments = !filter.summary || filter.expanded;
    let expanded_comments = include_comments.then(|| {
        matched
            .iter()
            .map(|cm| expanded_from_comment(cm, relative))
            .collect::<Vec<ExpandedComment>>()
    });
    // Empty match list means no matches — skip the file entirely. Summary
    // mode keeps listing the file (it passed the file-level gates) and
    // reports `matched_count: 0`.
    if include_comments && matched.is_empty() {
        return None;
    }

    Some(QueryResult {
        comment_count,
        comments: expanded_comments,
        last_activity,
        matched_count,
        path: relative.to_path_buf(),
        pending_count,
        pending_for: (!pending_for.is_empty()).then_some(pending_for),
    })
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
/// enough to close a broadcast conversation. the
/// broad form silently excluded broadcasts; the current semantics
/// match the documented "pending (unacked) comments" language.
fn is_pending(cm: &parser::Comment) -> bool {
    cm.is_pending()
}

/// A comment is pending for a specific `target` if `target` is in `to` and
/// has not acknowledged it.
fn is_pending_for(cm: &parser::Comment, target: &str) -> bool {
    cm.is_pending_for(target)
}

/// A broadcast comment is pending for `me` when `to` is empty, `me`
/// did not write it, AND `me` has not acknowledged yet. The caller's
/// ack "closes" the broadcast from their personal perspective even
/// when other participants have not acked (unlike the broad
/// `is_pending`, which considers any ack enough to close the
/// conversation).
fn is_pending_broadcast(cm: &parser::Comment, me: &str) -> bool {
    cm.is_pending_broadcast_for(me)
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

/// Render `count` followed by `noun`, singular only at exactly one.
///
/// Naive `-s` suffixing: the text surfaces only count regular nouns.
pub(crate) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Render the comment-count phrase of a result header.
///
/// Reads `{matched} of {total} comments` when a comment-level filter
/// narrowed the set, `{total} comments` otherwise. Shared with the
/// pretty printer so both text surfaces say the same thing.
pub(crate) fn format_comment_count(matched: u32, total: u32) -> String {
    let counted = plural(total as usize, "comment");
    if matched == total {
        counted
    } else {
        format!("{matched} of {counted}")
    }
}

/// Render query results in plain (or summary) text format.
///
/// Each result produces one header line with the path, comment count,
/// and the file-wide pending count. When `expanded` comments are
/// present, each comment produces an indented detail line.
///
/// This covers the `Plain` and `Summary` output modes. The `Pretty`
/// mode delegates to [`crate::display::format_query_pretty`]; the `Json`
/// mode is handled entirely in the binary's dispatch layer.
#[must_use]
pub fn render_query_plain(results: &[QueryResult]) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    for r in results {
        let _ = writeln!(
            out,
            "{} ({}, {} pending)",
            r.path.display(),
            format_comment_count(r.matched_count, r.comment_count),
            r.pending_count,
        );
        for cm in r.comments.as_deref().unwrap_or(&[]) {
            let status = if cm.ack.is_empty() {
                "pending"
            } else {
                "acked"
            };
            let _ = writeln!(
                out,
                "  {} {} ({}) [{}] {}",
                cm.id,
                cm.author,
                cm.author_type.as_str(),
                status,
                cm.content,
            );
        }
    }
    out
}
