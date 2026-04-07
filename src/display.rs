//! Pretty-print comment display with threaded nesting.
//!
//! This module formats remargin comments as a human-readable threaded tree.
//! Root comments are sorted by line number (document order), replies are
//! nested under their parents sorted by timestamp ascending.

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt::Write as _;

use crate::parser::{AuthorType, Comment};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum content lines before truncation.
const MAX_CONTENT_LINES: usize = 5;

// ---------------------------------------------------------------------------
// Tree data structure
// ---------------------------------------------------------------------------

/// A node in the comment tree, holding a reference to the comment and its
/// children (direct replies).
pub(crate) struct CommentNode<'cm> {
    /// Direct replies, sorted by timestamp ascending.
    pub children: Vec<Self>,
    /// The comment at this node.
    pub comment: &'cm Comment,
}

// ---------------------------------------------------------------------------
// Tree building
// ---------------------------------------------------------------------------

/// Build a node and its descendants recursively.
fn build_node<'cm>(
    idx: usize,
    comments: &[&'cm Comment],
    children_map: &BTreeMap<&str, Vec<usize>>,
) -> CommentNode<'cm> {
    let cm = comments[idx];
    let children = children_map
        .get(cm.id.as_str())
        .map_or_else(Vec::new, |child_indices| {
            child_indices
                .iter()
                .map(|&ci| build_node(ci, comments, children_map))
                .collect()
        });
    CommentNode {
        children,
        comment: cm,
    }
}

/// Build a forest (list of trees) from a flat comment list.
///
/// Roots: comments with no `reply_to`, or orphans whose `reply_to` points to a
/// non-existent ID.  Roots are sorted by line number ascending (document order).
///
/// Children: sorted by timestamp ascending (conversation order).
pub(crate) fn build_comment_tree<'cm>(comments: &[&'cm Comment]) -> Vec<CommentNode<'cm>> {
    // Build an ID set for quick lookup.
    let id_set: BTreeMap<&str, usize> = comments
        .iter()
        .enumerate()
        .map(|(i, cm)| (cm.id.as_str(), i))
        .collect();

    // Build a children map: parent_id -> list of child indices.
    let mut children_map: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut root_indices: Vec<usize> = Vec::new();

    for (i, cm) in comments.iter().enumerate() {
        match &cm.reply_to {
            Some(parent_id) if id_set.contains_key(parent_id.as_str()) => {
                children_map.entry(parent_id.as_str()).or_default().push(i);
            }
            _ => root_indices.push(i),
        }
    }

    // Sort roots by line number ascending.
    root_indices.sort_by_key(|&i| comments[i].line);

    // Sort each children list by timestamp ascending.
    for children in children_map.values_mut() {
        children.sort_by(|&a, &b| comments[a].ts.cmp(&comments[b].ts));
    }

    root_indices
        .iter()
        .map(|&ri| build_node(ri, comments, &children_map))
        .collect()
}

// ---------------------------------------------------------------------------
// Pending calculation
// ---------------------------------------------------------------------------

/// Count the number of pending comments.
///
/// A comment is pending if it has a non-empty `to` field and at least one
/// addressee has not acknowledged it.
pub(crate) fn count_pending(comments: &[&Comment]) -> usize {
    comments.iter().filter(|cm| is_pending(cm)).count()
}

/// Check if a comment is pending.
pub(crate) fn is_pending(cm: &Comment) -> bool {
    if cm.to.is_empty() {
        return false;
    }
    let acked_authors: Vec<&str> = cm.ack.iter().map(|a| a.author.as_str()).collect();
    cm.to
        .iter()
        .any(|addr| !acked_authors.contains(&addr.as_str()))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Format comments from a document as a pretty-printed threaded display.
///
/// Root comments are sorted by line number (document order).
/// Replies are nested under their parents, sorted by timestamp ascending.
/// Returns the formatted string ready for display.
#[must_use]
pub fn format_comments_pretty(file_path: &str, comments: &[&Comment]) -> String {
    let forest = build_comment_tree(comments);

    let mut out = String::new();
    let mut first = true;

    for node in &forest {
        render_node(&mut out, file_path, node, 0, &mut first);
    }

    // Footer.
    let total = comments.len();
    let pending = count_pending(comments);
    let _ = writeln!(out, "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
    let _ = write!(out, "{total} comments \u{00b7} {pending} pending");

    out
}

/// Render a single comment node at the given depth, writing to `out`.
///
/// `depth` controls indentation: 0 = root (2 spaces), 1 = reply (4 spaces), etc.
/// `first` tracks whether this is the first comment (no leading blank line).
fn render_node(
    out: &mut String,
    file_path: &str,
    node: &CommentNode<'_>,
    depth: usize,
    first: &mut bool,
) {
    let cm = node.comment;
    let indent = " ".repeat((depth + 1) * 2);

    // Blank line between comments (not before the first one).
    if *first {
        *first = false;
    } else {
        out.push('\n');
    }

    // file:line link.
    let _ = writeln!(out, "{file_path}:{}", cm.line);

    // Header: id . author (type) . timestamp.
    let author_type_str = match cm.author_type {
        AuthorType::Agent => "agent",
        AuthorType::Human => "human",
    };
    let ts_short = cm.ts.format("%Y-%m-%d %H:%M");
    let _ = writeln!(
        out,
        "{indent}{} \u{00b7} {} ({author_type_str}) \u{00b7} {ts_short}",
        cm.id, cm.author
    );

    // Threading marker for replies.
    if let Some(parent_id) = &cm.reply_to {
        let _ = writeln!(out, "{indent}\u{2502} \u{2934} reply-to: {parent_id}");
    }

    // Addressees.
    if !cm.to.is_empty() {
        let _ = writeln!(out, "{indent}\u{2502} to: {}", cm.to.join(", "));
    }

    // Content lines (truncated at MAX_CONTENT_LINES).
    let content_lines: Vec<&str> = cm.content.lines().collect();
    let truncate = content_lines.len() > MAX_CONTENT_LINES;
    let display_lines = if truncate {
        MAX_CONTENT_LINES - 1
    } else {
        content_lines.len()
    };

    for line in content_lines.iter().take(display_lines) {
        let _ = writeln!(out, "{indent}\u{2502} {line}");
    }
    if truncate {
        let _ = writeln!(out, "{indent}\u{2502} ...");
    }

    // Reactions (before status).
    for (emoji, authors) in &cm.reactions {
        let _ = writeln!(out, "{indent}\u{2502} {emoji} {}", authors.join(", "));
    }

    // Status: acked or pending.
    if cm.ack.is_empty() {
        let _ = writeln!(out, "{indent}\u{2502} pending");
    } else {
        for ack_entry in &cm.ack {
            let ack_ts = ack_entry.ts.format("%Y-%m-%d %H:%M");
            let _ = writeln!(
                out,
                "{indent}\u{2502} \u{2713} acked by {} @ {ack_ts}",
                ack_entry.author
            );
        }
    }

    // Recursively render children.
    for child in &node.children {
        render_node(out, file_path, child, depth + 1, first);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
