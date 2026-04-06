//! Tests for batch comment operations.

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::operations::batch::{BatchCommentOp, batch_comment};
use crate::parser::{self, AuthorType};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const MINIMAL_DOC: &str = "\
---
title: Test
author: eduardo
---

# Test Document

Some body text.
";

/// A longer document with numbered lines for precise `after_line` testing.
const MULTILINE_DOC: &str = "\
---
title: Test
author: eduardo
---

# Heading

Line one.

Line two.

Line three.

Line four.

Line five.
";

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
    }
}

fn system_with_doc(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/docs/test.md"), content.as_bytes())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: Simple batch -- 3 independent comments
// ---------------------------------------------------------------------------

#[test]
fn simple_batch() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();

    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            content: String::from("First batch comment."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            content: String::from("Second batch comment."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            content: String::from("Third batch comment."),
            reply_to: None,
            to: Vec::new(),
        },
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 3);

    // Verify all 3 comments exist in the document.
    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 3);
}

// ---------------------------------------------------------------------------
// Test 2: Batch with reply
// ---------------------------------------------------------------------------

#[test]
fn batch_with_reply() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();

    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            content: String::from("Root comment."),
            reply_to: None,
            to: Vec::new(),
        },
        // We will set reply_to after creating the first one.
        // Since batch processes in order, we cannot reference
        // future IDs. But we can test the basic flow.
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 1);

    // Now create a reply batch.
    let reply_ops = vec![BatchCommentOp {
        after_comment: None,
        after_line: None,
        attachments: Vec::new(),
        content: String::from("Reply to root."),
        reply_to: Some(ids[0].clone()),
        to: Vec::new(),
    }];

    let reply_ids =
        batch_comment(&system, Path::new("/docs/test.md"), &config, &reply_ops).unwrap();
    assert_eq!(reply_ids.len(), 1);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&reply_ids[0]).unwrap();
    assert_eq!(reply.reply_to.as_deref(), Some(ids[0].as_str()));
    assert_eq!(reply.thread.as_deref(), Some(ids[0].as_str()));
}

// ---------------------------------------------------------------------------
// Test 3: Batch failure -- missing attachment
// ---------------------------------------------------------------------------

#[test]
fn batch_failure_rolls_back() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();

    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            content: String::from("Good comment."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: vec![PathBuf::from("/nonexistent/file.png")],
            content: String::from("Bad comment with missing attachment."),
            reply_to: None,
            to: Vec::new(),
        },
    ];

    let result = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops);
    result.unwrap_err();

    // Original document should be unchanged (all-or-nothing).
    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert!(doc.comments().is_empty());
}

// ---------------------------------------------------------------------------
// Test 5: Preservation check
// ---------------------------------------------------------------------------

#[test]
fn preservation_check() {
    let doc_with_existing = "\
---
title: Test
author: eduardo
---

# Test

```remargin
---
id: existing
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:aaa
---
Existing comment.
```
";
    let system = system_with_doc(doc_with_existing);
    let config = open_config();

    let ops = vec![BatchCommentOp {
        after_comment: None,
        after_line: None,
        attachments: Vec::new(),
        content: String::from("New batch comment."),
        reply_to: None,
        to: Vec::new(),
    }];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 1);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 2);
    assert!(doc.find_comment("existing").is_some());
    assert!(doc.find_comment(&ids[0]).is_some());
}

// ---------------------------------------------------------------------------
// Test 6: Batch with multiple after_line positions (BUG rem-dbf)
// ---------------------------------------------------------------------------

