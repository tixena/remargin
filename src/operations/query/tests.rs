//! Tests for the cross-document query engine.

use std::path::Path;

use os_shim::mock::MockSystem;

use crate::operations::query::{QueryFilter, query, resolve_comment_id};
use crate::parser::AuthorType;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn doc_with_pending() -> &'static str {
    "\
---
title: Needs Review
---

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:aaa
---
Please review this.
```
"
}

fn doc_all_acked() -> &'static str {
    "\
---
title: All Done
---

```remargin
---
id: def
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
checksum: sha256:bbb
ack:
  - eduardo@2026-04-06T11:00:00-04:00
---
Already reviewed.
```
"
}

fn setup_system() -> MockSystem {
    MockSystem::new()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/docs"))
        .unwrap()
        .with_file(
            Path::new("/project/docs/pending.md"),
            doc_with_pending().as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/project/docs/done.md"),
            doc_all_acked().as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/project/plain.md"), b"# No comments here\n")
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: Query all -- finds documents with comments
// ---------------------------------------------------------------------------

#[test]
fn query_all_with_comments() {
    let system = setup_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    // Should find 2 files (pending.md and done.md), not plain.md
    assert_eq!(results.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 2: Query pending only
// ---------------------------------------------------------------------------

#[test]
fn query_pending_only() {
    let system = setup_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].path.to_str().unwrap().contains("pending.md"));
    assert_eq!(results[0].pending_count, 1);
}

// ---------------------------------------------------------------------------
// Test 3: Query pending_for specific recipient
// ---------------------------------------------------------------------------

#[test]
fn query_pending_for_alice() {
    let system = setup_system();
    let filter = QueryFilter {
        pending_for: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].pending_for.contains(&String::from("alice")));
}

// ---------------------------------------------------------------------------
// Test 4: Query by author
// ---------------------------------------------------------------------------

#[test]
fn query_by_author() {
    let system = setup_system();
    let filter = QueryFilter {
        author: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].path.to_str().unwrap().contains("done.md"));
}

// ---------------------------------------------------------------------------
// Test 5: Empty directory
// ---------------------------------------------------------------------------

#[test]
fn query_empty_dir() {
    let system = MockSystem::new().with_dir(Path::new("/empty")).unwrap();

    let filter = QueryFilter::default();
    let results = query(&system, Path::new("/empty"), &filter).unwrap();
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// Test 6: Query with --comment-id finds the document containing that comment
// ---------------------------------------------------------------------------

#[test]
fn query_by_comment_id_finds_matching_doc() {
    let system = setup_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].path.to_str().unwrap().contains("pending.md"));
}

// ---------------------------------------------------------------------------
// Test 7: Query with --comment-id in multi-doc folder returns only match
// ---------------------------------------------------------------------------

#[test]
fn query_by_comment_id_returns_only_matching_doc() {
    let system = setup_system();
    // "def" is the ID in done.md.
    let filter = QueryFilter {
        comment_id: Some(String::from("def")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].path.to_str().unwrap().contains("done.md"));
}

// ---------------------------------------------------------------------------
// Test 8: Query with --comment-id combined with --author
// ---------------------------------------------------------------------------

#[test]
fn query_by_comment_id_combined_with_author() {
    let system = setup_system();
    // Comment "abc" is by "eduardo", so author=eduardo should match.
    let filter = QueryFilter {
        author: Some(String::from("eduardo")),
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert_eq!(results.len(), 1);

    // Same comment but author=alice should not match (abc is by eduardo).
    let filter_mismatch = QueryFilter {
        author: Some(String::from("alice")),
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results_mismatch = query(&system, Path::new("/project"), &filter_mismatch).unwrap();
    assert!(results_mismatch.is_empty());
}

// ---------------------------------------------------------------------------
// Test 9: Query with --comment-id combined with --pending
// ---------------------------------------------------------------------------

#[test]
fn query_by_comment_id_combined_with_pending() {
    let system = setup_system();
    // Comment "abc" is pending, so pending=true should match.
    let filter = QueryFilter {
        comment_id: Some(String::from("abc")),
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert_eq!(results.len(), 1);

    // Comment "def" is acked, so pending=true should not match.
    let filter_acked = QueryFilter {
        comment_id: Some(String::from("def")),
        pending: true,
        ..QueryFilter::default()
    };

    let results_acked = query(&system, Path::new("/project"), &filter_acked).unwrap();
    assert!(results_acked.is_empty());
}

// ---------------------------------------------------------------------------
// Test 10: Query with --comment-id that does not exist returns empty results
// ---------------------------------------------------------------------------

#[test]
fn query_by_comment_id_not_found_returns_empty() {
    let system = setup_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("nonexistent")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// Test 11: Query with --comment-id on empty folder returns empty results
// ---------------------------------------------------------------------------

#[test]
fn query_by_comment_id_empty_folder_returns_empty() {
    let system = MockSystem::new().with_dir(Path::new("/empty")).unwrap();
    let filter = QueryFilter {
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/empty"), &filter).unwrap();
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// resolve_comment_id tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test 12: resolve_comment_id finds a single document
// ---------------------------------------------------------------------------

#[test]
fn resolve_comment_id_finds_single_doc() {
    let system = setup_system();
    let matches = resolve_comment_id(&system, Path::new("/project"), "abc").unwrap();
    assert_eq!(matches.len(), 1);
    assert!(matches[0].to_str().unwrap().contains("pending.md"));
}

// ---------------------------------------------------------------------------
// Test 13: resolve_comment_id returns empty for nonexistent ID
// ---------------------------------------------------------------------------

#[test]
fn resolve_comment_id_not_found() {
    let system = setup_system();
    let matches = resolve_comment_id(&system, Path::new("/project"), "nonexistent").unwrap();
    assert!(matches.is_empty());
}

// ---------------------------------------------------------------------------
// Test 14: resolve_comment_id returns multiple docs when ID duplicated
// ---------------------------------------------------------------------------

#[test]
fn resolve_comment_id_ambiguous() {
    // Create two documents with the same comment ID.
    let system = MockSystem::new()
        .with_dir(Path::new("/multi"))
        .unwrap()
        .with_file(Path::new("/multi/a.md"), doc_with_pending().as_bytes())
        .unwrap()
        .with_file(Path::new("/multi/b.md"), doc_with_pending().as_bytes())
        .unwrap();

    let matches = resolve_comment_id(&system, Path::new("/multi"), "abc").unwrap();
    assert_eq!(matches.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 15: resolve_comment_id scopes to subdirectory
// ---------------------------------------------------------------------------

#[test]
fn resolve_comment_id_scopes_to_subdir() {
    let system = setup_system();
    // Searching in /project/docs should find pending.md's comment.
    let matches = resolve_comment_id(&system, Path::new("/project/docs"), "abc").unwrap();
    assert_eq!(matches.len(), 1);

    // Searching at root, there is no abc outside of /project/docs.
    // (plain.md has no comments at all).
    let matches_root = resolve_comment_id(&system, Path::new("/project"), "abc").unwrap();
    assert_eq!(matches_root.len(), 1);
}

// ===========================================================================
// Expanded query tests
// ===========================================================================

/// Document with 3 comments: 2 pending (by different authors, to different recipients),
/// 1 acked.
fn doc_expanded() -> &'static str {
    "\
---
title: Expanded Test
---

```remargin
---
id: c1
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
to: [bob]
checksum: sha256:c1c1
---
First comment from alice.
```

```remargin
---
id: c2
author: bob
type: agent
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:c2c2
---
Second comment from bob.
```

```remargin
---
id: c3
author: alice
type: human
ts: 2026-04-06T14:00:00-04:00
to: [bob]
checksum: sha256:c3c3
ack:
  - bob@2026-04-06T15:00:00-04:00
