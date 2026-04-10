//! Tests for the comment block writer.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;

use chrono::DateTime;
use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::parser::{self, Acknowledgment, AuthorType, Comment};

use super::{
    InsertPosition, insert_comment, serialize_comment, verify_preservation, write_document,
};

/// Build a minimal comment for testing.
fn make_comment(id: &str, content: &str) -> Comment {
    Comment {
        ack: Vec::new(),
        attachments: Vec::new(),
        author: String::from("testuser"),
        author_type: AuthorType::Human,
        checksum: String::from("sha256:abc123"),
        content: String::from(content),
        fence_depth: 3,
        id: String::from(id),
        line: 0,
        reactions: BTreeMap::new(),
        reply_to: None,
        signature: None,
        thread: None,
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339("2026-04-06T14:32:00-04:00").unwrap(),
    }
}

#[test]
fn simple_serialize() {
    let comment = make_comment("abc", "Hello world.");
    let output = serialize_comment(&comment);

    assert!(output.starts_with("```remargin\n---\n"));
    assert!(output.contains("id: abc\n"));
    assert!(output.contains("author: testuser\n"));
    assert!(output.contains("type: human\n"));
    assert!(output.contains("checksum: sha256:abc123\n"));
    assert!(output.contains("Hello world.\n"));
    assert!(output.ends_with("```\n"));
}

