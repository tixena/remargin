//! Tests for the cross-document query engine.

use std::path::{Path, PathBuf};

use os_shim::mock::MemorySystem;

use crate::config::{Mode, ResolvedConfig};
use crate::operations::query::{
    QueryFilter, query, render_query_plain, resolve_comment_id, to_compact_row,
};
use crate::parser::AuthorType;

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

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

fn setup_system() -> MemorySystem {
    MemorySystem::new()
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

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn query_pending_only() {
    let system = setup_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
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

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .pending_for
            .as_ref()
            .is_some_and(|v| v.contains(&String::from("alice")))
    );
}

#[test]
fn query_by_author() {
    let system = setup_system();
    let filter = QueryFilter {
        author: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].path.to_str().unwrap().contains("done.md"));
}

#[test]
fn query_empty_dir() {
    let system = MemorySystem::new().with_dir(Path::new("/empty")).unwrap();

    let filter = QueryFilter::default();
    let results = query(&system, Path::new("/empty"), &filter, &open_config()).unwrap();
    assert!(results.is_empty());
}

#[test]
fn query_by_comment_id_finds_matching_doc() {
    let system = setup_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].path.to_str().unwrap().contains("pending.md"));
}

#[test]
fn query_by_comment_id_returns_only_matching_doc() {
    let system = setup_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("def")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].path.to_str().unwrap().contains("done.md"));
}

#[test]
fn query_by_comment_id_combined_with_author() {
    let system = setup_system();
    let filter = QueryFilter {
        author: Some(String::from("eduardo")),
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);

    let filter_mismatch = QueryFilter {
        author: Some(String::from("alice")),
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results_mismatch = query(
        &system,
        Path::new("/project"),
        &filter_mismatch,
        &open_config(),
    )
    .unwrap();
    assert!(results_mismatch.is_empty());
}

#[test]
fn query_by_comment_id_combined_with_pending() {
    let system = setup_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("abc")),
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);

    let filter_acked = QueryFilter {
        comment_id: Some(String::from("def")),
        pending: true,
        ..QueryFilter::default()
    };

    let results_acked = query(
        &system,
        Path::new("/project"),
        &filter_acked,
        &open_config(),
    )
    .unwrap();
    assert!(results_acked.is_empty());
}

#[test]
fn query_by_comment_id_not_found_returns_empty() {
    let system = setup_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("nonexistent")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    assert!(results.is_empty());
}

#[test]
fn query_by_comment_id_empty_folder_returns_empty() {
    let system = MemorySystem::new().with_dir(Path::new("/empty")).unwrap();
    let filter = QueryFilter {
        comment_id: Some(String::from("abc")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/empty"), &filter, &open_config()).unwrap();
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
    assert_eq!(matches, [] as [PathBuf; 0]);
}

#[test]
fn resolve_comment_id_ambiguous() {
    let system = MemorySystem::new()
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
    let matches = resolve_comment_id(&system, Path::new("/project/docs"), "abc").unwrap();
    assert_eq!(matches.len(), 1);

    let matches_root = resolve_comment_id(&system, Path::new("/project"), "abc").unwrap();
    assert_eq!(matches_root.len(), 1);
}

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

fn setup_expanded_system() -> MemorySystem {
    MemorySystem::new()
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    assert_eq!(review.comments.as_ref().unwrap().len(), 3);
    assert_eq!(review.comments.as_ref().unwrap()[0].id, "c1");
    assert_eq!(review.comments.as_ref().unwrap()[0].author, "alice");
    assert_eq!(
        review.comments.as_ref().unwrap()[0].content,
        "First comment from alice."
    );
    assert_eq!(review.comments.as_ref().unwrap()[1].id, "c2");
    assert_eq!(review.comments.as_ref().unwrap()[2].id, "c3");
}

#[test]
fn query_expanded_pending_filters_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    assert_eq!(review.comments.as_ref().unwrap().len(), 2);
    assert!(
        review
            .comments
            .as_ref()
            .unwrap()
            .iter()
            .all(|cm| cm.ack.is_empty())
    );
    let ids: Vec<&str> = review
        .comments
        .as_ref()
        .unwrap()
        .iter()
        .map(|cm| cm.id.as_str())
        .collect();
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    assert_eq!(review.comments.as_ref().unwrap().len(), 1);
    assert_eq!(review.comments.as_ref().unwrap()[0].id, "c2");
    assert!(
        review.comments.as_ref().unwrap()[0]
            .to
            .contains(&String::from("alice"))
    );
}

#[test]
fn query_expanded_author_filters_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        author: Some(String::from("bob")),
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    let review = &results[0];
    assert_eq!(review.comments.as_ref().unwrap().len(), 1);
    assert_eq!(review.comments.as_ref().unwrap()[0].id, "c2");
    assert_eq!(review.comments.as_ref().unwrap()[0].author, "bob");
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    assert_eq!(review.comments.as_ref().unwrap().len(), 1);
    assert_eq!(review.comments.as_ref().unwrap()[0].id, "c3");
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();
    assert_eq!(review.comments.as_ref().unwrap().len(), 1);
    assert_eq!(review.comments.as_ref().unwrap()[0].id, "c1");
}

