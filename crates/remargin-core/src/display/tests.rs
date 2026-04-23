//! Tests for pretty-print comment display with threaded nesting.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::PathBuf;

use chrono::DateTime;

use crate::display::{
    build_comment_tree, count_pending, format_comments_pretty, format_query_pretty, is_pending,
};
use crate::operations::query::{ExpandedComment, QueryResult};
use crate::parser::{Acknowledgment, AuthorType, Comment};

/// Parameters for building a test comment.
struct TestComment<'param> {
    ack: Vec<Acknowledgment>,
    author: &'param str,
    author_type: AuthorType,
    content: &'param str,
    id: &'param str,
    line: usize,
    reactions: BTreeMap<String, Vec<String>>,
    reply_to: Option<&'param str>,
    to: Vec<&'param str>,
    ts: &'param str,
}

impl Default for TestComment<'_> {
    fn default() -> Self {
        Self {
            ack: Vec::new(),
            author: "eduardo",
            author_type: AuthorType::Human,
            content: "Test content.",
            id: "abc",
            line: 10,
            reactions: BTreeMap::new(),
            reply_to: None,
            to: Vec::new(),
            ts: "2026-04-06T14:00:00-04:00",
        }
    }
}

fn build_comment(params: TestComment<'_>) -> Comment {
    Comment {
        ack: params.ack,
        attachments: Vec::new(),
        author: String::from(params.author),
        author_type: params.author_type,
        checksum: String::from("sha256:test"),
        content: String::from(params.content),
        id: String::from(params.id),
        line: params.line,
        reactions: params.reactions,
        remargin_kind: Vec::new(),
        reply_to: params.reply_to.map(String::from),
        signature: None,
        thread: None,
        to: params.to.into_iter().map(String::from).collect(),
        ts: DateTime::parse_from_rfc3339(params.ts).unwrap(),
    }
}

fn make_comment(id: &str, line: usize, ts: &str) -> Comment {
    build_comment(TestComment {
        id,
        line,
        ts,
        ..TestComment::default()
    })
}

fn make_reply(id: &str, line: usize, ts: &str, reply_to: &str) -> Comment {
    build_comment(TestComment {
        author: "claude",
        author_type: AuthorType::Agent,
        content: "Reply content.",
        id,
        line,
        reply_to: Some(reply_to),
        ts,
        ..TestComment::default()
    })
}

fn make_ack(author: &str, ts: &str) -> Acknowledgment {
    Acknowledgment {
        author: String::from(author),
        ts: DateTime::parse_from_rfc3339(ts).unwrap(),
    }
}

#[test]
fn tree_single_root() {
    let cm = make_comment("abc", 10, "2026-04-06T14:00:00-04:00");
    let comments: Vec<&Comment> = vec![&cm];
    let forest = build_comment_tree(&comments);

    assert_eq!(forest.len(), 1);
    assert_eq!(forest[0].comment.id, "abc");
    assert!(forest[0].children.is_empty());
}

#[test]
fn tree_root_with_replies() {
    let root = make_comment("abc", 10, "2026-04-06T14:00:00-04:00");
    let reply_b = make_reply("bbb", 20, "2026-04-06T14:05:00-04:00", "abc");
    let reply_c = make_reply("ccc", 30, "2026-04-06T14:03:00-04:00", "abc");
    let comments: Vec<&Comment> = vec![&root, &reply_b, &reply_c];
    let forest = build_comment_tree(&comments);

    assert_eq!(forest.len(), 1);
    assert_eq!(forest[0].comment.id, "abc");
    assert_eq!(forest[0].children.len(), 2);
    // Children sorted by ts: ccc (14:03) before bbb (14:05).
    assert_eq!(forest[0].children[0].comment.id, "ccc");
    assert_eq!(forest[0].children[1].comment.id, "bbb");
}

