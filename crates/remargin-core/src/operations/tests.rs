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
        unrestricted: false,
    }
}

/// Create a mock system with a document file.
fn system_with_doc(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/docs/test.md"), content.as_bytes())
        .unwrap()
}

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
            auto_ack: false,
            content: "This is a new comment.",
            position: &position,
            reply_to: None,
            sandbox: false,
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

#[test]
fn create_comment_with_sandbox_stages_and_writes_together() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();
    let position = InsertPosition::Append;

    let _new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Staged with sandbox.",
            position: &position,
            reply_to: None,
            sandbox: true,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert!(
        content.contains("sandbox:"),
        "sandbox frontmatter key should be set after atomic comment+sandbox write: {content}",
    );
    assert!(
        content.contains("eduardo@"),
        "sandbox entry for caller should be appended: {content}",
    );

    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 1);
    assert_eq!(doc.comments()[0].content, "Staged with sandbox.");
}

#[test]
fn create_comment_with_sandbox_is_idempotent_against_existing_entry() {
    // Seed the document with a pre-existing sandbox entry for eduardo.
    let seeded = "\
---
title: Test
author: eduardo
sandbox:
- eduardo@2026-04-11T10:00:00+00:00
---

# Test Document

Body.
";
    let system = system_with_doc(seeded);
    let config = open_config();
    let position = InsertPosition::Append;

    create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Second comment.",
            position: &position,
            reply_to: None,
            sandbox: true,
            to: &[],
        },
    )
    .unwrap();

    // Existing `10:00:00` timestamp is preserved (no refresh).
    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert!(content.contains("eduardo@2026-04-11T10:00:00+00:00"));
    // And the comment was still written.
    let doc = parser::parse(&content).unwrap();
    assert_eq!(doc.comments().len(), 1);
}

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
            auto_ack: false,
            content: "See attached.",
            position: &position,
            reply_to: None,
            sandbox: false,
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
            auto_ack: false,
            content: "Replying to abc.",
            position: &position,
            reply_to: Some("abc"),
            sandbox: false,
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

#[test]
fn ack_single_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert_eq!(cm.ack.len(), 1);
    assert_eq!(cm.ack[0].author, "eduardo");
}

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

#[test]
fn delete_simple_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    delete_comments(&system, Path::new("/docs/test.md"), &config, &["abc"]).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert!(doc.comments().is_empty());
}

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
            auto_ack: false,
            content: "Second comment.",
            position: &position,
            reply_to: None,
            sandbox: false,
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

#[test]
fn delete_restores_original_whitespace() {
    let original = "\
---
title: Test
author: eduardo
---

# Section One

Start with the HTTP transport.

### 2. Inline widget complexity

More text here.
";
    let system = system_with_doc(original);
    let config = open_config();
    let position = InsertPosition::AfterLine(9);

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "test comment",
            position: &position,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    // Delete the comment.
    delete_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &[new_id.as_str()],
    )
    .unwrap();

    let after = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    // The document should not have triple-newline sequences (max one blank line).
    assert!(
        !after.contains("\n\n\n"),
        "delete left triple-newline artifact:\n{after}"
    );
}

#[test]
fn delete_at_end_no_trailing_blanks() {
    let original = "\
---
title: Test
author: eduardo
---

# Test Document

Some body text.
";
    let system = system_with_doc(original);
    let config = open_config();
    let position = InsertPosition::Append;

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "appended comment",
            position: &position,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    delete_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &[new_id.as_str()],
    )
    .unwrap();

    let after = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    assert!(
        !after.contains("\n\n\n"),
        "delete at end left trailing blank lines:\n{after}"
    );
}

#[test]
fn delete_after_frontmatter_no_leading_blanks() {
    let original = "\
---
title: Test
author: eduardo
---

# Test Document

Some text.
";
    let system = system_with_doc(original);
    let config = open_config();
    let position = InsertPosition::AfterLine(5);

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "after frontmatter",
            position: &position,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    delete_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &[new_id.as_str()],
    )
    .unwrap();

    let after = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    assert!(
        !after.contains("\n\n\n"),
        "delete after frontmatter left leading blank lines:\n{after}"
    );
}

