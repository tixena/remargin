//! Pretty-print comment display with threaded nesting.

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt::Write as _;

use crate::operations::query::{ExpandedComment, QueryResult};
use crate::parser::Comment;

const MAX_CONTENT_LINES: usize = 5;

/// A node in the comment tree: a comment plus its direct replies.
pub(crate) struct CommentNode<'cm> {
    /// Direct replies, sorted by timestamp ascending.
    pub children: Vec<Self>,
    pub comment: &'cm Comment,
}

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
    let id_set: BTreeMap<&str, usize> = comments
        .iter()
        .enumerate()
        .map(|(i, cm)| (cm.id.as_str(), i))
        .collect();

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

    root_indices.sort_by_key(|&i| comments[i].line);

    for children in children_map.values_mut() {
        children.sort_by(|&a, &b| comments[a].ts.cmp(&comments[b].ts));
    }

    root_indices
        .iter()
        .map(|&ri| build_node(ri, comments, &children_map))
        .collect()
}

/// A comment is pending when the conversation is still open: a
/// directed comment (`to` non-empty) with at least one recipient who
/// has not acknowledged, or a broadcast (`to` empty) with no acks at
/// all (rem-4j91).
pub(crate) fn count_pending(comments: &[&Comment]) -> usize {
    comments.iter().filter(|cm| is_pending(cm)).count()
}

pub(crate) fn is_pending(cm: &Comment) -> bool {
    if cm.to.is_empty() {
        return cm.ack.is_empty();
    }
    let acked_authors: Vec<&str> = cm.ack.iter().map(|a| a.author.as_str()).collect();
    cm.to
        .iter()
        .any(|addr| !acked_authors.contains(&addr.as_str()))
}

/// Root comments are sorted by line number (document order).
/// Replies are nested under their parents, sorted by timestamp ascending.
#[must_use]
pub fn format_comments_pretty(file_path: &str, comments: &[&Comment]) -> String {
    let forest = build_comment_tree(comments);

    let mut out = String::new();
    let mut first = true;

    for node in &forest {
        render_node(&mut out, file_path, node, 0, &mut first);
    }

    let total = comments.len();
    let pending = count_pending(comments);
    let _ = writeln!(out, "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
    let _ = write!(out, "{total} comments \u{00b7} {pending} pending");

    out
}

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

    if *first {
        *first = false;
    } else {
        out.push('\n');
    }

    let _ = writeln!(out, "{file_path}:{}", cm.line);

    let author_type_str = cm.author_type.as_str();
    let ts_short = cm.ts.format("%Y-%m-%d %H:%M");
    let _ = writeln!(
        out,
        "{indent}{} \u{00b7} {} ({author_type_str}) \u{00b7} {ts_short}",
        cm.id, cm.author
    );

    if let Some(parent_id) = &cm.reply_to {
        let _ = writeln!(out, "{indent}\u{2502} \u{2934} reply-to: {parent_id}");
    }

    if !cm.to.is_empty() {
        let _ = writeln!(out, "{indent}\u{2502} to: {}", cm.to.join(", "));
    }

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

    for (emoji, entries) in cm.reactions.entries_by_emoji() {
        let authors: Vec<String> = entries.iter().map(|e| e.author.clone()).collect();
        let _ = writeln!(out, "{indent}\u{2502} {emoji} {}", authors.join(", "));
    }

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

    for child in &node.children {
        render_node(out, file_path, child, depth + 1, first);
    }
}