#[test]
fn tree_deep_nesting() {
    let a = make_comment("aaa", 10, "2026-04-06T14:00:00-04:00");
    let b = make_reply("bbb", 20, "2026-04-06T14:01:00-04:00", "aaa");
    let c = make_reply("ccc", 30, "2026-04-06T14:02:00-04:00", "bbb");
    let d = make_reply("ddd", 40, "2026-04-06T14:03:00-04:00", "ccc");
    let comments: Vec<&Comment> = vec![&a, &b, &c, &d];
    let forest = build_comment_tree(&comments);

    assert_eq!(forest.len(), 1);
    assert_eq!(forest[0].comment.id, "aaa");
    assert_eq!(forest[0].children[0].comment.id, "bbb");
    assert_eq!(forest[0].children[0].children[0].comment.id, "ccc");
    assert_eq!(
        forest[0].children[0].children[0].children[0].comment.id,
        "ddd"
    );
}

#[test]
fn tree_multiple_roots() {
    let root_a = make_comment("aaa", 50, "2026-04-06T14:00:00-04:00");
    let root_b = make_comment("bbb", 10, "2026-04-06T14:01:00-04:00");
    let reply_c = make_reply("ccc", 55, "2026-04-06T14:02:00-04:00", "aaa");
    let comments: Vec<&Comment> = vec![&root_a, &root_b, &reply_c];
    let forest = build_comment_tree(&comments);

    // Two roots sorted by line: bbb (line 10) before aaa (line 50).
    assert_eq!(forest.len(), 2);
    assert_eq!(forest[0].comment.id, "bbb");
    assert_eq!(forest[1].comment.id, "aaa");
    assert_eq!(forest[1].children.len(), 1);
    assert_eq!(forest[1].children[0].comment.id, "ccc");
}

#[test]
fn tree_orphan_reply() {
    let orphan = make_reply("bbb", 10, "2026-04-06T14:00:00-04:00", "nonexistent");
    let comments: Vec<&Comment> = vec![&orphan];
    let forest = build_comment_tree(&comments);

    // Orphan treated as root.
    assert_eq!(forest.len(), 1);
    assert_eq!(forest[0].comment.id, "bbb");
}

#[test]
fn tree_children_sorted_by_ts() {
    let root = make_comment("aaa", 10, "2026-04-06T12:00:00-04:00");
    let late = make_reply("bbb", 20, "2026-04-06T14:00:00-04:00", "aaa");
    let early = make_reply("ccc", 30, "2026-04-06T13:00:00-04:00", "aaa");
    let comments: Vec<&Comment> = vec![&root, &late, &early];
    let forest = build_comment_tree(&comments);

    assert_eq!(forest[0].children[0].comment.id, "ccc");
    assert_eq!(forest[0].children[1].comment.id, "bbb");
}

#[test]
fn tree_roots_sorted_by_line() {
    let a = make_comment("aaa", 50, "2026-04-06T14:00:00-04:00");
    let b = make_comment("bbb", 10, "2026-04-06T14:01:00-04:00");
    let comments: Vec<&Comment> = vec![&a, &b];
    let forest = build_comment_tree(&comments);

    assert_eq!(forest[0].comment.id, "bbb");
    assert_eq!(forest[1].comment.id, "aaa");
}

#[test]
fn tree_empty() {
    let comments: Vec<&Comment> = vec![];
    let forest = build_comment_tree(&comments);
    assert!(forest.is_empty());
}