#[test]
fn query_expanded_multiple_files() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(r.comments.as_ref().is_some_and(|v| !v.is_empty()));
    }
    let other = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("other.md"))
        .unwrap();
    assert_eq!(other.comments.as_ref().unwrap().len(), 1);
    assert_eq!(other.comments.as_ref().unwrap()[0].author, "carol");
}

#[test]
fn query_summary_has_empty_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        summary: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    for r in &results {
        assert!(r.comments.as_ref().is_none_or(Vec::is_empty));
    }
}

#[test]
fn query_expanded_no_matching_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        author: Some(String::from("nobody")),
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].comments.as_ref().unwrap().len(), 1);

    let cm = &results[0].comments.as_ref().unwrap()[0];
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
    assert_eq!(cm.attachments, [] as [String; 0]);
    assert_eq!(cm.checksum, "sha256:c3c3");
    assert!(cm.signature.is_none());
}

/// The serde-serialized `ts` and the hand-formatted ack both spell a zero offset as `Z`.
#[test]
fn compact_row_ts_and_ack_agree_on_the_z_spelling() {
    let doc = "\
---
title: Mixed
---

```remargin
---
id: zed
author: alice
type: human
ts: 2026-04-06T12:00:00+00:00
to: [bob]
ack:
  - bob@2026-04-06T13:00:00+00:00
checksum: sha256:zzz
---
Please review.
```
";
    let system = MemorySystem::new()
        .with_dir(Path::new("/z"))
        .unwrap()
        .with_file(Path::new("/z/a.md"), doc.as_bytes())
        .unwrap();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/z"), &filter, &open_config()).unwrap();
    let comments = results[0].comments.as_ref().unwrap();
    let row = to_compact_row(&comments[0], false);

    assert_eq!(row[4], serde_json::json!("2026-04-06T12:00:00Z"));
    assert_eq!(row[8], serde_json::json!(["bob@2026-04-06T13:00:00Z"]));
}

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

fn setup_pending_system() -> MemorySystem {
    MemorySystem::new()
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
fn broadcast_counts_as_pending() {
    let system = setup_pending_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/pend"), &filter, &open_config()).unwrap();
    let mixed_result = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("mixed.md"))
        .unwrap();

    assert_eq!(mixed_result.pending_count, 2);
}

#[test]
fn to_with_no_ack_is_pending() {
    let system = setup_pending_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/pend"), &filter, &open_config()).unwrap();
    let mixed = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("mixed.md"))
        .unwrap();

    assert_eq!(mixed.pending_count, 2);
    assert!(
        mixed
            .pending_for
            .as_ref()
            .is_some_and(|v| v.contains(&String::from("eduardo")))
    );
}

#[test]
fn to_fully_acked_not_pending() {
    let system = setup_pending_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/pend"), &filter, &open_config()).unwrap();
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

    let results = query(&system, Path::new("/pend"), &filter, &open_config()).unwrap();
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

    let results = query(&system, Path::new("/pend"), &filter, &open_config()).unwrap();
    for r in &results {
        assert_eq!(
            r.pending_count,
            u32::try_from(r.comments.as_ref().map_or(0, Vec::len)).unwrap(),
            "pending_count should equal expanded comments length for {}",
            r.path.display()
        );
    }
}

#[test]
fn pending_for_excludes_fully_acked() {
    let system = setup_pending_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/pend"), &filter, &open_config()).unwrap();
    let partial = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("partial.md"))
        .unwrap();

    assert!(
        partial
            .pending_for
            .as_ref()
            .is_some_and(|v| v.contains(&String::from("carol"))),
        "carol should be in pending_for"
    );
    assert!(
        partial
            .pending_for
            .as_ref()
            .is_none_or(|v| !v.contains(&String::from("bob"))),
        "bob should NOT be in pending_for (already acked)"
    );
}