#[test]
fn batch_two_after_line_comments_both_placed_correctly() {
    let system = system_with_doc(MULTILINE_DOC);
    let config = open_config();

    // Insert comment A after "Line one." and comment B after "Line four."
    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: Some(9), // after "Line one."
            attachments: Vec::new(),
            content: String::from("Comment after line one."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: Some(13), // after "Line three."
            attachments: Vec::new(),
            content: String::from("Comment after line three."),
            reply_to: None,
            to: Vec::new(),
        },
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 2);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 2);

    // Both comments must exist and be distinct.
    let cm_a = doc.find_comment(&ids[0]).unwrap();
    let cm_b = doc.find_comment(&ids[1]).unwrap();
    assert_eq!(cm_a.content, "Comment after line one.");
    assert_eq!(cm_b.content, "Comment after line three.");

    // Comment A must appear before Comment B in the document.
    assert!(
        cm_a.line < cm_b.line,
        "Comment A (line {}) should be before Comment B (line {})",
        cm_a.line,
        cm_b.line
    );

    // Verify the body text is still intact — "Line one." and "Line three." still exist.
    assert!(
        content.contains("Line one."),
        "body text 'Line one.' missing"
    );
    assert!(
        content.contains("Line three."),
        "body text 'Line three.' missing"
    );
}

#[test]
fn batch_after_line_reverse_order() {
    let system = system_with_doc(MULTILINE_DOC);
    let config = open_config();

    // Insert in reverse order: higher line first, lower line second.
    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: Some(13), // after "Line three." (higher)
            attachments: Vec::new(),
            content: String::from("Comment after line three."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: Some(9), // after "Line one." (lower)
            attachments: Vec::new(),
            content: String::from("Comment after line one."),
            reply_to: None,
            to: Vec::new(),
        },
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 2);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();

    let cm_three = doc.find_comment(&ids[0]).unwrap();
    let cm_one = doc.find_comment(&ids[1]).unwrap();

    // Even though "after line three" was submitted first, "after line one"
    // should appear earlier in the document.
    assert!(
        cm_one.line < cm_three.line,
        "Comment at line one ({}) should be before comment at line three ({})",
        cm_one.line,
        cm_three.line
    );
}

#[test]
fn batch_three_after_line_same_region() {
    let system = system_with_doc(MULTILINE_DOC);
    let config = open_config();

    // Three comments targeting consecutive lines.
    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: Some(9), // after "Line one."
            attachments: Vec::new(),
            content: String::from("First."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: Some(11), // after "Line two."
            attachments: Vec::new(),
            content: String::from("Second."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: Some(13), // after "Line three."
            attachments: Vec::new(),
            content: String::from("Third."),
            reply_to: None,
            to: Vec::new(),
        },
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 3);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 3);

    let cm_1 = doc.find_comment(&ids[0]).unwrap();
    let cm_2 = doc.find_comment(&ids[1]).unwrap();
    let cm_3 = doc.find_comment(&ids[2]).unwrap();

    // All three must be in document order matching their target lines.
    assert!(
        cm_1.line < cm_2.line,
        "First ({}) should be before Second ({})",
        cm_1.line,
        cm_2.line
    );
    assert!(
        cm_2.line < cm_3.line,
        "Second ({}) should be before Third ({})",
        cm_2.line,
        cm_3.line
    );
}

#[test]
fn batch_mixed_after_line_and_append() {
    let system = system_with_doc(MULTILINE_DOC);
    let config = open_config();

    // Mix: one after_line, one append, one after_line.
    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: Some(9), // after "Line one."
            attachments: Vec::new(),
            content: String::from("Positioned comment."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: None, // append
            attachments: Vec::new(),
            content: String::from("Appended comment."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: Some(13), // after "Line three."
            attachments: Vec::new(),
            content: String::from("Another positioned comment."),
            reply_to: None,
            to: Vec::new(),
        },
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 3);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 3);

    let cm_pos1 = doc.find_comment(&ids[0]).unwrap();
    let cm_append = doc.find_comment(&ids[1]).unwrap();
    let cm_pos2 = doc.find_comment(&ids[2]).unwrap();

    // Positioned comments in body, appended at end.
    assert!(
        cm_pos1.line < cm_pos2.line,
        "Positioned at line 9 ({}) should be before positioned at line 13 ({})",
        cm_pos1.line,
        cm_pos2.line
    );
    assert!(
        cm_pos2.line < cm_append.line,
        "Positioned at line 13 ({}) should be before appended ({})",
        cm_pos2.line,
        cm_append.line
    );
}

