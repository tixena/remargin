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