#[test]
fn unacked_broadcast_counts_as_pending() {
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
    let system = MemorySystem::new()
        .with_dir(Path::new("/bonly"))
        .unwrap()
        .with_file(Path::new("/bonly/note.md"), broadcast_only.as_bytes())
        .unwrap();

    let filter = QueryFilter::default();
    let results = query(&system, Path::new("/bonly"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].pending_count, 1);
    assert!(results[0].pending_for.as_ref().is_none_or(Vec::is_empty));

    let pending_filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };
    let pending_results = query(
        &system,
        Path::new("/bonly"),
        &pending_filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(
        pending_results.len(),
        1,
        "fresh broadcast must surface under --pending"
    );
}

#[test]
fn acked_broadcast_not_pending() {
    let acked_broadcast = "\
---
title: Acked Broadcast
---

```remargin
---
id: b2
author: bot
type: agent
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:b2b2
ack:
  - alice@2026-04-06T10:00:00-04:00
---
Broadcast, already closed by an ack.
```
";
    let system = MemorySystem::new()
        .with_dir(Path::new("/bclosed"))
        .unwrap()
        .with_file(Path::new("/bclosed/note.md"), acked_broadcast.as_bytes())
        .unwrap();

    let pending_filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/bclosed"),
        &pending_filter,
        &open_config(),
    )
    .unwrap();
    assert!(
        results.is_empty(),
        "acked broadcast should not surface under --pending"
    );
}

#[test]
fn pending_for_partially_acked() {
    let system = setup_pending_system();

    let filter_carol = QueryFilter {
        pending_for: Some(String::from("carol")),
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/pend"), &filter_carol, &open_config()).unwrap();
    assert!(
        results
            .iter()
            .any(|r| r.path.to_str().unwrap().contains("partial.md")),
        "partial.md should appear for pending_for=carol"
    );

    let filter_bob = QueryFilter {
        pending_for: Some(String::from("bob")),
        ..QueryFilter::default()
    };
    let results_bob = query(&system, Path::new("/pend"), &filter_bob, &open_config()).unwrap();
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

    let results = query(&system, Path::new("/pend"), &filter, &open_config()).unwrap();
    let partial = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("partial.md"))
        .unwrap();

    assert_eq!(partial.comments.as_ref().unwrap().len(), 1);
    assert_eq!(partial.comments.as_ref().unwrap()[0].id, "pa1");
}