#[test]
fn batch_after_line_with_reply_in_same_batch() {
    let doc_with_comment = "\
---
title: Test
author: eduardo
---

# Heading

```remargin
---
id: root
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:aaa
---
Root comment.
```

Line after comment.
";
    let system = system_with_doc(doc_with_comment);
    let config = open_config();

    // One positioned comment + one reply to existing comment.
    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: Some(19), // after "Line after comment."
            attachments: Vec::new(),
            content: String::from("New comment at end."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            content: String::from("Reply to root."),
            reply_to: Some(String::from("root")),
            to: Vec::new(),
        },
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 2);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 3); // root + 2 new

    let reply = doc.find_comment(&ids[1]).unwrap();
    assert_eq!(reply.reply_to.as_deref(), Some("root"));
    assert_eq!(reply.thread.as_deref(), Some("root"));

    // The reply should be after the root comment (placed by reply_to logic).
    let root = doc.find_comment("root").unwrap();
    assert!(
        reply.line > root.line,
        "Reply ({}) should be after root ({})",
        reply.line,
        root.line
    );
}

#[test]
fn batch_two_after_same_line() {
    let system = system_with_doc(MULTILINE_DOC);
    let config = open_config();

    // Two comments both targeting the same line.
    let ops = vec![
        BatchCommentOp {
            after_comment: None,
            after_line: Some(9), // after "Line one."
            attachments: Vec::new(),
            content: String::from("First at line 9."),
            reply_to: None,
            to: Vec::new(),
        },
        BatchCommentOp {
            after_comment: None,
            after_line: Some(9), // also after "Line one."
            attachments: Vec::new(),
            content: String::from("Second at line 9."),
            reply_to: None,
            to: Vec::new(),
        },
    ];

    let ids = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();
    assert_eq!(ids.len(), 2);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 2);

    let cm_1 = doc.find_comment(&ids[0]).unwrap();
    let cm_2 = doc.find_comment(&ids[1]).unwrap();

    // Both should be near line 9, and first should come before second.
    assert!(
        cm_1.line < cm_2.line,
        "First ({}) should be before second ({})",
        cm_1.line,
        cm_2.line
    );
}

// ---------------------------------------------------------------------------
// Negative tests for after_line batch
// ---------------------------------------------------------------------------

#[test]
fn batch_after_line_beyond_document_length() {
    let system = system_with_doc(MULTILINE_DOC);
    let config = open_config();

    let ops = vec![BatchCommentOp {
        after_comment: None,
        after_line: Some(9999), // way beyond document length
        attachments: Vec::new(),
        content: String::from("Should fail."),
        reply_to: None,
        to: Vec::new(),
    }];

    let result = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops);
    result.unwrap_err();
}

#[test]
fn batch_after_line_zero() {
    let system = system_with_doc(MULTILINE_DOC);
    let config = open_config();

    // Line 0 is before the first line — should insert at the very top.
    let ops = vec![BatchCommentOp {
        after_comment: None,
        after_line: Some(0),
        attachments: Vec::new(),
        content: String::from("At the very top."),
        reply_to: None,
        to: Vec::new(),
    }];

    // This should either work (insert at top) or error gracefully — not corrupt.
    let result = batch_comment(&system, Path::new("/docs/test.md"), &config, &ops);
    if let Ok(ids) = result {
        assert_eq!(ids.len(), 1);
        let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
        let doc = parser::parse(&content).unwrap();
        assert_eq!(doc.comments().len(), 1);
    }
    // If it errors, that's also acceptable — just not corruption.
}