/// Format cross-document query results as a pretty-printed flat display.
///
/// Comments are shown flat (not threaded), grouped by file path (sorted
/// alphabetically). Each file group has a per-file header and footer with
/// pending counts. A grand footer summarises totals across all files.
///
/// When `filter_name` is provided, pending counts read "pending for <name>";
/// otherwise just "pending".
#[must_use]
pub fn format_query_pretty(results: &[QueryResult], filter_name: Option<&str>) -> String {
    let mut out = String::new();

    let mut sorted: Vec<&QueryResult> = results.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));

    let mut total_pending: usize = 0;
    let file_count = sorted.len();
    let mut first_file = true;

    for result in &sorted {
        if !first_file {
            out.push('\n');
        }
        first_file = false;

        let path_str = result.path.display().to_string();
        let comment_count = result.comments.len();
        let pending_count = count_pending_expanded(&result.comments);
        total_pending += pending_count;

        let pending_label = format_pending_label(pending_count, filter_name);
        let _ = writeln!(
            out,
            "{path_str} ({comment_count} comments, {pending_label})"
        );

        let mut comments: Vec<&ExpandedComment> = result.comments.iter().collect();
        comments.sort_by_key(|cm| cm.line);

        for cm in &comments {
            render_expanded_comment(&mut out, &path_str, cm);
        }

        let _ = writeln!(out, "\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
        let _ = write!(out, "{pending_label}");
    }

    let grand_pending_label = format_pending_label(total_pending, filter_name);
    let _ = write!(
        out,
        "\n\n\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\n{grand_pending_label} across {file_count} files"
    );

    out
}

fn render_expanded_comment(out: &mut String, file_path: &str, cm: &ExpandedComment) {
    out.push('\n');

    let _ = writeln!(out, "{file_path}:{}", cm.line);

    let author_type_str = cm.author_type.as_str();
    let ts_short = cm.ts.format("%Y-%m-%d %H:%M");
    let _ = writeln!(
        out,
        "  {} \u{00b7} {} ({author_type_str}) \u{00b7} {ts_short}",
        cm.id, cm.author
    );

    if let Some(parent_id) = &cm.reply_to {
        let _ = writeln!(out, "  \u{2502} \u{2934} reply-to: {parent_id}");
    }

    if !cm.to.is_empty() {
        let _ = writeln!(out, "  \u{2502} to: {}", cm.to.join(", "));
    }

    let content_lines: Vec<&str> = cm.content.lines().collect();
    let truncate = content_lines.len() > MAX_CONTENT_LINES;
    let display_lines = if truncate {
        MAX_CONTENT_LINES - 1
    } else {
        content_lines.len()
    };

    for line in content_lines.iter().take(display_lines) {
        let _ = writeln!(out, "  \u{2502} {line}");
    }
    if truncate {
        let _ = writeln!(out, "  \u{2502} ...");
    }

    for (emoji, entries) in cm.reactions.entries_by_emoji() {
        let authors: Vec<String> = entries.iter().map(|e| e.author.clone()).collect();
        let _ = writeln!(out, "  \u{2502} {emoji} {}", authors.join(", "));
    }

    if cm.ack.is_empty() {
        let _ = writeln!(out, "  \u{2502} pending");
    } else {
        for ack_entry in &cm.ack {
            let ack_ts = ack_entry.ts.format("%Y-%m-%d %H:%M");
            let _ = writeln!(
                out,
                "  \u{2502} \u{2713} acked by {} @ {ack_ts}",
                ack_entry.author
            );
        }
    }
}

fn count_pending_expanded(comments: &[ExpandedComment]) -> usize {
    comments.iter().filter(|cm| is_pending_expanded(cm)).count()
}

fn is_pending_expanded(cm: &ExpandedComment) -> bool {
    if cm.to.is_empty() {
        return cm.ack.is_empty();
    }
    let acked_authors: Vec<&str> = cm.ack.iter().map(|a| a.author.as_str()).collect();
    cm.to
        .iter()
        .any(|addr| !acked_authors.contains(&addr.as_str()))
}

fn format_pending_label(count: usize, filter_name: Option<&str>) -> String {
    filter_name.map_or_else(
        || format!("{count} pending"),
        |name| format!("{count} pending for {name}"),
    )
}

#[cfg(test)]
mod tests;