#[test]
fn query_default_includes_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    for r in &results {
        assert!(
            r.comments.as_ref().is_some_and(|v| !v.is_empty()),
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    for r in &results {
        for cm in r.comments.iter().flatten() {
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert!(!results.is_empty());
    for r in &results {
        assert!(
            r.comments.as_ref().is_none_or(Vec::is_empty),
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    for r in &results {
        assert!(
            r.comments.as_ref().is_some_and(|v| !v.is_empty()),
            "--expanded should include comments for {}",
            r.path.display()
        );
    }
}

#[test]
fn file_path_on_default_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter::default();

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let review = results
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("review.md"))
        .unwrap();

    for cm in review.comments.iter().flatten() {
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    for r in &results {
        assert!(
            r.comments.as_ref().is_none_or(Vec::is_empty),
            "summary suppresses comments"
        );
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

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    for r in &results {
        assert!(
            r.comments.as_ref().is_some_and(|v| !v.is_empty()),
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
    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let first = results.first().unwrap();

    let value = serde_json::to_value(first).unwrap();
    let obj = value.as_object().unwrap();

    for key in [
        "comment_count",
        "comments",
        "matched_count",
        "path",
        "pending_count",
        "pending_for",
    ] {
        assert!(
            obj.contains_key(key),
            "required key `{key}` missing from serialized QueryResult"
        );
    }

    assert!(obj["path"].is_string());

    assert!(obj["pending_for"].is_array());

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

    assert!(
        !comment.contains_key("type"),
        "legacy `type` key must not appear in serialized ExpandedComment"
    );
    let author_type = comment["author_type"].as_str().unwrap();
    assert!(
        matches!(author_type, "human" | "agent"),
        "author_type must be lowercase, got {author_type:?}"
    );

    assert!(comment["file"].is_string());
}

#[test]
fn expanded_comment_skips_none_options_in_json() {
    let system = MemorySystem::new()
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
    let results = query(&system, Path::new("/mini"), &filter, &open_config()).unwrap();
    let first = results.first().unwrap();

    let value = serde_json::to_value(first).unwrap();
    let comment = value["comments"][0].as_object().unwrap();

    for key in ["reply_to", "thread", "signature"] {
        assert!(
            !comment.contains_key(key),
            "optional key `{key}` should be skipped when None"
        );
    }

    assert_eq!(comment["ack"], serde_json::json!([]));
    assert_eq!(comment["attachments"], serde_json::json!([]));
    assert_eq!(comment["to"], serde_json::json!([]));
    assert_eq!(comment["reactions"], serde_json::json!({}));
}

#[test]
fn content_regex_filters_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter::default()
        .with_content_regex("alice", false)
        .unwrap();

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let all_comments: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    assert_eq!(all_comments, vec!["c1"]);
}

#[test]
fn content_regex_composes_with_pending() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
            .with_content_regex("comment", false)
            .unwrap()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    let ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    assert!(ids.contains(&"c1"));
    assert!(ids.contains(&"c2"));
    assert!(!ids.contains(&"c3"), "acked comment must be excluded");
    assert!(!ids.contains(&"d1"), "capital C must not match lowercase");
}

#[test]
fn content_regex_ignore_case_matches_diacritic_class() {
    // `\u{c9}` (capital E-acute) is spelled as an escape: the lint config forbids non-ASCII
    // literals.
    let doc = "\
---
title: Cafe Doc
---

```remargin
---
id: m1
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
to: [bob]
checksum: sha256:m1
---
Visited CAF\u{c9} today.
```

```remargin
---
id: m2
author: alice
type: human
ts: 2026-04-06T11:00:00-04:00
to: [bob]
checksum: sha256:m2
---
Nothing match-worthy here.
```
";
    let system = MemorySystem::new()
        .with_dir(Path::new("/d"))
        .unwrap()
        .with_file(Path::new("/d/x.md"), doc.as_bytes())
        .unwrap();

    let pattern = "c[aA\u{e0}\u{c0}\u{e1}\u{c1}\u{e2}\u{c2}\u{e3}\u{c3}\u{e4}\u{c4}]f[eE\u{e8}\u{c8}\u{e9}\u{c9}\u{ea}\u{ca}\u{eb}\u{cb}]";
    let filter = QueryFilter::default()
        .with_content_regex(pattern, true)
        .unwrap();

    let results = query(&system, Path::new("/d"), &filter, &open_config()).unwrap();
    let ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    assert_eq!(ids, vec!["m1"]);
}

#[test]
fn content_regex_invalid_pattern_errors() {
    let result = QueryFilter::default().with_content_regex("[unclosed", false);
    assert!(result.is_err(), "invalid regex must return Err, not panic");
}

#[test]
fn content_regex_no_match_yields_empty_results() {
    let system = setup_expanded_system();
    let filter = QueryFilter::default()
        .with_content_regex("xyzzy-no-such-token", false)
        .unwrap();

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert!(results.is_empty());
}

/// Fixture covering the four pending shapes simultaneously:
/// - `brd_open`: broadcast, no acks (pending under --pending and --pending-broadcast for anyone).
/// - `brd_mine`: broadcast, alice acked (pending for others, NOT for alice).
/// - `dir_alice`: directed to alice, unacked (pending-for-me when me=alice).
/// - `dir_bob`: directed to bob, unacked (pending-for-me when me=bob; NOT for alice).
/// - `dir_closed`: directed to alice, acked by alice (not pending at all).
fn doc_four_shapes() -> &'static str {
    "\
---
title: Four Shapes
---

```remargin
---
id: brd_open
author: bot
type: agent
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:b0
---
Fresh broadcast, zero acks.
```

```remargin
---
id: brd_mine
author: bot
type: agent
ts: 2026-04-06T09:30:00-04:00
checksum: sha256:b1
ack:
  - alice@2026-04-06T10:00:00-04:00
---
Broadcast acked by alice.
```

```remargin
---
id: dir_alice
author: bob
type: human
ts: 2026-04-06T10:00:00-04:00
to: [alice]
checksum: sha256:da
---
Directed to alice, no ack.
```

```remargin
---
id: dir_bob
author: alice
type: human
ts: 2026-04-06T10:30:00-04:00
to: [bob]
checksum: sha256:db
---
Directed to bob, no ack.
```

```remargin
---
id: dir_closed
author: bob
type: human
ts: 2026-04-06T11:00:00-04:00
to: [alice]
checksum: sha256:dc
ack:
  - alice@2026-04-06T12:00:00-04:00
---
Directed to alice, acked.
```
"
}

fn setup_four_shapes_system() -> MemorySystem {
    MemorySystem::new()
        .with_dir(Path::new("/four"))
        .unwrap()
        .with_file(Path::new("/four/shapes.md"), doc_four_shapes().as_bytes())
        .unwrap()
}

#[test]
fn pending_for_me_surfaces_only_directed_unacked_by_caller() {
    let system = setup_four_shapes_system();
    let filter = QueryFilter {
        expanded: true,
        pending_for_me: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/four"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    let ids: Vec<&str> = results[0]
        .comments
        .iter()
        .flatten()
        .map(|cm| cm.id.as_str())
        .collect();
    assert_eq!(ids, vec!["dir_alice"]);
}

#[test]
fn pending_for_me_matches_pending_for() {
    let system = setup_four_shapes_system();
    let me = String::from("alice");

    let pending_for_me = QueryFilter {
        expanded: true,
        pending_for_me: Some(me.clone()),
        ..QueryFilter::default()
    };
    let pending_for = QueryFilter {
        expanded: true,
        pending_for: Some(me),
        ..QueryFilter::default()
    };

    let me_results = query(&system, Path::new("/four"), &pending_for_me, &open_config()).unwrap();
    let for_results = query(&system, Path::new("/four"), &pending_for, &open_config()).unwrap();

    let me_ids: Vec<&str> = me_results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    let for_ids: Vec<&str> = for_results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();

    assert_eq!(me_ids, for_ids);
}

#[test]
fn pending_broadcast_only_surfaces_unacked_broadcasts() {
    let system = setup_four_shapes_system();
    let filter = QueryFilter {
        expanded: true,
        pending_broadcast: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/four"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    let ids: Vec<&str> = results[0]
        .comments
        .iter()
        .flatten()
        .map(|cm| cm.id.as_str())
        .collect();
    assert_eq!(ids, vec!["brd_open"]);
}

#[test]
fn pending_broadcast_excludes_directed_even_unacked() {
    let system = setup_four_shapes_system();
    let filter = QueryFilter {
        expanded: true,
        pending_broadcast: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/four"), &filter, &open_config()).unwrap();
    let ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    assert!(
        !ids.contains(&"dir_alice"),
        "directed comment must not surface under pending_broadcast"
    );
    assert!(!ids.contains(&"dir_bob"));
    assert!(!ids.contains(&"dir_closed"));
}

#[test]
fn pending_for_me_and_pending_broadcast_union() {
    let system = setup_four_shapes_system();
    let filter = QueryFilter {
        expanded: true,
        pending_broadcast: Some(String::from("alice")),
        pending_for_me: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/four"), &filter, &open_config()).unwrap();
    let mut ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["brd_open", "dir_alice"]);
}

#[test]
fn pending_broadcast_respects_callers_ack() {
    let system = setup_four_shapes_system();
    let filter = QueryFilter {
        expanded: true,
        pending_broadcast: Some(String::from("bob")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/four"), &filter, &open_config()).unwrap();
    let mut ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["brd_mine", "brd_open"]);
}

#[test]
fn pending_union_composes_with_author_filter() {
    let system = setup_four_shapes_system();
    let filter = QueryFilter {
        author: Some(String::from("bob")),
        expanded: true,
        pending_for_me: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/four"), &filter, &open_config()).unwrap();
    let ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    assert_eq!(ids, vec!["dir_alice"]);
}

/// Build a document that carries two remargin blocks with distinct
/// `remargin_kind` lists so the OR-semantics filter can be exercised
/// without any other predicate firing.
fn kind_doc() -> &'static str {
    "\
---
title: Kind Filter
---

```remargin
---
id: q1
author: alice
type: human
ts: 2026-04-10T12:00:00-04:00
checksum: sha256:ce6efcb37b2a6f75f1fd85be6ffdd2bbfcc2e8ce4bd56e37f34e7e9a8e12cd19
remargin_kind: [question]
---
what do you think
```

```remargin
---
id: t1
author: bob
type: human
ts: 2026-04-10T13:00:00-04:00
checksum: sha256:4c7081d5e6c36cf7dbc8cce15d24478b7cafcf2af7dfa2d32ccec3bba5ae0da7
remargin_kind: [todo, action item]
---
follow up with design
```

```remargin
---
id: u1
author: alice
type: human
ts: 2026-04-10T14:00:00-04:00
checksum: sha256:c3b6fb0c9a83f83f3b4b8a2a8e0b1c2f2b8c92ea52a7fa0de2ed7f5f2d7c8a7f
---
no tags at all
```
"
}

fn kind_system() -> MemorySystem {
    MemorySystem::new()
        .with_dir(Path::new("/kinds"))
        .unwrap()
        .with_file(Path::new("/kinds/doc.md"), kind_doc().as_bytes())
        .unwrap()
}

#[test]
fn query_kind_filter_single_value() {
    let system = kind_system();
    let filter = QueryFilter {
        expanded: true,
        remargin_kind: vec![String::from("question")],
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/kinds"), &filter, &open_config()).unwrap();
    let ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    assert_eq!(ids, vec!["q1"]);
}

#[test]
fn query_kind_filter_uses_or_semantics() {
    let system = kind_system();
    let filter = QueryFilter {
        expanded: true,
        remargin_kind: vec![String::from("question"), String::from("todo")],
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/kinds"), &filter, &open_config()).unwrap();
    let mut ids: Vec<&str> = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten().map(|cm| cm.id.as_str()))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["q1", "t1"]);
}

#[test]
fn query_kind_filter_empty_returns_everything() {
    let system = kind_system();
    let filter = QueryFilter {
        expanded: true,
        remargin_kind: Vec::new(),
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/kinds"), &filter, &open_config()).unwrap();
    let count = results
        .iter()
        .flat_map(|r| r.comments.iter().flatten())
        .count();
    assert_eq!(count, 3, "empty filter should return every comment");
}

#[test]
fn query_kind_filter_excludes_unmatched_comments() {
    let system = kind_system();
    let filter = QueryFilter {
        expanded: true,
        remargin_kind: vec![String::from("blocker")],
        ..QueryFilter::default()
    };
    let results = query(&system, Path::new("/kinds"), &filter, &open_config()).unwrap();
    assert!(results.is_empty());
}

#[test]
fn query_file_path_pending_returns_one_result() {
    let system = setup_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(
        &system,
        Path::new("/project/docs/pending.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].pending_count, 1);
    assert_eq!(results[0].path.to_str().unwrap(), "pending.md");
}

#[test]
fn query_file_path_all_acked_is_empty() {
    let system = setup_system();
    let filter = QueryFilter {
        pending: true,
        ..QueryFilter::default()
    };

    let results = query(
        &system,
        Path::new("/project/docs/done.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert!(results.is_empty());
}

#[test]
fn query_file_path_matches_directory_scoped_result() {
    let system = setup_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let file = query(
        &system,
        Path::new("/project/docs/pending.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    let dir = query(&system, Path::new("/project"), &filter, &open_config()).unwrap();
    let dir_pending = dir
        .iter()
        .find(|r| r.path.to_str().unwrap().contains("pending.md"))
        .unwrap();

    let file_ids: Vec<&str> = file[0]
        .comments
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    let dir_ids: Vec<&str> = dir_pending
        .comments
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(file_ids, dir_ids);
}

#[test]
fn query_file_path_expanded_returns_all_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    let comments = results[0].comments.as_ref().unwrap();
    let ids: Vec<&str> = comments.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["c1", "c2", "c3"]);
}

#[test]
fn query_file_path_author_filter() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        author: Some(String::from("bob")),
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    let comments = results[0].comments.as_ref().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "c2");
}

#[test]
fn query_file_path_no_comments_is_empty() {
    let system = setup_system();
    let filter = QueryFilter::default();

    let results = query(
        &system,
        Path::new("/project/plain.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert!(results.is_empty());
}

#[test]
fn query_file_path_comment_id_filter() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        comment_id: Some(String::from("c3")),
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    let comments = results[0].comments.as_ref().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "c3");
}

#[test]
fn query_file_path_relative_is_file_name() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(results[0].path.to_str().unwrap(), "review.md");
    for cm in results[0].comments.as_ref().unwrap() {
        assert_eq!(cm.file.to_str().unwrap(), "review.md");
    }
}

/// Rows are 14 positional columns with no `file`; acks collapse to `author@ts`.
#[test]
fn compact_row_shape_drops_file_and_compacts_acks() {
    use crate::operations::query::{COMMENT_COLS, to_compact_result};

    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(results.len(), 1);

    let compact = to_compact_result(&results[0], false);
    assert_eq!(compact["path"].as_str().unwrap(), "review.md");
    assert_eq!(compact["comment_count"].as_u64().unwrap(), 3);

    assert!(!COMMENT_COLS.contains(&"file"));
    let rows = compact["comments"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        assert_eq!(row.as_array().unwrap().len(), COMMENT_COLS.len());
    }

    let c3 = rows[2].as_array().unwrap();
    assert_eq!(c3[0].as_str().unwrap(), "c3");
    let ack = c3[8].as_array().unwrap();
    assert_eq!(ack.len(), 1);
    assert_eq!(ack[0].as_str().unwrap(), "bob@2026-04-06T15:00:00-04:00");
    assert!(c3[5].is_null(), "reply_to null: {c3:?}");
    assert!(c3[11].is_null(), "edited_at null: {c3:?}");
}

/// `checksum` and `signature` land immediately before the last column, `content`.
#[test]
fn compact_row_include_integrity_adds_columns() {
    use crate::operations::query::{COMMENT_COLS, COMMENT_COLS_INTEGRITY, to_compact_result};

    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();

    let base = to_compact_result(&results[0], false);
    let integrity = to_compact_result(&results[0], true);

    let base_row = base["comments"][0].as_array().unwrap();
    let integrity_row = integrity["comments"][0].as_array().unwrap();
    assert_eq!(base_row.len(), COMMENT_COLS.len());
    assert_eq!(integrity_row.len(), COMMENT_COLS_INTEGRITY.len());
    assert_eq!(integrity_row[13].as_str().unwrap(), "sha256:c1c1");
    assert!(integrity_row[14].is_null(), "unsigned signature null");
    assert_eq!(
        integrity_row[15],
        base_row[COMMENT_COLS.len() - 1],
        "content stays last"
    );
}

/// The row alias renders its `Option` tuple columns as nullable in TypeScript and Zod.
#[test]
fn compact_comment_row_schema_renders_nullable_columns() {
    use crate::operations::query::{compact_comment_row_schema, compact_query_result_schema};

    let row_ts = compact_comment_row_schema::Schema::ts_definition();
    assert!(
        row_ts.contains("string | null"),
        "TS nullable columns: {row_ts}"
    );
    let row_zod = compact_comment_row_schema::Schema::zod_schema();
    assert!(
        row_zod.contains("z.nullable(z.string())"),
        "Zod nullable columns: {row_zod}"
    );

    let payload_ts = compact_query_result_schema::Schema::ts_definition();
    assert!(
        payload_ts.contains("Array<CompactCommentRow>"),
        "payload references row type: {payload_ts}"
    );
    let payload_zod = compact_query_result_schema::Schema::zod_schema();
    assert!(
        payload_zod.contains("z.array(CompactCommentRow$Schema)"),
        "payload Zod references row schema: {payload_zod}"
    );
}

/// `base_path` is the join root for result paths: a file argument renders its parent directory.
#[test]
fn display_base_path_file_renders_parent_directory() {
    use crate::operations::query::display_base_path;

    assert_eq!(
        display_base_path("notes/generated_types.md", true),
        "notes/"
    );
    assert_eq!(display_base_path("generated_types.md", true), "./");
    assert_eq!(display_base_path("a/b/c.md", true), "a/b/");
}

#[test]
fn display_base_path_directory_renders_argument() {
    use crate::operations::query::display_base_path;

    assert_eq!(display_base_path("notes", false), "notes/");
    assert_eq!(display_base_path("notes/", false), "notes/");
    assert_eq!(display_base_path(".", false), "./");
    assert_eq!(display_base_path("", false), "./");
}

/// Summary counts stay file-wide; only `comments` and `matched_count` narrow to the matches.
#[test]
fn matched_count_reports_the_filtered_subset() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending_for: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    let review = &results[0];
    assert_eq!(review.comment_count, 3);
    assert_eq!(review.matched_count, 1);
    assert_eq!(review.comments.as_ref().unwrap().len(), 1);
    assert_eq!(review.comments.as_ref().unwrap()[0].id, "c2");
}

#[test]
fn matched_count_equals_comment_count_without_filters() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 2);
    for r in &results {
        assert_eq!(r.matched_count, r.comment_count);
    }
}

#[test]
fn summary_mode_reports_matched_count_without_comments() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        author: Some(String::from("bob")),
        summary: true,
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    let review = &results[0];
    assert!(review.comments.is_none());
    assert_eq!(review.comment_count, 3);
    assert_eq!(review.matched_count, 1);
}

/// Combined filters the file-level gates each pass but no single comment
/// satisfies together: bob authored c2, and c3 is the only comment after
/// the cutoff.
fn zero_match_filter() -> QueryFilter {
    QueryFilter {
        author: Some(String::from("bob")),
        since: Some(chrono::DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap()),
        ..QueryFilter::default()
    }
}

#[test]
fn summary_mode_lists_file_with_zero_matches() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        summary: true,
        ..zero_match_filter()
    };

    let results = query(&system, Path::new("/exp"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].comment_count, 3);
    assert_eq!(results[0].matched_count, 0);
}

#[test]
fn non_summary_zero_matches_skips_file() {
    let system = setup_expanded_system();

    let results = query(
        &system,
        Path::new("/exp"),
        &zero_match_filter(),
        &open_config(),
    )
    .unwrap();
    assert!(results.is_empty());
}

#[test]
fn compact_result_carries_matched_count() {
    use crate::operations::query::to_compact_result;

    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending_for: Some(String::from("alice")),
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();

    let compact = to_compact_result(&results[0], false);
    assert_eq!(compact["comment_count"].as_u64().unwrap(), 3);
    assert_eq!(compact["matched_count"].as_u64().unwrap(), 1);
    assert_eq!(compact["comments"].as_array().unwrap().len(), 1);
}

#[test]
fn plain_header_names_both_counts_under_a_filter() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        pending_for: Some(String::from("alice")),
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();

    let output = render_query_plain(&results);
    assert!(
        output.starts_with("review.md (1 of 3 comments, 2 pending)\n"),
        "plain header should name matched and file-wide counts, was:\n{output}"
    );
}

#[test]
fn plain_header_keeps_the_bare_count_without_a_filter() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        expanded: true,
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();

    let output = render_query_plain(&results);
    assert!(
        output.starts_with("review.md (3 comments, 2 pending)\n"),
        "unfiltered plain header should stay unchanged, was:\n{output}"
    );
}

#[test]
fn plain_summary_header_names_both_counts_under_a_filter() {
    let system = setup_expanded_system();
    let filter = QueryFilter {
        author: Some(String::from("bob")),
        summary: true,
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/exp/review.md"),
        &filter,
        &open_config(),
    )
    .unwrap();

    let output = render_query_plain(&results);
    assert_eq!(output, "review.md (1 of 3 comments, 2 pending)\n");
}

#[test]
fn comment_count_pluralizes_the_noun() {
    use crate::operations::query::format_comment_count;

    assert_eq!(format_comment_count(0, 0), "0 comments");
    assert_eq!(format_comment_count(1, 1), "1 comment");
    assert_eq!(format_comment_count(2, 2), "2 comments");
    assert_eq!(format_comment_count(1, 190), "1 of 190 comments");
}

#[test]
fn plain_header_says_one_comment_for_a_single_comment_file() {
    let system = setup_system();
    let filter = QueryFilter {
        summary: true,
        ..QueryFilter::default()
    };
    let results = query(
        &system,
        Path::new("/project/docs/pending.md"),
        &filter,
        &open_config(),
    )
    .unwrap();

    let output = render_query_plain(&results);
    assert_eq!(output, "pending.md (1 comment, 1 pending)\n");
}

fn doc_two_broadcasts() -> &'static str {
    "\
---
title: Two Broadcasts
---

```remargin
---
id: brd_by_alice
author: alice
type: human
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:c0
---
Alice's own broadcast, zero acks.
```

```remargin
---
id: brd_by_bob
author: bob
type: human
ts: 2026-04-06T09:30:00-04:00
checksum: sha256:c1
---
Bob's broadcast, zero acks.
```
"
}

#[test]
fn pending_broadcast_excludes_callers_own_broadcast() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/two"))
        .unwrap()
        .with_file(Path::new("/two/b.md"), doc_two_broadcasts().as_bytes())
        .unwrap();
    let filter = QueryFilter {
        expanded: true,
        pending_broadcast: Some(String::from("alice")),
        ..QueryFilter::default()
    };

    let results = query(&system, Path::new("/two"), &filter, &open_config()).unwrap();
    assert_eq!(results.len(), 1);
    let ids: Vec<&str> = results[0]
        .comments
        .iter()
        .flatten()
        .map(|cm| cm.id.as_str())
        .collect();
    assert_eq!(ids, vec!["brd_by_bob"]);
}

#[test]
fn pending_label_names_the_active_flavor() {
    let directed = QueryFilter {
        pending_for_me: Some(String::from("alice")),
        ..QueryFilter::default()
    };
    assert_eq!(directed.pending_label().as_deref(), Some("for alice"));

    let explicit = QueryFilter {
        pending_for: Some(String::from("bob")),
        ..QueryFilter::default()
    };
    assert_eq!(explicit.pending_label().as_deref(), Some("for bob"));

    let broadcast = QueryFilter {
        pending_broadcast: Some(String::from("alice")),
        ..QueryFilter::default()
    };
    assert_eq!(
        broadcast.pending_label().as_deref(),
        Some("broadcast, unacked by alice")
    );

    let union = QueryFilter {
        pending_for_me: Some(String::from("alice")),
        pending_broadcast: Some(String::from("alice")),
        ..QueryFilter::default()
    };
    assert_eq!(
        union.pending_label().as_deref(),
        Some("for alice or broadcast")
    );

    assert_eq!(QueryFilter::default().pending_label(), None);
}
