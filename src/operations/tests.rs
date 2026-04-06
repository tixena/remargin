//! Tests for comment operations.

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::operations::{
    CreateCommentParams, ack_comments, create_comment, delete_comments, edit_comment, react,
};
use crate::parser::{self, AuthorType};
use crate::writer::InsertPosition;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A minimal valid remargin document for testing.
const MINIMAL_DOC: &str = "\
---
title: Test
author: eduardo
---

# Test Document

Some body text.
";

/// A document with one existing comment.
fn doc_with_comment() -> String {
    String::from(
        "\
---
title: Test
author: eduardo
---

# Test Document

Some body text.

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
---
First comment.
```
",
    )
}

/// A document with a comment chain for cascade testing.
fn doc_with_thread() -> String {
    String::from(
        "\
---
title: Test
author: eduardo
---

# Test Document

```remargin
---
id: root
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:aaa
ack:
  - alice@2026-04-06T13:00:00-04:00
---
Root comment.
```

```remargin
---
id: child1
author: alice
type: human
ts: 2026-04-06T13:00:00-04:00
reply-to: root
thread: root
checksum: sha256:bbb
ack:
  - eduardo@2026-04-06T14:00:00-04:00
---
Reply to root.
```

```remargin
---
id: grandchild
author: eduardo
type: human
ts: 2026-04-06T14:00:00-04:00
reply-to: child1
thread: root
checksum: sha256:ccc
ack:
  - alice@2026-04-06T15:00:00-04:00
---
Reply to child1.
```
",
    )
}

/// Create a test config with open mode.
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

/// Create a mock system with a document file.
fn system_with_doc(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/docs/test.md"), content.as_bytes())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: Create simple comment
// ---------------------------------------------------------------------------

#[test]
fn create_simple_comment() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();
    let position = InsertPosition::Append;

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            content: "This is a new comment.",
            position: &position,
            reply_to: None,
            to: &[],
        },
    )
    .unwrap();

    // Verify the comment was created.
    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let comments = doc.comments();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, new_id);
    assert_eq!(comments[0].content, "This is a new comment.");
    assert!(comments[0].checksum.starts_with("sha256:"));
}

// ---------------------------------------------------------------------------
// Test 2: Create with attachment
// ---------------------------------------------------------------------------

#[test]
fn create_with_attachment() {
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), MINIMAL_DOC.as_bytes())
        .unwrap()
        .with_file(Path::new("/tmp/screenshot.png"), b"PNG_DATA")
        .unwrap()
        .with_dir(Path::new("/docs/assets"))
        .unwrap();

    let config = open_config();
    let position = InsertPosition::Append;

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[PathBuf::from("/tmp/screenshot.png")],
            content: "See attached.",
            position: &position,
            reply_to: None,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment(&new_id).unwrap();
    assert_eq!(cm.attachments.len(), 1);
    assert!(cm.attachments[0].contains("screenshot.png"));
}

// ---------------------------------------------------------------------------
// Test 4: Create reply -- thread auto-populated
// ---------------------------------------------------------------------------

#[test]
fn create_reply_auto_thread() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();
    let position = InsertPosition::Append;

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            content: "Replying to abc.",
            position: &position,
            reply_to: Some("abc"),
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&new_id).unwrap();
    assert_eq!(reply.reply_to.as_deref(), Some("abc"));
    assert_eq!(reply.thread.as_deref(), Some("abc"));
}

// ---------------------------------------------------------------------------
// Test 5: Ack single
// ---------------------------------------------------------------------------

#[test]
fn ack_single_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    ack_comments(&system, Path::new("/docs/test.md"), &config, &["abc"]).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert_eq!(cm.ack.len(), 1);
    assert_eq!(cm.ack[0].author, "eduardo");
}

// ---------------------------------------------------------------------------
// Test 7: React add
// ---------------------------------------------------------------------------

#[test]
fn react_add_emoji() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    react(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "abc",
        "thumbsup",
        false,
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert!(cm.reactions.contains_key("thumbsup"));
    assert!(cm.reactions["thumbsup"].contains(&String::from("eduardo")));
}

// ---------------------------------------------------------------------------
// Test 8: React remove
// ---------------------------------------------------------------------------

#[test]
fn react_remove_emoji() {
    // Start with a document that has a reaction.
    let doc_content = "\
---
title: Test
author: eduardo
---

# Test Document

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
reactions:
  thumbsup: [eduardo]
---
Content.
```
";
    let system = system_with_doc(doc_content);
    let config = open_config();

    react(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "abc",
        "thumbsup",
        true,
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert!(
        !cm.reactions.contains_key("thumbsup"),
        "reaction should be removed"
    );
}

// ---------------------------------------------------------------------------
// Test 10: Delete simple
// ---------------------------------------------------------------------------

#[test]
fn delete_simple_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    delete_comments(&system, Path::new("/docs/test.md"), &config, &["abc"]).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert!(doc.comments().is_empty());
}

// ---------------------------------------------------------------------------
// Test 13: Edit content -- checksum recalculated, ack cleared
// ---------------------------------------------------------------------------

#[test]
fn edit_content_recomputes_checksum() {
    let doc_content = "\
---
title: Test
author: eduardo
---

# Test Document

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:old_checksum
ack:
  - alice@2026-04-06T13:00:00-04:00
---
Original content.
```
";
    let system = system_with_doc(doc_content);
    let config = open_config();

    edit_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "abc",
        "Updated content.",
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert_eq!(cm.content, "Updated content.");
    assert_ne!(cm.checksum, "sha256:old_checksum");
    assert!(cm.ack.is_empty(), "ack should be cleared after edit");
}

// ---------------------------------------------------------------------------
// Test 14: Edit cascade -- ack cleared on edited comment and children
// ---------------------------------------------------------------------------

#[test]
fn edit_cascade_clears_acks() {
    let system = system_with_doc(&doc_with_thread());
    let config = open_config();

    edit_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "root",
        "Edited root content.",
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();

    // Root's ack should be cleared.
    let root = doc.find_comment("root").unwrap();
    assert!(root.ack.is_empty(), "root ack should be cleared");

    // Child1's ack should be cleared (cascading).
    let child1 = doc.find_comment("child1").unwrap();
    assert!(child1.ack.is_empty(), "child1 ack should be cleared");

    // Grandchild's ack should be cleared (deep cascading).
    let grandchild = doc.find_comment("grandchild").unwrap();
    assert!(
        grandchild.ack.is_empty(),
        "grandchild ack should be cleared"
    );
}

// ---------------------------------------------------------------------------
// Test 16: Preservation invariant
// ---------------------------------------------------------------------------

#[test]
fn preservation_invariant() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();
    let position = InsertPosition::Append;

    // Create a second comment.
    create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            content: "Second comment.",
            position: &position,
            reply_to: None,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let comments = doc.comments();
    assert_eq!(comments.len(), 2);

    // Original comment still present.
    assert!(doc.find_comment("abc").is_some());
}

// ---------------------------------------------------------------------------
// Error cases
// ---------------------------------------------------------------------------

#[test]
fn ack_nonexistent_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    let result = ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["nonexistent"],
    );
    assert!(result.is_err());
}

#[test]
fn delete_nonexistent_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    let result = delete_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["nonexistent"],
    );
    assert!(result.is_err());
}

#[test]
fn edit_nonexistent_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    let result = edit_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "nonexistent",
        "new content",
    );
    assert!(result.is_err());
}
