//! Tests for the cross-document query engine.

use std::path::Path;

use os_shim::mock::MockSystem;

use crate::operations::query::{QueryFilter, query, resolve_comment_id};
use crate::parser::AuthorType;

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

#[test]
fn query_all_with_comments() {
    let system = setup_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/project"), &filter).unwrap();
    // Should find 2 files (pending.md and done.md), not plain.md
    assert_eq!(results.len(), 2);
}

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

#[test]
fn query_empty_dir() {
    let system = MockSystem::new().with_dir(Path::new("/empty")).unwrap();

    let filter = QueryFilter::default();
    let results = query(&system, Path::new("/empty"), &filter).unwrap();
    assert!(results.is_empty());
}

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

#[test]
fn resolve_comment_id_finds_single_doc() {
    let system = setup_system();
    let matches = resolve_comment_id(&system, Path::new("/project"), "abc").unwrap();
    assert_eq!(matches.len(), 1);
    assert!(matches[0].to_str().unwrap().contains("pending.md"));
}

#[test]
fn resolve_comment_id_not_found() {
    let system = setup_system();
    let matches = resolve_comment_id(&system, Path::new("/project"), "nonexistent").unwrap();
    assert!(matches.is_empty());
}

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

#[test]
fn query_summary_has_empty_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        summary: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    // summary=true suppresses comment data.
    for r in &results {
        assert!(r.comments.is_empty());
    }
}

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

// ===========================================================================
// Pending count bug-fix tests (rem-s6f)
// ===========================================================================

/// Document with a broadcast comment (no `to` field) plus a directed comment.
fn doc_broadcast_and_directed() -> &'static str {
    "\
---
title: Mixed
---

```remargin
---
id: bcast
author: bot
type: agent
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:bc1
---
Broadcast -- no to field.
```

```remargin
---
id: dir1
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
to: [eduardo]
checksum: sha256:d1d1
---
Directed to eduardo.
```
"
}

/// Document with a comment addressed to two people, only one of whom acked.
fn doc_partially_acked() -> &'static str {
    "\
---
title: Partial
---

```remargin
---
id: pa1
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
to: [bob, carol]
checksum: sha256:pa1
ack:
  - bob@2026-04-06T11:00:00-04:00
---
Partially acked: bob acked, carol did not.
```
"
}

/// Document with a comment fully acked by all recipients.
fn doc_fully_acked_multi() -> &'static str {
    "\
---
title: Fully Acked
---

```remargin
---
id: fa1
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
to: [bob, carol]
checksum: sha256:fa1
ack:
  - bob@2026-04-06T11:00:00-04:00
  - carol@2026-04-06T12:00:00-04:00
---
Fully acked by both.
```
"
}

fn setup_pending_system() -> MockSystem {
    MockSystem::new()
        .with_dir(Path::new("/pend"))
        .unwrap()
        .with_file(
            Path::new("/pend/mixed.md"),
            doc_broadcast_and_directed().as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/pend/partial.md"),
            doc_partially_acked().as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/pend/full.md"),
            doc_fully_acked_multi().as_bytes(),
        )
        .unwrap()
}

#[test]
fn no_to_not_pending() {
    let system = setup_pending_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/pend"), &filter).unwrap();
    let mixed_result = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("mixed.md"))
        .unwrap();

    // mixed.md should appear because dir1 is pending, but pending_count
    // should be 1 (only dir1), NOT 2 (bcast should not count).
    assert_eq!(mixed_result.pending_count, 1);
}

#[test]
fn to_with_no_ack_is_pending() {
    let system = setup_pending_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/pend"), &filter).unwrap();
    let mixed = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("mixed.md"))
        .unwrap();

    assert_eq!(mixed.pending_count, 1);
    assert!(mixed.pending_for.contains(&String::from("eduardo")));
}

#[test]
fn to_fully_acked_not_pending() {
    let system = setup_pending_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/pend"), &filter).unwrap();
    // full.md has a fully-acked comment so it should NOT appear.
    assert!(
        !results
            .iter()
            .any(|r| r.path.to_str().unwrap().contains("full.md")),
        "fully-acked document should not appear in pending results"
    );
}