#[test]
fn delete_multiple_adjacent_comments() {
    let original = "\
---
title: Test
author: eduardo
---

# Test Document

Some body text.
";
    let system = system_with_doc(original);
    let config = open_config();

    // Insert three comments consecutively.
    let id1 = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "first",
            position: &InsertPosition::Append,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let id2 = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "second",
            position: &InsertPosition::Append,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let id3 = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "third",
            position: &InsertPosition::Append,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    // Delete all three.
    delete_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &[id1.as_str(), id2.as_str(), id3.as_str()],
    )
    .unwrap();

    let after = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    assert!(
        !after.contains("\n\n\n"),
        "deleting multiple adjacent comments left excessive blank lines:\n{after}"
    );
}

#[test]
fn delete_middle_of_three_comments() {
    let original = "\
---
title: Test
author: eduardo
---

# Test Document

Some body text.
";
    let system = system_with_doc(original);
    let config = open_config();

    let id1 = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "first",
            position: &InsertPosition::Append,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let id2 = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "second",
            position: &InsertPosition::AfterComment(id1),
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let _id3 = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "third",
            position: &InsertPosition::AfterComment(id2.clone()),
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    // Delete only the middle comment.
    delete_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &[id2.as_str()],
    )
    .unwrap();

    let after = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&after).unwrap();

    // Two comments should remain.
    assert_eq!(doc.comments().len(), 2);

    assert!(
        !after.contains("\n\n\n"),
        "deleting middle comment left excessive blank lines:\n{after}"
    );
}

#[test]
fn delete_collapses_adjacent_body_segments() {
    use crate::parser::{ParsedDocument, Segment};

    let mut doc = ParsedDocument {
        segments: vec![
            Segment::Body(String::from("Text before.\n")),
            Segment::Body(String::from("\n")),
            Segment::Body(String::from("\n")),
            Segment::Body(String::from("Text after.\n")),
        ],
    };

    super::collapse_body_segments(&mut doc.segments);

    let markdown = doc.to_markdown();
    assert!(
        !markdown.contains("\n\n\n"),
        "collapsed body segments still have triple-newline:\n{markdown}"
    );
    assert!(markdown.contains("Text before."));
    assert!(markdown.contains("Text after."));
}

#[test]
fn delete_preserves_intentional_blank_lines() {
    // A document with two blank lines between sections (intentional) and a
    // comment at the end. Deleting the comment should preserve the existing
    // two-newline separation (one blank line) in the body.
    let original = "\
---
title: Test
author: eduardo
---

# Section One

Text in section one.

## Section Two

Text in section two.

```remargin
---
id: xyz
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
---
A comment at the end.
```
";
    let system = system_with_doc(original);
    let config = open_config();

    delete_comments(&system, Path::new("/docs/test.md"), &config, &["xyz"]).unwrap();

    let after = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    // The blank line between Section One and Section Two should be preserved.
    assert!(
        after.contains("Text in section one.\n\n## Section Two"),
        "intentional blank line between sections was removed:\n{after}"
    );
}

#[test]
fn ack_nonexistent_comment() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    let result = ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["nonexistent"],
        false,
    );
    assert!(result.is_err());
}

#[test]
fn ack_remove_clears_identity_ack() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    // First, ack so there is something to remove.
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();
    // Then remove the identity's ack.
    ack_comments(&system, Path::new("/docs/test.md"), &config, &["abc"], true).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert!(
        cm.ack.iter().all(|a| a.author != "eduardo"),
        "expected eduardo's ack to be removed, got {:?}",
        cm.ack
    );
}

