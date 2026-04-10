//! Tests for threading operations.

extern crate alloc;

use alloc::collections::BTreeMap;

use chrono::DateTime;

use crate::operations::threading::{build_thread_tree, find_descendants, resolve_thread_root};
use crate::parser::{AuthorType, Comment};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_comment(id: &str, reply_to: Option<&str>, thread: Option<&str>) -> Comment {
    Comment {
        ack: Vec::new(),
        attachments: Vec::new(),
        author: String::from("eduardo"),
        author_type: AuthorType::Human,
        checksum: String::from("sha256:test"),
        content: String::from("Test content."),
        fence_depth: 3,
        id: String::from(id),
        line: 0,
        reactions: BTreeMap::new(),
        reply_to: reply_to.map(String::from),
        signature: None,
        thread: thread.map(String::from),
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339("2026-04-06T12:00:00-04:00").unwrap(),
    }
}

// ---------------------------------------------------------------------------
// Test 1: Reply to root -- thread = parent ID
// ---------------------------------------------------------------------------

#[test]
fn reply_to_root_thread_is_parent_id() {
    let root = make_comment("root", None, None);
    let comments: Vec<&Comment> = vec![&root];

    let thread = resolve_thread_root(&comments, "root");
    assert_eq!(thread, "root");
}

// ---------------------------------------------------------------------------
// Test 2: Reply to reply -- thread inherited
// ---------------------------------------------------------------------------

#[test]
fn reply_to_reply_inherits_thread() {
    let root = make_comment("root", None, None);
    let child = make_comment("child", Some("root"), Some("root"));
    let comments: Vec<&Comment> = vec![&root, &child];

    let thread = resolve_thread_root(&comments, "child");
    assert_eq!(thread, "root"); // Inherited from child's thread field
}

// ---------------------------------------------------------------------------
// Test 3: Dangling parent -- no crash
// ---------------------------------------------------------------------------

#[test]
fn dangling_parent_no_crash() {
    let comments: Vec<&Comment> = Vec::new();

    let thread = resolve_thread_root(&comments, "nonexistent");
    assert_eq!(thread, "nonexistent"); // Falls back to the reply_to value
}

// ---------------------------------------------------------------------------
// Test 4: Thread tree -- 5 comments in 2 threads
// ---------------------------------------------------------------------------

#[test]
fn thread_tree_two_threads() {
    let alpha_root = make_comment("a", None, None);
    let alpha_reply_one = make_comment("a1", Some("a"), Some("a"));
    let alpha_reply_two = make_comment("a2", Some("a"), Some("a"));
    let beta_root = make_comment("b", None, None);
    let beta_reply = make_comment("b1", Some("b"), Some("b"));

    let comments: Vec<&Comment> = vec![
        &alpha_root,
        &alpha_reply_one,
        &alpha_reply_two,
        &beta_root,
        &beta_reply,
    ];
    let tree = build_thread_tree(&comments);

    assert_eq!(tree.roots.len(), 2);
    assert_eq!(tree.roots[0].comment_id, "a");
    assert_eq!(tree.roots[0].children.len(), 2);
    assert_eq!(tree.roots[1].comment_id, "b");
    assert_eq!(tree.roots[1].children.len(), 1);
}

// ---------------------------------------------------------------------------
// Test 5: Find descendants -- 3-level chain
// ---------------------------------------------------------------------------

#[test]
fn find_descendants_three_levels() {
    let root = make_comment("root", None, None);
    let child = make_comment("child", Some("root"), Some("root"));
    let grandchild = make_comment("grandchild", Some("child"), Some("root"));

    let comments: Vec<&Comment> = vec![&root, &child, &grandchild];
    let descendants = find_descendants(&comments, "root");

    assert_eq!(descendants.len(), 2);
    assert!(descendants.contains(&String::from("child")));
    assert!(descendants.contains(&String::from("grandchild")));
}

// ---------------------------------------------------------------------------
// Test 6: No replies -- all are roots
// ---------------------------------------------------------------------------

#[test]
fn no_replies_all_roots() {
    let cm1 = make_comment("a", None, None);
    let cm2 = make_comment("b", None, None);
    let cm3 = make_comment("c", None, None);

    let comments: Vec<&Comment> = vec![&cm1, &cm2, &cm3];
    let tree = build_thread_tree(&comments);

    assert_eq!(tree.roots.len(), 3);
    for root in &tree.roots {
        assert!(root.children.is_empty());
    }
}