#[test]
fn to_partially_acked_still_pending() {
    let system = setup_pending_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/pend"), &filter).unwrap();
    let partial = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("partial.md"))
        .unwrap();

    assert_eq!(partial.pending_count, 1);
}

#[test]
fn pending_count_matches_expanded() {
    let system = setup_pending_system();
    let filter = QueryFilter {
        expanded: true,
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/pend"), &filter).unwrap();
    for r in &results {
        assert_eq!(
            r.pending_count,
            u32::try_from(r.comments.len()).unwrap(),
            "pending_count should equal expanded comments length for {}",
            r.path.display()
        );
    }
}

#[test]
fn pending_for_excludes_fully_acked() {
    let system = setup_pending_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/pend"), &filter).unwrap();
    let partial = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("partial.md"))
        .unwrap();

    // bob acked, carol did not. pending_for should contain carol but not bob.
    assert!(
        partial.pending_for.contains(&String::from("carol")),
        "carol should be in pending_for"
    );
    assert!(
        !partial.pending_for.contains(&String::from("bob")),
        "bob should NOT be in pending_for (already acked)"
    );
}

#[test]
fn broadcast_comment_never_pending() {
    // A single-file system with only a broadcast comment.
    let broadcast_only = "\
---
title: Broadcast Only
---

```remargin
---
id: b1
author: bot
type: agent
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:b1b1
---
No to field at all.
```
";
    let system = MockSystem::new()
        .with_dir(Path::new("/bonly"))
        .unwrap()
        .with_file(Path::new("/bonly/note.md"), broadcast_only.as_bytes())
        .unwrap();

    let filter = QueryFilter::default();
    let results = query(&system, Path::new("/bonly"), &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].pending_count, 0);
    assert!(results[0].pending_for.is_empty());

    // With --pending filter, document should not appear.
    let pending_filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };
    let pending_results = query(&system, Path::new("/bonly"), &pending_filter).unwrap();
    assert!(
        pending_results.is_empty(),
        "broadcast-only doc should not appear in --pending results"
    );
}

#[test]
fn pending_for_partially_acked() {
    let system = setup_pending_system();

    // carol has not acked -- should find partial.md
    let filter_carol = QueryFilter {
        pending_for: Some(String::from("carol")),
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/pend"), &filter_carol).unwrap();
    assert!(
        results
            .iter()
            .any(|r| r.path.to_str().unwrap().contains("partial.md")),
        "partial.md should appear for pending_for=carol"
    );

    // bob already acked -- should NOT find partial.md
    let filter_bob = QueryFilter {
        pending_for: Some(String::from("bob")),
        ..QueryFilter::default()
    };
    let results_bob = query(&system, Path::new("/pend"), &filter_bob).unwrap();
    assert!(
        !results_bob
            .iter()
            .any(|r| r.path.to_str().unwrap().contains("partial.md")),
        "partial.md should NOT appear for pending_for=bob (already acked)"
    );
}

#[test]
fn expanded_pending_for_partial_ack() {
    let system = setup_pending_system();
    let filter = QueryFilter {
        expanded: true,
        pending_for: Some(String::from("carol")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/pend"), &filter).unwrap();
    let partial = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("partial.md"))
        .unwrap();

    assert_eq!(partial.comments.len(), 1);
    assert_eq!(partial.comments[0].id, "pa1");
}

// ===========================================================================
// Default expanded + file path tests (rem-frc)
// ===========================================================================

#[test]
fn query_default_includes_comments() {
    let system = setup_expanded_system();
    // Default filter: no explicit expanded=true, no summary.
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    // Comments should be included by default (not empty).
    for r in &results {
        assert!(
            !r.comments.is_empty(),
            "default query should include comments for {}",
            r.path.display()
        );
    }
}

#[test]
fn expanded_comments_have_file_path() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    for r in &results {
        for cm in &r.comments {
            assert_eq!(
                cm.file, r.path,
                "comment {}'s file field should match parent result path",
                cm.id
            );
        }
    }
}

#[test]
fn query_summary_only() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        summary: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    // summary should still return results (with counts).
    assert!(!results.is_empty());
    for r in &results {
        assert!(
            r.comments.is_empty(),
            "summary mode should suppress comments for {}",
            r.path.display()
        );
        assert!(r.comment_count > 0, "should still have counts");
    }
}