#[test]
fn ack_remove_preserves_other_acks() {
    let system = system_with_doc(&doc_with_comment());
    let mut eduardo_config = open_config();
    let mut agent_config = open_config();
    agent_config.identity = Some(String::from("some_agent"));

    // Both identities ack the same comment.
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &eduardo_config,
        &["abc"],
        false,
    )
    .unwrap();
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &agent_config,
        &["abc"],
        false,
    )
    .unwrap();

    // Eduardo removes only his ack.
    eduardo_config.identity = Some(String::from("eduardo"));
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &eduardo_config,
        &["abc"],
        true,
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    let authors: Vec<&str> = cm.ack.iter().map(|a| a.author.as_str()).collect();
    assert_eq!(authors, vec!["some_agent"]);
}

#[test]
fn ack_remove_is_idempotent_when_not_acked() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    // Removing a non-existent ack should be a no-op (not an error).
    ack_comments(&system, Path::new("/docs/test.md"), &config, &["abc"], true).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert!(cm.ack.is_empty());
}

#[test]
fn ack_twice_is_idempotent() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    // First ack — records a timestamp.
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    let content_after_first = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let first_doc = parser::parse(&content_after_first).unwrap();
    let first_cm = first_doc.find_comment("abc").unwrap();
    assert_eq!(first_cm.ack.len(), 1);
    let first_ts = first_cm.ack[0].ts;

    // Second ack by the same identity — should be a no-op (no new entry).
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    let content_after_second = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let second_doc = parser::parse(&content_after_second).unwrap();
    let second_cm = second_doc.find_comment("abc").unwrap();
    assert_eq!(
        second_cm.ack.len(),
        1,
        "second ack by same identity should not add a new entry",
    );
    assert_eq!(second_cm.ack[0].author, "eduardo");
    assert_eq!(
        second_cm.ack[0].ts, first_ts,
        "original timestamp should be preserved across re-acks",
    );
}

#[test]
fn ack_self_heals_duplicate_entries() {
    // Pre-dirty input: two acks from `alice` at different timestamps
    // (legacy buggy run). A subsequent ack should collapse them to
    // exactly one entry keyed on the first timestamp.
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
ack:
  - alice@2026-04-06T13:00:00-04:00
  - alice@2026-04-06T14:00:00-04:00
---
First comment.
```
";
    let system = system_with_doc(doc_content);
    let mut config = open_config();
    config.identity = Some(String::from("alice"));

    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    let alice_acks: Vec<_> = cm.ack.iter().filter(|a| a.author == "alice").collect();
    assert_eq!(
        alice_acks.len(),
        1,
        "duplicate acks for alice should be collapsed, got {:?}",
        cm.ack
    );
    assert_eq!(
        alice_acks[0].ts.to_rfc3339(),
        "2026-04-06T13:00:00-04:00",
        "first (earliest) ack timestamp should be preserved",
    );
}

#[test]
fn ack_noop_rewrites_file() {
    // The file should be rewritten every time `ack` runs, even when
    // the acting identity is already in the list. This keeps
    // `remargin_last_activity` and the frontmatter checksum fresh so
    // downstream inbox queries stay consistent.
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();

    // First ack — populates alice... er, eduardo — into the list.
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    // No-op ack (eduardo already present) — the ensure_frontmatter +
    // write_document tail must still run so remargin_last_activity
    // and the frontmatter checksum stay fresh.
    ack_comments(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    let second_content = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    // The frontmatter-level `remargin_last_activity` is recomputed on
    // every `ensure_frontmatter` call.
    assert!(second_content.contains("remargin_last_activity"));
    // And the ack block must remain a single entry for eduardo
    // (no duplicate push on no-op).
    let doc = parser::parse(&second_content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert_eq!(cm.ack.len(), 1);
    assert_eq!(cm.ack[0].author, "eduardo");
}

#[test]
fn ack_remove_after_dedup_clears_all_duplicates() {
    // Pre-dirty: alice has two ack entries. `ack --remove` as alice
    // should produce zero alice entries (dedup first, then strip).
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
ack:
  - alice@2026-04-06T13:00:00-04:00
  - alice@2026-04-06T14:00:00-04:00
---
First comment.
```
";
    let system = system_with_doc(doc_content);
    let mut config = open_config();
    config.identity = Some(String::from("alice"));

    ack_comments(&system, Path::new("/docs/test.md"), &config, &["abc"], true).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment("abc").unwrap();
    assert!(
        cm.ack.iter().all(|a| a.author != "alice"),
        "all duplicate alice acks should be gone after --remove, got {:?}",
        cm.ack
    );
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

#[test]
fn auto_ack_on_reply() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();
    let position = InsertPosition::Append;

    let _reply_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: true,
            content: "Reply with auto-ack.",
            position: &position,
            reply_to: Some("abc"),
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let parent = doc.find_comment("abc").unwrap();
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "eduardo");
}

