//! Tests for the cross-document query engine.

use std::path::Path;

use os_shim::mock::MockSystem;

use crate::operations::query::{QueryFilter, query};

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