#[test]
fn full_serialize() {
    let mut reactions = BTreeMap::new();
    reactions.insert(String::from("thumbsup"), vec![String::from("bob")]);

    let comment = Comment {
        ack: vec![Acknowledgment {
            author: String::from("jorge"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T15:00:00-04:00").unwrap(),
        }],
        attachments: vec![String::from("diagram.png")],
        author: String::from("eduardo"),
        author_type: AuthorType::Agent,
        checksum: String::from("sha256:deadbeef"),
        content: String::from("Full comment body."),
        fence_depth: 3,
        id: String::from("full"),
        line: 0,
        reactions,
        reply_to: Some(String::from("xyz")),
        signature: Some(String::from("ed25519:sig==")),
        thread: Some(String::from("t01")),
        to: vec![String::from("jorge"), String::from("claude")],
        ts: DateTime::parse_from_rfc3339("2026-04-06T14:32:00-04:00").unwrap(),
    };

    let output = serialize_comment(&comment);

    // Verify canonical field order by checking relative positions.
    let id_pos = output.find("id: full").unwrap();
    let author_pos = output.find("author: eduardo").unwrap();
    let type_pos = output.find("type: agent").unwrap();
    let timestamp_pos = output.find("ts: ").unwrap();
    let to_pos = output.find("to: [jorge, claude]").unwrap();
    let reply_pos = output.find("reply-to: xyz").unwrap();
    let thread_pos = output.find("thread: t01").unwrap();
    let attach_pos = output.find("attachments: [diagram.png]").unwrap();
    let react_pos = output.find("reactions:").unwrap();
    let ack_pos = output.find("ack:").unwrap();
    let checksum_pos = output.find("checksum: sha256:deadbeef").unwrap();
    let sig_pos = output.find("signature: ed25519:sig==").unwrap();

    assert!(id_pos < author_pos);
    assert!(author_pos < type_pos);
    assert!(type_pos < timestamp_pos);
    assert!(timestamp_pos < to_pos);
    assert!(to_pos < reply_pos);
    assert!(reply_pos < thread_pos);
    assert!(thread_pos < attach_pos);
    assert!(attach_pos < react_pos);
    assert!(react_pos < ack_pos);
    assert!(ack_pos < checksum_pos);
    assert!(checksum_pos < sig_pos);
}

#[test]
fn fence_depth_three_no_backticks() {
    let comment = make_comment("fd3", "No backticks here.");
    let output = serialize_comment(&comment);
    assert!(output.starts_with("```remargin\n"));
}

#[test]
fn fence_depth_four_code_blocks() {
    let comment = make_comment("fd4", "```python\nprint('hello')\n```");
    let output = serialize_comment(&comment);
    assert!(output.starts_with("````remargin\n"));
}

#[test]
fn fence_depth_six_deep_nesting() {
    let comment = make_comment("fd6", "`````remargin\nquoted block\n`````");
    let output = serialize_comment(&comment);
    assert!(output.starts_with("``````remargin\n"));
}

#[test]
fn insert_after_comment() {
    let doc_str = "\
```remargin
---
id: abc
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
---
First comment.
```
Some text after.
";
    let mut doc = parser::parse(doc_str).unwrap();
    let new_comment = make_comment("new1", "Inserted comment.");
    insert_comment(
        &mut doc,
        new_comment,
        &InsertPosition::AfterComment(String::from("abc")),
    )
    .unwrap();

    let ids: Vec<&str> = doc.comments().iter().map(|cm| cm.id.as_str()).collect();
    assert_eq!(ids, vec!["abc", "new1"]);
}

#[test]
fn insert_after_line() {
    let doc_str = "Line 1\nLine 2\nLine 3\n";
    let mut doc = parser::parse(doc_str).unwrap();
    let new_comment = make_comment("ln2", "After line 2.");
    insert_comment(&mut doc, new_comment, &InsertPosition::AfterLine(2)).unwrap();

    let markdown = doc.to_markdown();
    let comments = parser::parse(&markdown).unwrap().comments().len();
    assert_eq!(comments, 1_usize);

    // Verify the comment appears between line 2 and line 3.
    let line2_pos = markdown.find("Line 2").unwrap();
    let comment_pos = markdown.find("id: ln2").unwrap();
    let line3_pos = markdown.find("Line 3").unwrap();
    assert!(line2_pos < comment_pos);
    assert!(comment_pos < line3_pos);
}

#[test]
fn append_comment() {
    let doc_str = "# Title\n\nSome text.\n";
    let mut doc = parser::parse(doc_str).unwrap();
    let new_comment = make_comment("end1", "Appended.");
    insert_comment(&mut doc, new_comment, &InsertPosition::Append).unwrap();

    let markdown = doc.to_markdown();
    assert!(markdown.contains("id: end1"));

    // The comment should be at the end.
    let text_pos = markdown.find("Some text.").unwrap();
    let comment_pos = markdown.find("id: end1").unwrap();
    assert!(text_pos < comment_pos);
}

#[test]
fn round_trip_serialize() {
    let comment = make_comment("rt1", "Round-trip test body.");
    let serialized = serialize_comment(&comment);
    let doc = parser::parse(&serialized).unwrap();
    let reparsed = doc.comments()[0];

    assert_eq!(reparsed.id, "rt1");
    assert_eq!(reparsed.content, "Round-trip test body.");

    // Re-serialize and verify structural equivalence.
    let reserialized = serialize_comment(reparsed);
    let doc2 = parser::parse(&reserialized).unwrap();
    assert_eq!(doc2.comments()[0].id, "rt1");
    assert_eq!(doc2.comments()[0].content, "Round-trip test body.");
}

#[test]
fn preservation_pass() {
    let mut before = HashSet::new();
    before.insert(String::from("a01"));
    before.insert(String::from("b02"));

    let mut after = HashSet::new();
    after.insert(String::from("a01"));
    after.insert(String::from("b02"));
    after.insert(String::from("c03"));

    let mut added = HashSet::new();
    added.insert(String::from("c03"));
    let removed = HashSet::new();

    verify_preservation(&before, &after, &added, &removed).unwrap();
}

#[test]
fn preservation_fail_unexpected() {
    let mut before = HashSet::new();
    before.insert(String::from("a01"));

    let mut after = HashSet::new();
    after.insert(String::from("a01"));
    after.insert(String::from("sneaky"));

    let added = HashSet::new();
    let removed = HashSet::new();

    let err = verify_preservation(&before, &after, &added, &removed).unwrap_err();
    assert!(
        format!("{err}").contains("unexpected"),
        "expected 'unexpected' error, got: {err}"
    );
}

#[test]
fn write_with_mock_system() {
    let comment = make_comment("wrt1", "Written comment.");
    let doc_str = serialize_comment(&comment);
    let doc = parser::parse(&doc_str).unwrap();

    let system = MockSystem::new().with_dir(Path::new("/docs")).unwrap();

    let added = HashSet::new();
    let removed = HashSet::new();

    write_document(&system, Path::new("/docs/test.md"), &doc, &added, &removed).unwrap();

    // Verify the file was written.
    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert!(content.contains("id: wrt1"));
}

#[test]
fn insert_after_line_no_double_newline() {
    let doc_str = "Line 1\nLine 2\nLine 3\n";
    let mut doc = parser::parse(doc_str).unwrap();
    let comment = make_comment("aln1", "After line insert.");
    insert_comment(&mut doc, comment, &InsertPosition::AfterLine(2)).unwrap();

    let markdown = doc.to_markdown();
    assert!(
        !markdown.contains("\n\n\n"),
        "AfterLine insert produced triple-newline:\n{markdown}"
    );
}

#[test]
fn insert_after_comment_no_double_newline() {
    let doc_str = "\
```remargin
---
id: abc
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
---
First comment.
```
Some text after.
";
    let mut doc = parser::parse(doc_str).unwrap();
    let comment = make_comment("acn1", "After comment insert.");
    insert_comment(
        &mut doc,
        comment,
        &InsertPosition::AfterComment(String::from("abc")),
    )
    .unwrap();

    let markdown = doc.to_markdown();
    assert!(
        !markdown.contains("\n\n\n"),
        "AfterComment insert produced triple-newline:\n{markdown}"
    );
}

#[test]
fn insert_append_no_double_newline() {
    let doc_str = "# Title\n\nSome text.\n";
    let mut doc = parser::parse(doc_str).unwrap();
    let comment = make_comment("apn1", "Appended.");
    insert_comment(&mut doc, comment, &InsertPosition::Append).unwrap();

    let markdown = doc.to_markdown();
    assert!(
        !markdown.contains("\n\n\n"),
        "Append insert produced triple-newline:\n{markdown}"
    );
}

#[test]
fn insert_after_nonexistent_comment() {
    let doc_str = "# Just text\n";
    let mut doc = parser::parse(doc_str).unwrap();
    let new_comment = make_comment("err1", "Oops.");
    let result = insert_comment(
        &mut doc,
        new_comment,
        &InsertPosition::AfterComment(String::from("nonexistent")),
    );
    result.unwrap_err();
}

#[test]
fn insert_after_last_line() {
    let doc_str = "Line 1\nLine 2\nLine 3\n";
    let line_count = doc_str.split('\n').count();
    let mut doc = parser::parse(doc_str).unwrap();
    let new_comment = make_comment("last", "After last line.");
    insert_comment(
        &mut doc,
        new_comment,
        &InsertPosition::AfterLine(line_count),
    )
    .unwrap();

    let markdown = doc.to_markdown();
    assert!(markdown.contains("id: last"));

    // Comment should appear after all body text.
    let line3_pos = markdown.find("Line 3").unwrap();
    let comment_pos = markdown.find("id: last").unwrap();
    assert!(line3_pos < comment_pos);
}

#[test]
fn insert_after_line_beyond_length_clamps() {
    let doc_str = "Line 1\nLine 2\n";
    let mut doc = parser::parse(doc_str).unwrap();
    let new_comment = make_comment("far", "Way past the end.");
    insert_comment(&mut doc, new_comment, &InsertPosition::AfterLine(10000)).unwrap();

    let markdown = doc.to_markdown();
    assert!(markdown.contains("id: far"));

    // Comment should appear after all body text (effectively appended).
    let line2_pos = markdown.find("Line 2").unwrap();
    let comment_pos = markdown.find("id: far").unwrap();
    assert!(line2_pos < comment_pos);
}
