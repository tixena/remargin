//! Thread tree building and reply chain resolution.
//!
//! Comments can form reply chains. This module provides functions to build
//! a tree structure from a flat list of comments, and to find all descendants
//! of a given comment for cascading operations.

#[cfg(test)]
mod tests;

use crate::parser::Comment;

/// A tree of comment threads.
#[derive(Debug)]
#[non_exhaustive]
pub struct ThreadTree {
    /// Top-level comments (roots), each with their nested reply chains.
    pub roots: Vec<ThreadNode>,
}

/// A node in the thread tree.
#[derive(Debug)]
#[non_exhaustive]
pub struct ThreadNode {
    /// Nested replies to this comment.
    pub children: Vec<Self>,
    /// The comment at this node.
    pub comment_id: String,
}

/// Build a thread tree from a list of comments.
///
/// Top-level comments (no `reply_to`) become roots.
/// Comments with `reply_to` are nested under their parent.
#[must_use]
pub fn build_thread_tree(comments: &[&Comment]) -> ThreadTree {
    let mut roots: Vec<ThreadNode> = Vec::new();

    // First, identify root comments (no reply_to).
    for cm in comments {
        if cm.reply_to.is_none() {
            let node = build_node(cm, comments);
            roots.push(node);
        }
    }

    ThreadTree { roots }
}

/// Recursively build a tree node for a comment.
fn build_node(comment: &Comment, all_comments: &[&Comment]) -> ThreadNode {
    let children: Vec<ThreadNode> = all_comments
        .iter()
        .filter(|cm| cm.reply_to.as_deref() == Some(&comment.id))
        .map(|child| build_node(child, all_comments))
        .collect();

    ThreadNode {
        comment_id: comment.id.clone(),
        children,
    }
}

/// Find all descendants of a comment (for cascading operations).
///
/// Returns IDs of all children, grandchildren, etc. in depth-first order.
#[must_use]
pub fn find_descendants(comments: &[&Comment], ancestor_id: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut stack = vec![String::from(ancestor_id)];

    while let Some(parent_id) = stack.pop() {
        for cm in comments {
            if cm.reply_to.as_deref() == Some(parent_id.as_str()) && !result.contains(&cm.id) {
                result.push(cm.id.clone());
                stack.push(cm.id.clone());
            }
        }
    }

    result
}

/// Resolve the thread root for a new reply.
///
/// - If the parent has a `thread` field, use it (same root).
/// - If the parent has no `thread`, the parent IS the root.
/// - If the parent doesn't exist (dangling reference), use the `reply_to` value.
#[must_use]
pub fn resolve_thread_root(comments: &[&Comment], parent_id: &str) -> String {
    comments
        .iter()
        .find(|cm| cm.id == parent_id)
        .and_then(|parent| parent.thread.clone())
        .unwrap_or_else(|| String::from(parent_id))
}