---
Third comment, already acked.
```
"
}

/// Second document for multi-file tests.
fn doc_expanded_other() -> &'static str {
    "\
---
title: Other Doc
---

```remargin
---
id: d1
author: carol
type: human
ts: 2026-04-07T08:00:00-04:00
to: [alice]
checksum: sha256:d1d1
---
Comment from carol.
```
"
}

fn setup_expanded_system() -> MockSystem {
    MockSystem::new()
        .with_dir(Path::new("/exp"))
        .unwrap()
        .with_file(Path::new("/exp/review.md"), doc_expanded().as_bytes())
        .unwrap()
        .with_file(Path::new("/exp/other.md"), doc_expanded_other().as_bytes())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 16: query_expanded_returns_comments
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_returns_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    // review.md has 3 comments, other.md has 1 comment.
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    assert_eq!(review.comments.len(), 3);
    assert_eq!(review.comments[0].id, "c1");
    assert_eq!(review.comments[0].author, "alice");
    assert_eq!(review.comments[0].content, "First comment from alice.");
    assert_eq!(review.comments[1].id, "c2");
    assert_eq!(review.comments[2].id, "c3");
}

// ---------------------------------------------------------------------------
// Test 17: query_expanded_pending_filters_comments
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_pending_filters_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    // c1 and c2 are pending, c3 is acked.
    assert_eq!(review.comments.len(), 2);
    assert!(review.comments.iter().all(|cm| cm.ack.is_empty()));
    let ids: Vec<&str> = review.comments.iter().map(|cm| cm.id.as_str()).collect();
    assert!(ids.contains(&"c1"));
    assert!(ids.contains(&"c2"));
}

// ---------------------------------------------------------------------------
// Test 18: query_expanded_pending_for_filters_comments
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_pending_for_filters_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending_for: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    // Only c2 is pending and addressed to alice.
    assert_eq!(review.comments.len(), 1);
    assert_eq!(review.comments[0].id, "c2");
    assert!(review.comments[0].to.contains(&String::from("alice")));
}

// ---------------------------------------------------------------------------
// Test 19: query_expanded_author_filters_comments
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_author_filters_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        author: Some(String::from("bob")),
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    let review = &results[0];
    // Only c2 is by bob.
    assert_eq!(review.comments.len(), 1);
    assert_eq!(review.comments[0].id, "c2");
    assert_eq!(review.comments[0].author, "bob");
}

// ---------------------------------------------------------------------------
// Test 20: query_expanded_since_filters_comments
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_since_filters_comments() {
    let system = setup_expanded_system();
    let since = chrono::DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap();
    let filter = QueryFilter {
        expanded: true,
        since: Some(since),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    // Only c3 (14:00) is after 13:00. c1 (10:00) and c2 (12:00) are before.
    assert_eq!(review.comments.len(), 1);
    assert_eq!(review.comments[0].id, "c3");
}

// ---------------------------------------------------------------------------
// Test 21: query_expanded_combined_filters
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_combined_filters() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        author: Some(String::from("alice")),
        expanded: true,
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    // Only c1 is pending AND by alice (c3 is by alice but acked, c2 is pending but by bob).
    assert_eq!(review.comments.len(), 1);
    assert_eq!(review.comments[0].id, "c1");
}

// ---------------------------------------------------------------------------
// Test 22: query_expanded_multiple_files
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_multiple_files() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    // Both files have pending comments.
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(!r.comments.is_empty());
    }
    // other.md has 1 pending comment from carol.
    let other = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("other.md"))
        .unwrap();
    assert_eq!(other.comments.len(), 1);
    assert_eq!(other.comments[0].author, "carol");
}

// ---------------------------------------------------------------------------
// Test 23: query_not_expanded_has_empty_comments
// ---------------------------------------------------------------------------

#[test]
fn query_not_expanded_has_empty_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    // Default (expanded=false) means comments vec is always empty.
    for r in &results {
        assert!(r.comments.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Test 24: query_expanded_no_matching_comments
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_no_matching_comments() {
    let system = setup_expanded_system();
    // Filter for author "nobody" -- no comments match.
    let filter = QueryFilter {
        author: Some(String::from("nobody")),
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    // File-level filter already excludes the file, and with expanded the
    // per-comment filter also finds nothing, so result is empty.
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// Test 25: query_expanded_comment_fields_complete
// ---------------------------------------------------------------------------

#[test]
fn query_expanded_comment_fields_complete() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("c3")),
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].comments.len(), 1);

    let cm = &results[0].comments[0];
    // Verify all fields are populated correctly.
    assert_eq!(cm.id, "c3");
    assert_eq!(cm.author, "alice");
    assert!(matches!(cm.author_type, AuthorType::Human));
    assert_eq!(cm.content, "Third comment, already acked.");
    assert_eq!(cm.ts.to_rfc3339(), "2026-04-06T14:00:00-04:00");
    assert!(cm.line > 0);
    assert_eq!(cm.to, vec![String::from("bob")]);
    assert_eq!(cm.ack.len(), 1);
    assert_eq!(cm.ack[0].author, "bob");
    assert!(cm.reply_to.is_none());
    assert!(cm.thread.is_none());
    assert!(cm.reactions.is_empty());
    assert!(cm.attachments.is_empty());
    assert_eq!(cm.checksum, "sha256:c3c3");
    assert!(cm.signature.is_none());
}