#[test]
fn render_root_comment() {
    let cm = build_comment(TestComment {
        id: "abc",
        author: "eduardo",
        ts: "2026-04-06T14:32:00-04:00",
        line: 25,
        content: "The comment content goes here.",
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("docs/design.md", &comments);

    assert!(output.contains("docs/design.md:25"));
    assert!(output.contains("abc \u{00b7} eduardo (human) \u{00b7} 2026-04-06 14:32"));
    assert!(output.contains("\u{2502} The comment content goes here."));
}

#[test]
fn render_reply_indentation() {
    let root = build_comment(TestComment {
        id: "abc",
        author: "eduardo",
        ts: "2026-04-06T14:32:00-04:00",
        line: 25,
        content: "Root content.",
        ..TestComment::default()
    });
    let reply = build_comment(TestComment {
        id: "xyz",
        author: "claude",
        author_type: AuthorType::Agent,
        ts: "2026-04-06T14:33:00-04:00",
        line: 35,
        content: "Reply content.",
        reply_to: Some("abc"),
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&root, &reply];
    let output = format_comments_pretty("file.md", &comments);

    // Root header indented 2 spaces, reply header indented 4 spaces.
    assert!(output.contains("  abc \u{00b7} eduardo (human)"));
    assert!(output.contains("    xyz \u{00b7} claude (agent)"));
}

#[test]
fn render_deep_indent() {
    let a = make_comment("aaa", 10, "2026-04-06T14:00:00-04:00");
    let b = make_reply("bbb", 20, "2026-04-06T14:01:00-04:00", "aaa");
    let c = make_reply("ccc", 30, "2026-04-06T14:02:00-04:00", "bbb");
    let comments: Vec<&Comment> = vec![&a, &b, &c];
    let output = format_comments_pretty("file.md", &comments);

    // Depth 0 = 2 spaces, depth 1 = 4, depth 2 = 6.
    assert!(output.contains("      ccc \u{00b7}"));
}

#[test]
fn render_threading_marker() {
    let root = make_comment("abc", 10, "2026-04-06T14:00:00-04:00");
    let reply = make_reply("xyz", 20, "2026-04-06T14:01:00-04:00", "abc");
    let comments: Vec<&Comment> = vec![&root, &reply];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2502} \u{2934} reply-to: abc"));
}

#[test]
fn render_pending_status() {
    let cm = build_comment(TestComment {
        id: "abc",
        content: "Need your review.",
        to: vec!["alice"],
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2502} pending"));
}

#[test]
fn render_acked_status() {
    let ack = make_ack("eduardo", "2026-04-06T15:00:00-04:00");
    let cm = build_comment(TestComment {
        id: "abc",
        author: "claude",
        author_type: AuthorType::Agent,
        content: "Some content.",
        ack: vec![ack],
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2502} \u{2713} acked by eduardo @ 2026-04-06 15:00"));
}

#[test]
fn render_multiple_acks() {
    let ack1 = make_ack("alice", "2026-04-06T15:00:00-04:00");
    let ack2 = make_ack("bob", "2026-04-06T15:30:00-04:00");
    let cm = build_comment(TestComment {
        id: "abc",
        to: vec!["alice", "bob"],
        ack: vec![ack1, ack2],
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2713} acked by alice @ 2026-04-06 15:00"));
    assert!(output.contains("\u{2713} acked by bob @ 2026-04-06 15:30"));
}

#[test]
fn render_reactions() {
    let mut reactions = BTreeMap::new();
    reactions.insert(
        String::from("\u{1f44d}"),
        vec![String::from("jorge"), String::from("alice")],
    );
    let cm = build_comment(TestComment {
        id: "abc",
        content: "Nice idea.",
        reactions,
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2502} \u{1f44d} jorge, alice"));
}

#[test]
fn render_content_truncation() {
    let content = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7";
    let cm = build_comment(TestComment {
        id: "abc",
        content,
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2502} Line 1"));
    assert!(output.contains("\u{2502} Line 2"));
    assert!(output.contains("\u{2502} Line 3"));
    assert!(output.contains("\u{2502} Line 4"));
    assert!(output.contains("\u{2502} ..."));
    // Lines 5, 6, 7 should NOT appear.
    assert!(!output.contains("\u{2502} Line 5"));
    assert!(!output.contains("\u{2502} Line 6"));
    assert!(!output.contains("\u{2502} Line 7"));
}

#[test]
fn render_addressees() {
    let cm = build_comment(TestComment {
        id: "abc",
        content: "Review this.",
        to: vec!["alice", "bob"],
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2502} to: alice, bob"));
}

#[test]
fn render_footer() {
    let cm1 = build_comment(TestComment {
        id: "aaa",
        line: 10,
        ts: "2026-04-06T14:00:00-04:00",
        to: vec!["alice"],
        ..TestComment::default()
    });
    let cm2 = build_comment(TestComment {
        id: "bbb",
        line: 20,
        ts: "2026-04-06T14:01:00-04:00",
        to: vec!["bob"],
        ..TestComment::default()
    });
    let ack = make_ack("charlie", "2026-04-06T15:00:00-04:00");
    let cm3 = build_comment(TestComment {
        id: "ccc",
        line: 30,
        ts: "2026-04-06T14:02:00-04:00",
        to: vec!["charlie"],
        ack: vec![ack],
        ..TestComment::default()
    });
    // Post-rem-4j91: broadcasts count as pending unless acked. Close
    // these two so the footer still asserts "2 pending" (from cm1/cm2).
    let ack4 = make_ack("dave", "2026-04-06T15:10:00-04:00");
    let cm4 = build_comment(TestComment {
        id: "ddd",
        line: 40,
        ts: "2026-04-06T14:03:00-04:00",
        ack: vec![ack4],
        ..TestComment::default()
    });
    let ack5 = make_ack("eve", "2026-04-06T15:11:00-04:00");
    let cm5 = build_comment(TestComment {
        id: "eee",
        line: 50,
        ts: "2026-04-06T14:04:00-04:00",
        ack: vec![ack5],
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm1, &cm2, &cm3, &cm4, &cm5];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"));
    assert!(output.contains("5 comments \u{00b7} 2 pending"));
}

#[test]
fn render_empty_footer() {
    let comments: Vec<&Comment> = vec![];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"));
    assert!(output.contains("0 comments \u{00b7} 0 pending"));
}

#[test]
fn render_no_pending() {
    let ack1 = make_ack("alice", "2026-04-06T15:00:00-04:00");
    let cm1 = build_comment(TestComment {
        id: "aaa",
        to: vec!["alice"],
        ack: vec![ack1],
        ..TestComment::default()
    });
    // Post-rem-4j91: a broadcast (no `to`) counts as pending unless
    // somebody has acked it. Close both broadcasts with an ack so the
    // footer reads "0 pending".
    let ack2 = make_ack("bob", "2026-04-06T14:10:00-04:00");
    let cm2 = build_comment(TestComment {
        id: "bbb",
        line: 20,
        ts: "2026-04-06T14:01:00-04:00",
        ack: vec![ack2],
        ..TestComment::default()
    });
    let ack3 = make_ack("bob", "2026-04-06T14:11:00-04:00");
    let cm3 = build_comment(TestComment {
        id: "ccc",
        line: 30,
        ts: "2026-04-06T14:02:00-04:00",
        ack: vec![ack3],
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm1, &cm2, &cm3];
    let output = format_comments_pretty("file.md", &comments);

    assert!(output.contains("3 comments \u{00b7} 0 pending"));
}

#[test]
fn render_content_with_special_chars() {
    let content = "Line with \u{2502} bar and `backticks` here";
    let cm = build_comment(TestComment {
        id: "abc",
        content,
        ..TestComment::default()
    });
    let comments: Vec<&Comment> = vec![&cm];
    let output = format_comments_pretty("file.md", &comments);

    // The special characters should appear literally.
    assert!(output.contains("Line with \u{2502} bar and `backticks` here"));
}

#[test]
fn render_orphan_mixed_with_roots() {
    let root1 = make_comment("aaa", 10, "2026-04-06T14:00:00-04:00");
    let root2 = make_comment("bbb", 30, "2026-04-06T14:01:00-04:00");
    let orphan = make_reply("ccc", 20, "2026-04-06T14:02:00-04:00", "nonexistent");
    let comments: Vec<&Comment> = vec![&root1, &root2, &orphan];
    let output = format_comments_pretty("file.md", &comments);

    // All 3 appear as roots in document order: aaa (10), ccc (20), bbb (30).
    let aaa_pos = output.find("aaa \u{00b7}").unwrap();
    let ccc_pos = output.find("ccc \u{00b7}").unwrap();
    let bbb_pos = output.find("bbb \u{00b7}").unwrap();
    assert!(aaa_pos < ccc_pos);
    assert!(ccc_pos < bbb_pos);
}

#[test]
fn broadcast_no_ack_is_pending() {
    // Broadcast (empty `to`) with no acks is pending under the
    // post-rem-4j91 semantics: a fresh broadcast keeps the
    // conversation open until somebody acks.
    let cm = make_comment("abc", 10, "2026-04-06T14:00:00-04:00");
    assert!(is_pending(&cm));
    assert_eq!(count_pending(&[&cm]), 1);
}

#[test]
fn broadcast_with_ack_not_pending() {
    // Any ack closes a broadcast from the "is this conversation
    // still open?" perspective used by count_pending.
    let ack = make_ack("alice", "2026-04-06T15:00:00-04:00");
    let cm = build_comment(TestComment {
        id: "abc",
        to: vec![],
        ack: vec![ack],
        ..TestComment::default()
    });
    assert!(!is_pending(&cm));
    assert_eq!(count_pending(&[&cm]), 0);
}

#[test]
fn pending_with_partial_ack() {
    let ack = make_ack("alice", "2026-04-06T15:00:00-04:00");
    let cm = build_comment(TestComment {
        id: "abc",
        to: vec!["alice", "bob"],
        ack: vec![ack],
        ..TestComment::default()
    });
    // alice acked but bob didn't -> still pending.
    assert!(is_pending(&cm));
}

#[test]
fn pending_all_acked() {
    let ack1 = make_ack("alice", "2026-04-06T15:00:00-04:00");
    let ack2 = make_ack("bob", "2026-04-06T15:30:00-04:00");
    let cm = build_comment(TestComment {
        id: "abc",
        to: vec!["alice", "bob"],
        ack: vec![ack1, ack2],
        ..TestComment::default()
    });
    assert!(!is_pending(&cm));
}

/// Build an `ExpandedComment` for query pretty-print tests.
fn make_expanded(
    id: &str,
    author: &str,
    author_type: AuthorType,
    line: usize,
    content: &str,
    ts: &str,
    file: &str,
) -> ExpandedComment {
    ExpandedComment {
        ack: Vec::new(),
        attachments: Vec::new(),
        author: String::from(author),
        author_type,
        checksum: String::from("sha256:test"),
        content: String::from(content),
        file: PathBuf::from(file),
        id: String::from(id),
        line,
        reactions: BTreeMap::new(),
        reply_to: None,
        signature: None,
        thread: None,
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339(ts).unwrap(),
    }
}

fn make_query_result(path: &str, comments: Vec<ExpandedComment>) -> QueryResult {
    let comment_count = u32::try_from(comments.len()).unwrap_or(u32::MAX);
    let last_activity = comments.iter().map(|c| c.ts).max();
    // Pending count is computed by the pretty-printer from the
    // comments themselves (expanded mode); the stored
    // QueryResult.pending_count is unused by format_query_pretty for
    // the footer, but we keep it consistent for readers that inspect
    // the struct.
    let pending_count = u32::try_from(
        comments
            .iter()
            .filter(|c| {
                if c.to.is_empty() {
                    c.ack.is_empty()
                } else {
                    let acked: Vec<&str> = c.ack.iter().map(|a| a.author.as_str()).collect();
                    c.to.iter().any(|addr| !acked.contains(&addr.as_str()))
                }
            })
            .count(),
    )
    .unwrap_or(u32::MAX);
    QueryResult {
        comment_count,
        comments,
        last_activity,
        path: PathBuf::from(path),
        pending_count,
        pending_for: Vec::new(),
    }
}

#[test]
fn query_pretty_single_file() {
    let cm = make_expanded(
        "abc",
        "eduardo",
        AuthorType::Human,
        10,
        "Fix this bug.",
        "2026-04-06T14:00:00-04:00",
        "docs/design.md",
    );
    let result = make_query_result("docs/design.md", vec![cm]);
    let output = format_query_pretty(&[result], None);

    // Post-rem-4j91: a broadcast (empty `to`) with no acks counts as
    // pending, so "1 pending" here (one broadcast, unacked).
    assert!(output.contains("docs/design.md (1 comments, 1 pending)"));
    assert!(output.contains("docs/design.md:10"));
    assert!(output.contains("abc \u{00b7} eduardo (human) \u{00b7} 2026-04-06 14:00"));
    assert!(output.contains("\u{2502} Fix this bug."));
    // Grand footer.
    assert!(output.contains("\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}"));
    assert!(output.contains("1 pending across 1 files"));
}

#[test]
fn query_pretty_multi_file() {
    let cm1 = make_expanded(
        "aaa",
        "eduardo",
        AuthorType::Human,
        5,
        "Comment in B.",
        "2026-04-06T14:00:00-04:00",
        "src/b.md",
    );
    let cm2 = make_expanded(
        "bbb",
        "claude",
        AuthorType::Agent,
        15,
        "Comment in A.",
        "2026-04-06T14:01:00-04:00",
        "src/a.md",
    );
    let result_b = make_query_result("src/b.md", vec![cm1]);
    let result_a = make_query_result("src/a.md", vec![cm2]);

    // Pass in non-alphabetical order; output should sort alphabetically.
    let output = format_query_pretty(&[result_b, result_a], None);

    let pos_a = output.find("src/a.md (1 comments").unwrap();
    let pos_b = output.find("src/b.md (1 comments").unwrap();
    assert!(pos_a < pos_b, "Files should be sorted alphabetically");
    // Post-rem-4j91: broadcasts with no acks count as pending.
    assert!(output.contains("2 pending across 2 files"));
}

#[test]
fn query_pretty_pending_for() {
    let mut cm = make_expanded(
        "abc",
        "eduardo",
        AuthorType::Human,
        10,
        "Review this.",
        "2026-04-06T14:00:00-04:00",
        "design.md",
    );
    cm.to = vec![String::from("alice")];
    let mut result = make_query_result("design.md", vec![cm]);
    result.pending_count = 1;

    let output = format_query_pretty(&[result], Some("alice"));

    assert!(output.contains("1 pending for alice"));
    // Per-file header also uses the filter name.
    assert!(output.contains("design.md (1 comments, 1 pending for alice)"));
    // Grand footer.
    assert!(output.contains("1 pending for alice across 1 files"));
}

#[test]
fn query_pretty_flat_not_threaded() {
    // A reply should appear at depth=0 (flat), not nested.
    let root = make_expanded(
        "aaa",
        "eduardo",
        AuthorType::Human,
        20,
        "Root comment.",
        "2026-04-06T14:00:00-04:00",
        "file.md",
    );
    let mut reply = make_expanded(
        "bbb",
        "claude",
        AuthorType::Agent,
        10,
        "Reply comment.",
        "2026-04-06T14:01:00-04:00",
        "file.md",
    );
    reply.reply_to = Some(String::from("aaa"));
    let result = make_query_result("file.md", vec![root, reply]);

    let output = format_query_pretty(&[result], None);

    // Both comments should be at the same indentation (2 spaces).
    assert!(output.contains("  aaa \u{00b7} eduardo (human)"));
    assert!(output.contains("  bbb \u{00b7} claude (agent)"));
    // Reply should still show the reply-to marker.
    assert!(output.contains("\u{2502} \u{2934} reply-to: aaa"));
    // Reply (line 10) should come BEFORE root (line 20) since sorted by line.
    let bbb_pos = output.find("bbb \u{00b7}").unwrap();
    let aaa_pos = output.find("aaa \u{00b7}").unwrap();
    assert!(
        bbb_pos < aaa_pos,
        "Comments sorted by line number, not thread"
    );
}

#[test]
fn query_pretty_content_truncation() {
    let content = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7";
    let cm = make_expanded(
        "abc",
        "eduardo",
        AuthorType::Human,
        10,
        content,
        "2026-04-06T14:00:00-04:00",
        "file.md",
    );
    let result = make_query_result("file.md", vec![cm]);
    let output = format_query_pretty(&[result], None);

    assert!(output.contains("\u{2502} Line 1"));
    assert!(output.contains("\u{2502} Line 4"));
    assert!(output.contains("\u{2502} ..."));
    // Lines beyond truncation should not appear.
    assert!(!output.contains("\u{2502} Line 5"));
    assert!(!output.contains("\u{2502} Line 7"));
}

#[test]
fn query_pretty_reactions() {
    let mut reactions = BTreeMap::new();
    reactions.insert(
        String::from("\u{1f44d}"),
        vec![String::from("alice"), String::from("bob")],
    );
    let mut cm = make_expanded(
        "abc",
        "eduardo",
        AuthorType::Human,
        10,
        "Nice idea.",
        "2026-04-06T14:00:00-04:00",
        "file.md",
    );
    cm.reactions = reactions;
    let result = make_query_result("file.md", vec![cm]);
    let output = format_query_pretty(&[result], None);

    assert!(output.contains("\u{2502} \u{1f44d} alice, bob"));
}

#[test]
fn query_pretty_acked_status() {
    let ack = Acknowledgment {
        author: String::from("alice"),
        ts: DateTime::parse_from_rfc3339("2026-04-06T15:00:00-04:00").unwrap(),
    };
    let mut cm = make_expanded(
        "abc",
        "eduardo",
        AuthorType::Human,
        10,
        "Review this.",
        "2026-04-06T14:00:00-04:00",
        "file.md",
    );
    cm.to = vec![String::from("alice")];
    cm.ack = vec![ack];
    let result = make_query_result("file.md", vec![cm]);
    let output = format_query_pretty(&[result], None);

    assert!(output.contains("\u{2502} \u{2713} acked by alice @ 2026-04-06 15:00"));
    // Should NOT contain "pending" line for this comment.
    assert!(!output.contains("\u{2502} pending\n"));
}

#[test]
fn query_pretty_empty_results() {
    let output = format_query_pretty(&[], None);

    // Grand footer still present.
    assert!(output.contains("\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}"));
    assert!(output.contains("0 pending across 0 files"));
}

#[test]
fn query_pretty_no_filter() {
    let cm = make_expanded(
        "abc",
        "eduardo",
        AuthorType::Human,
        10,
        "Content.",
        "2026-04-06T14:00:00-04:00",
        "file.md",
    );
    let result = make_query_result("file.md", vec![cm]);
    let output = format_query_pretty(&[result], None);

    // Without filter_name, footer should just say "N pending" not "pending for <name>".
    // Post-rem-4j91: the unacked broadcast contributes one pending.
    assert!(output.contains("1 pending\n"));
    assert!(!output.contains("pending for"));
}

#[test]
fn query_pretty_file_line_links() {
    let cm1 = make_expanded(
        "aaa",
        "eduardo",
        AuthorType::Human,
        42,
        "Comment at line 42.",
        "2026-04-06T14:00:00-04:00",
        "docs/guide.md",
    );
    let cm2 = make_expanded(
        "bbb",
        "claude",
        AuthorType::Agent,
        7,
        "Comment at line 7.",
        "2026-04-06T14:01:00-04:00",
        "docs/guide.md",
    );
    let result = make_query_result("docs/guide.md", vec![cm1, cm2]);
    let output = format_query_pretty(&[result], None);

    assert!(output.contains("docs/guide.md:42"));
    assert!(output.contains("docs/guide.md:7"));
}

#[test]
fn query_pretty_reply_marker() {
    let mut cm = make_expanded(
        "xyz",
        "claude",
        AuthorType::Agent,
        15,
        "Reply content.",
        "2026-04-06T14:01:00-04:00",
        "file.md",
    );
    cm.reply_to = Some(String::from("abc"));
    let result = make_query_result("file.md", vec![cm]);
    let output = format_query_pretty(&[result], None);

    assert!(output.contains("  \u{2502} \u{2934} reply-to: abc"));
}

#[test]
fn query_no_pretty_unchanged() {
    // Verify that the non-pretty format_query_pretty function signature exists
    // and the pretty output format is distinct from what non-pretty would produce.
    let cm = make_expanded(
        "abc",
        "eduardo",
        AuthorType::Human,
        10,
        "Content.",
        "2026-04-06T14:00:00-04:00",
        "file.md",
    );
    let result = make_query_result("file.md", vec![cm]);
    let output = format_query_pretty(&[result], None);

    // Pretty output has file:line links, vertical bars, and footer separators.
    assert!(output.contains("file.md:10"));
    assert!(output.contains("\u{2502}"));
    assert!(output.contains("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"));
    assert!(output.contains("\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}"));
}