#[test]
fn auto_ack_preserves_reply_to() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();
    let position = InsertPosition::Append;

    let reply_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: true,
            content: "Reply with auto-ack.",
            position: &position,
            reply_to: Some("abc"),
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&reply_id).unwrap();
    assert_eq!(reply.reply_to.as_deref(), Some("abc"));
    assert_eq!(reply.thread.as_deref(), Some("abc"));
}

#[test]
fn auto_ack_false_does_not_ack() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();
    let position = InsertPosition::Append;

    let _reply_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Reply without auto-ack.",
            position: &position,
            reply_to: Some("abc"),
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let parent = doc.find_comment("abc").unwrap();
    assert!(parent.ack.is_empty(), "parent should not be acked");
}

#[test]
fn auto_ack_without_reply_to_errors() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();
    let position = InsertPosition::Append;

    let result = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: true,
            content: "Top-level with auto-ack.",
            position: &position,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    );

    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(
        err_msg.contains("--auto-ack requires --reply-to"),
        "unexpected error: {err_msg}"
    );
}

#[test]
fn auto_ack_without_reply_to_no_file_modification() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();
    let position = InsertPosition::Append;

    let before = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let result = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: true,
            content: "Top-level with auto-ack.",
            position: &position,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    );

    result.unwrap_err();
    let after = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(before, after, "file should not be modified on error");
}

// ===========================================================================
// Reply-to auto-populate `to` tests (rem-3nm)
// ===========================================================================

#[test]
fn reply_auto_populates_to() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();
    let position = InsertPosition::Append;

    // Reply to "abc" (authored by "eduardo") without specifying `--to`.
    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Reply with auto-to.",
            position: &position,
            reply_to: Some("abc"),
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&new_id).unwrap();
    assert_eq!(
        reply.to,
        vec![String::from("eduardo")],
        "to should be auto-populated from parent author"
    );
}

#[test]
fn reply_explicit_to_not_overridden() {
    let system = system_with_doc(&doc_with_comment());
    let config = open_config();
    let position = InsertPosition::Append;

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Reply with explicit to.",
            position: &position,
            reply_to: Some("abc"),
            sandbox: false,
            to: &[String::from("bob")],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&new_id).unwrap();
    assert_eq!(
        reply.to,
        vec![String::from("bob")],
        "explicit to should not be overridden"
    );
}

#[test]
fn root_comment_no_auto_to() {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();
    let position = InsertPosition::Append;

    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Root comment, no to.",
            position: &position,
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment(&new_id).unwrap();
    assert!(cm.to.is_empty(), "root comment should have empty to");
}

#[test]
fn reply_auto_populates_to_different_author() {
    // Use the thread doc which has comments by "eduardo" and "alice".
    let system = system_with_doc(&doc_with_thread());
    let config = open_config();
    let position = InsertPosition::Append;

    // Reply to "child1" authored by "alice".
    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Reply to alice's comment.",
            position: &position,
            reply_to: Some("child1"),
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&new_id).unwrap();
    assert_eq!(
        reply.to,
        vec![String::from("alice")],
        "to should auto-populate from child1's author (alice)"
    );
}
