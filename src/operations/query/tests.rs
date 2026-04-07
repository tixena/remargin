//! Tests for the cross-document query engine.

use std::path::Path;

use os_shim::mock::MockSystem;

use crate::operations::query::{QueryFilter, query, resolve_comment_id};

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