#[test]
fn backward_compat_expanded_flag() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    for r in &results {
        assert!(
            !r.comments.is_empty(),
            "--expanded should include comments for {}",
            r.path.display()
        );
    }
}

#[test]
fn file_path_on_default_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();

    for cm in &review.comments {
        assert!(
            cm.file.to_str().unwrap().contains("review.md"),
            "comment {} file should be review.md, got {}",
            cm.id,
            cm.file.display()
        );
    }
}

#[test]
fn summary_with_pending_filter() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        pending: true,
        summary: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    for r in &results {
        assert!(r.comments.is_empty(), "summary suppresses comments");
        assert!(r.pending_count > 0, "pending filter still applies");
    }
}

#[test]
fn expanded_overrides_summary() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        summary: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    for r in &results {
        assert!(
            !r.comments.is_empty(),
            "expanded=true should override summary for {}",
            r.path.display()
        );
    }
}

#[test]
fn query_result_json_shape_matches_schema() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/exp"), &filter).unwrap();
    let first = results.first().unwrap();

    // Serialize the whole result via serde (this is what the CLI's
    // `--json query` output relies on after rem-w0b).
    let value = serde_json::to_value(first).unwrap();
    let obj = value.as_object().unwrap();

    // Required QueryResult keys.
    for key in [
        "comment_count",
        "comments",
        "path",
        "pending_count",
        "pending_for",
    ] {
        assert!(
            obj.contains_key(key),
            "required key `{key}` missing from serialized QueryResult"
        );
    }

    // `path` must be a plain string (PathBuf), not some JSON object.
    assert!(obj["path"].is_string());

    // `pending_for` must always be present as an array, even when empty.
    assert!(obj["pending_for"].is_array());

    // Drill into the first embedded ExpandedComment.
    let comments = obj["comments"].as_array().unwrap();
    let comment = comments.first().unwrap().as_object().unwrap();

    for key in [
        "ack",
        "attachments",
        "author",
        "author_type",
        "checksum",
        "content",
        "file",
        "id",
        "line",
        "reactions",
        "to",
        "ts",
    ] {
        assert!(
            comment.contains_key(key),
            "required key `{key}` missing from serialized ExpandedComment"
        );
    }

    // Schema uses `author_type` with lowercase enum values, not `type`.
    assert!(
        !comment.contains_key("type"),
        "legacy `type` key must not appear in serialized ExpandedComment"
    );
    let author_type = comment["author_type"].as_str().unwrap();
    assert!(
        matches!(author_type, "human" | "agent"),
        "author_type must be lowercase, got {author_type:?}"
    );

    // `file` must render as a string path (what Zod `z.string()` expects),
    // not as a `{ path: ... }` object or similar.
    assert!(comment["file"].is_string());
}

#[test]
fn expanded_comment_skips_none_options_in_json() {
    // Feed a file with a minimal comment (no reply_to/thread/signature)
    // and make sure those fields are omitted from the JSON so the Zod
    // `strictObject` schema treats them as `undefined`.
    let system = MockSystem::new()
        .with_dir(Path::new("/mini"))
        .unwrap()
        .with_file(
            Path::new("/mini/mini.md"),
            b"\
```remargin
---
id: mini
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:mini
---
Minimal.
```
",
        )
        .unwrap();

    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/mini"), &filter).unwrap();
    let first = results.first().unwrap();

    let value = serde_json::to_value(first).unwrap();
    let comment = value["comments"][0].as_object().unwrap();

    for key in ["reply_to", "thread", "signature"] {
        assert!(
            !comment.contains_key(key),
            "optional key `{key}` should be skipped when None"
        );
    }

    // But required collections must still be present (as empty).
    assert_eq!(comment["ack"], serde_json::json!([]));
    assert_eq!(comment["attachments"], serde_json::json!([]));
    assert_eq!(comment["to"], serde_json::json!([]));
    assert_eq!(comment["reactions"], serde_json::json!({}));
}
