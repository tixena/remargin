//! Tests for comment operations.

use core::slice::from_ref;
use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::operations::{
    CreateCommentParams, ack_comments, create_comment, delete_comments, edit_comment, projections,
    react, sandbox as sandbox_ops, sign,
};
use crate::parser::{self, AuthorType};
use crate::writer::{FORBIDDEN_TARGETS, InsertPosition};

/// A minimal valid remargin document for testing.
const MINIMAL_DOC: &str = "\
---
title: Test
author: eduardo
---

# Test Document

Some body text.
";

/// Ed25519 test key used by the `project_sign` tests. Matched pair with
/// the public key registered for `eduardo` under `sign_config()` — keeps
/// the projection's signature output verifiable against the registry.
const PROJECT_SIGN_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
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
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
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
checksum: sha256:36bdfea47737fe3a6a75940e2f6eaed328a8a6e810c40f4c67fc68ae43047a98
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
checksum: sha256:4e8651a2833c950fa75cb2cf8ae7993a2b9be0d152981653d2c04e642916cfd9
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
checksum: sha256:d995882cea93224578ce6079bff70d0a280a0aff4957929bbc149e2af66d4c6f
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
        source_path: None,
        unrestricted: false,
    }
}

/// Create a mock system with a document file.
fn system_with_doc(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/docs/test.md"), content.as_bytes())
        .unwrap()
}

/// Config used by `project_sign` tests. Identity is `eduardo`, key is
/// wired to `/keys/ed25519`, and the registry maps `eduardo` to the
/// public half of [`PROJECT_SIGN_PRIVATE_KEY`]. Mode is `open` to keep
/// the verify gate neutral during fixture setup.
fn sign_config() -> ResolvedConfig {
    let public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin";
    let yaml = format!(
        "\
participants:
  eduardo:
    type: human
    status: active
    pubkeys:
      - {public_key}
  alice:
    type: human
    status: active
    pubkeys:
      - {public_key}
"
    );
    let registry = serde_yaml::from_str(&yaml).unwrap();
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: Some(PathBuf::from("/keys/ed25519")),
        mode: Mode::Open,
        registry: Some(registry),
        source_path: None,
        unrestricted: false,
    }
}

/// `MockSystem` seeded with a document and the matching signing key.
fn sign_system(doc: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap()
        .with_file(
            Path::new("/keys/ed25519"),
            PROJECT_SIGN_PRIVATE_KEY.as_bytes(),
        )
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
checksum: sha256:82f813dda444d58a0f0fcb5b097cdac27e01647eb2fefaf24972a1d554eb377f
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
checksum: sha256:959fab49e20f26dac6eb9d2599717216f2c6ff587e065d65cbc69508e1b2a1f7
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
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
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
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
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
fn reply_explicit_to_prepends_parent_author() {
    // Updated invariant (rem-kja): the parent author is always first
    // in `to:`; explicit `--to` entries are appended after it.
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
        vec![String::from("eduardo"), String::from("bob")],
        "parent author (eduardo) should be first, explicit bob appended",
    );
}

#[test]
fn reply_dedupes_parent_when_caller_includes_it() {
    // If the caller explicitly includes the parent author in `--to`,
    // it should be deduped (not doubled), with the parent still first
    // and other recipients preserved in input order.
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
            content: "Reply with parent re-listed in to.",
            position: &position,
            reply_to: Some("abc"),
            sandbox: false,
            to: &[String::from("eduardo"), String::from("bob")],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&new_id).unwrap();
    assert_eq!(
        reply.to,
        vec![String::from("eduardo"), String::from("bob")],
        "parent appears once, bob preserved in input order",
    );
}

#[test]
fn reply_with_multiple_extras_prepends_parent() {
    // Parent first, then all caller-supplied recipients in input order.
    let system = system_with_doc(&doc_with_thread());
    let config = open_config();
    let position = InsertPosition::Append;

    // Reply to "child1" (authored by "alice") with extras [bob, carol].
    let new_id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "Reply with multiple extras.",
            position: &position,
            reply_to: Some("child1"),
            sandbox: false,
            to: &[String::from("bob"), String::from("carol")],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let reply = doc.find_comment(&new_id).unwrap();
    assert_eq!(
        reply.to,
        vec![
            String::from("alice"),
            String::from("bob"),
            String::from("carol"),
        ],
        "parent alice first, then bob then carol in input order",
    );
}

#[test]
fn root_comment_preserves_explicit_to() {
    // When there's no `reply_to`, `effective_to` is just `params.to`
    // (no parent to prepend).
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
            content: "Root with to.",
            position: &position,
            reply_to: None,
            sandbox: false,
            to: &[String::from("bob")],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    let cm = doc.find_comment(&new_id).unwrap();
    assert_eq!(cm.to, vec![String::from("bob")]);
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

// --- Projection tests (rem-3uo) ---
//
// Each `project_*` helper returns a `(before, after)` pair suitable for
// feeding into `plan_ops::project_report`. These tests pin the invariant
// that projections never mutate disk and that their `after` doc matches
// what the paired mutating op would have written.

#[test]
fn project_ack_adds_ack_without_mutating_disk() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let (before, after) = projections::project_ack(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    // `before` must reflect the on-disk document exactly.
    assert_eq!(before.comments().len(), 1);
    assert!(before.find_comment("abc").unwrap().ack.is_empty());

    // `after` carries the projected ack.
    let after_comment = after.find_comment("abc").unwrap();
    assert_eq!(after_comment.ack.len(), 1);
    assert_eq!(after_comment.ack[0].author, "eduardo");

    // Disk must be unchanged.
    let after_disk = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(before_bytes, after_disk, "project_ack must not mutate disk");
}

#[test]
fn project_ack_missing_comment_surfaces_error() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();

    let err = projections::project_ack(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["does-not-exist"],
        false,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("not found"),
        "expected 'not found' error, got: {err}"
    );
}

#[test]
fn project_delete_removes_comment_without_mutating_disk() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let (before, after) =
        projections::project_delete(&system, Path::new("/docs/test.md"), &config, &["abc"])
            .unwrap();

    assert_eq!(before.comments().len(), 1);
    assert_eq!(after.comments().len(), 0);

    // Disk must be unchanged even though the projection removed a comment.
    let after_disk = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(
        before_bytes, after_disk,
        "project_delete must not mutate disk"
    );
}

#[test]
fn project_delete_missing_comment_surfaces_error() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();

    let err = projections::project_delete(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &["does-not-exist"],
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("not found"),
        "expected 'not found' error, got: {err}"
    );
}

#[test]
fn project_react_adds_reaction_without_mutating_disk() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let (before, after) = projections::project_react(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "abc",
        ":thumbsup:",
        false,
    )
    .unwrap();

    assert!(before.find_comment("abc").unwrap().reactions.is_empty());

    let after_reactions = &after.find_comment("abc").unwrap().reactions;
    assert_eq!(after_reactions.len(), 1);
    assert_eq!(
        after_reactions.get(":thumbsup:"),
        Some(&vec![String::from("eduardo")])
    );

    let after_disk = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(
        before_bytes, after_disk,
        "project_react must not mutate disk"
    );
}

#[test]
fn project_react_remove_is_idempotent_for_missing_emoji() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();

    let (_before, after) = projections::project_react(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "abc",
        ":heart:",
        true,
    )
    .unwrap();

    // Removing a reaction that was never set leaves the map empty.
    assert!(after.find_comment("abc").unwrap().reactions.is_empty());
}

#[test]
fn project_ack_matches_real_ack_comments_after_output() {
    // Property: project_ack's `after` document should be byte-identical
    // to what `ack_comments` writes to disk — modulo `ts` which is a
    // wall clock read. We compare structural invariants instead of raw
    // bytes.
    let seeded = doc_with_comment();

    // Real path.
    let system_real = system_with_doc(&seeded);
    let config = open_config();
    ack_comments(
        &system_real,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();
    let real_doc = parser::parse(
        &system_real
            .read_to_string(Path::new("/docs/test.md"))
            .unwrap(),
    )
    .unwrap();

    // Projection path against a fresh mock.
    let system_plan = system_with_doc(&seeded);
    let (_before, projected) = projections::project_ack(
        &system_plan,
        Path::new("/docs/test.md"),
        &config,
        &["abc"],
        false,
    )
    .unwrap();

    // The real writer and the projection should agree on comment ids,
    // content, and ack authors (the ts field is a wall-clock read, so we
    // don't compare it).
    assert_eq!(real_doc.comments().len(), projected.comments().len());
    let real_ack = &real_doc.find_comment("abc").unwrap().ack;
    let proj_ack = &projected.find_comment("abc").unwrap().ack;
    assert_eq!(real_ack.len(), proj_ack.len());
    assert_eq!(real_ack[0].author, proj_ack[0].author);
}

// --- project_comment / project_edit (rem-3fp) ---

#[test]
fn project_comment_appends_without_mutating_disk() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let params = projections::ProjectCommentParams::new("Projected body.", &InsertPosition::Append);
    let (before, after) =
        projections::project_comment(&system, Path::new("/docs/test.md"), &config, &params)
            .unwrap();

    assert_eq!(before.comments().len(), 1);
    assert_eq!(after.comments().len(), 2);

    // The appended comment carries the body, author from config, empty
    // ack list, and no signature (plan never signs).
    let new_cm = after
        .comments()
        .into_iter()
        .find(|cm| cm.id != "abc")
        .unwrap();
    assert_eq!(new_cm.content, "Projected body.");
    assert_eq!(new_cm.author, "eduardo");
    assert!(new_cm.ack.is_empty());
    assert!(new_cm.signature.is_none());

    // Disk untouched.
    let after_disk = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(
        before_bytes, after_disk,
        "project_comment must not mutate disk"
    );
}

#[test]
fn project_comment_auto_ack_without_reply_errors() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();

    let params = projections::ProjectCommentParams {
        attachment_filenames: &[],
        auto_ack: true,
        content: "bad params",
        position: &InsertPosition::Append,
        reply_to: None,
        sandbox: false,
        to: &[],
    };
    let err = projections::project_comment(&system, Path::new("/docs/test.md"), &config, &params)
        .unwrap_err();
    assert!(
        err.to_string().contains("--auto-ack requires --reply-to"),
        "expected auto-ack guard, got: {err}"
    );
}

#[test]
fn project_comment_reply_auto_acks_parent() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();

    let params = projections::ProjectCommentParams {
        attachment_filenames: &[],
        auto_ack: true,
        content: "reply body",
        position: &InsertPosition::AfterComment(String::from("abc")),
        reply_to: Some("abc"),
        sandbox: false,
        to: &[],
    };
    let (_before, after) =
        projections::project_comment(&system, Path::new("/docs/test.md"), &config, &params)
            .unwrap();

    // Parent (`abc`) now carries an ack from the acting identity.
    let parent = after.find_comment("abc").unwrap();
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "eduardo");

    // Reply carries reply_to + inherited thread.
    let reply = after
        .comments()
        .into_iter()
        .find(|cm| cm.id != "abc")
        .unwrap();
    assert_eq!(reply.reply_to.as_deref(), Some("abc"));
    assert_eq!(reply.thread.as_deref(), Some("abc"));
}

#[test]
fn project_comment_attachments_are_not_copied() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();

    let attach = ["photo.png"];
    let params = projections::ProjectCommentParams {
        attachment_filenames: &attach,
        auto_ack: false,
        content: "with fake attachment",
        position: &InsertPosition::Append,
        reply_to: None,
        sandbox: false,
        to: &[],
    };
    let (_before, after) =
        projections::project_comment(&system, Path::new("/docs/test.md"), &config, &params)
            .unwrap();

    let new_cm = after
        .comments()
        .into_iter()
        .find(|cm| cm.id != "abc")
        .unwrap();
    assert_eq!(new_cm.attachments, vec![String::from("assets/photo.png")]);

    // Assets dir must not exist on disk (plan is pure).
    assert!(
        !system.exists(Path::new("/docs/assets")).unwrap_or(false),
        "project_comment must not create the assets directory"
    );
    assert!(
        !system
            .exists(Path::new("/docs/assets/photo.png"))
            .unwrap_or(false),
        "project_comment must not copy attachment bytes"
    );
}

#[test]
fn project_edit_recomputes_checksum_and_clears_ack() {
    // Start from a doc where `abc` already has an ack so we can observe
    // edit's cascading clear.
    let seeded = "---\ntitle: Test\nauthor: eduardo\n---\n\n# Body\n\n```remargin\n---\nid: abc\nauthor: eduardo\ntype: human\nts: 2026-04-06T12:00:00-04:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\nack:\n  - alice@2026-04-06T13:00:00-04:00\n---\nFirst comment.\n```\n";
    let system = system_with_doc(seeded);
    let config = open_config();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let (before, after) = projections::project_edit(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "abc",
        "Edited body.",
    )
    .unwrap();

    assert_eq!(before.find_comment("abc").unwrap().ack.len(), 1);

    let edited = after.find_comment("abc").unwrap();
    assert_eq!(edited.content, "Edited body.");
    assert_ne!(
        edited.checksum,
        before.find_comment("abc").unwrap().checksum
    );
    assert!(edited.ack.is_empty(), "edit must clear acks on the target");
    assert!(
        edited.signature.is_none(),
        "edit must clear signature on the target"
    );

    let after_disk = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(
        before_bytes, after_disk,
        "project_edit must not mutate disk"
    );
}

#[test]
fn project_edit_missing_comment_surfaces_error() {
    let seeded = doc_with_comment();
    let system = system_with_doc(&seeded);
    let config = open_config();

    let err = projections::project_edit(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "does-not-exist",
        "whatever",
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not found"),
        "expected 'not found' error, got: {err}"
    );
}

#[test]
fn project_edit_cascades_ack_clear_to_descendants() {
    let seeded = doc_with_thread();
    let system = system_with_doc(&seeded);
    let config = open_config();

    // Sanity: root and child1 both start with an ack.
    let parsed_before = parser::parse(&seeded).unwrap();
    assert!(!parsed_before.find_comment("root").unwrap().ack.is_empty());
    assert!(!parsed_before.find_comment("child1").unwrap().ack.is_empty());

    let (_before, after) = projections::project_edit(
        &system,
        Path::new("/docs/test.md"),
        &config,
        "root",
        "Rewritten root.",
    )
    .unwrap();

    // Root's content was changed — ack cleared.
    assert!(after.find_comment("root").unwrap().ack.is_empty());
    // child1 is a descendant of root — ack cleared via the cascade.
    assert!(after.find_comment("child1").unwrap().ack.is_empty());
}

// --------------------------------------------------------------------
// project_batch / project_purge / project_migrate / project_sandbox_*
// (rem-qll): composite + destructive ops that sit on top of the
// lightweight projections.
// --------------------------------------------------------------------

/// Seed a two-comment document by creating each comment via the real
/// `create_comment` helper so checksums and frontmatter match what
/// `verify` expects (same pattern as [`seed_with_comment`] below,
/// extended to produce a second comment).
fn seed_two_comments() -> (MockSystem, ResolvedConfig, String, String) {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();
    let pos = InsertPosition::Append;

    let first = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams::new("First comment.", &pos),
    )
    .unwrap();
    let second = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams::new("Second comment.", &pos),
    )
    .unwrap();
    (system, config, first, second)
}

/// Seed a one-comment document via the real `create_comment` helper
/// so subsequent plan projections see a verify-clean baseline.
fn seed_with_comment() -> (MockSystem, ResolvedConfig, String) {
    let system = system_with_doc(MINIMAL_DOC);
    let config = open_config();
    let pos = InsertPosition::Append;
    let id = create_comment(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &CreateCommentParams::new("First comment.", &pos),
    )
    .unwrap();
    (system, config, id)
}

#[test]
fn project_batch_applies_sub_ops_in_order_without_mutating_disk() {
    let (system, config, _first) = seed_with_comment();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let ops = vec![
        projections::ProjectBatchOp::new(String::from("Second.")),
        projections::ProjectBatchOp::new(String::from("Third.")),
    ];
    let (before, after) =
        projections::project_batch(&system, Path::new("/docs/test.md"), &config, &ops).unwrap();

    // Sanity on the before/after pair.
    assert_eq!(before.comments().len(), 1);
    assert_eq!(after.comments().len(), 3);

    // Disk must be untouched.
    let after_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(before_bytes, after_bytes, "plan batch must not write disk");
}

#[test]
fn project_batch_auto_ack_without_reply_rejects_with_index() {
    let (system, config, _first) = seed_with_comment();

    let mut op = projections::ProjectBatchOp::new(String::from("Missing reply_to."));
    op.auto_ack = true;
    let err = projections::project_batch(&system, Path::new("/docs/test.md"), &config, &[op])
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("sub-op 0"),
        "rejection must name the failing sub-op index: {msg}"
    );
}

#[test]
fn project_batch_reply_auto_acks_parent() {
    let (system, config, first) = seed_with_comment();

    let mut reply = projections::ProjectBatchOp::new(String::from("Reply."));
    reply.reply_to = Some(first.clone());
    reply.auto_ack = true;
    let (_before, after) =
        projections::project_batch(&system, Path::new("/docs/test.md"), &config, &[reply]).unwrap();

    let parent = after.find_comment(&first).unwrap();
    assert!(
        parent.ack.iter().any(|a| a.author == "eduardo"),
        "parent must be auto-acked by the plan projection"
    );
}

#[test]
fn project_purge_strips_every_comment_without_writing_disk() {
    let (system, config, _first, _second) = seed_two_comments();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let (before, after) =
        projections::project_purge(&system, Path::new("/docs/test.md"), &config).unwrap();

    assert_eq!(before.comments().len(), 2);
    assert!(
        after.comments().is_empty(),
        "project_purge must strip every comment"
    );

    let after_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(before_bytes, after_bytes, "plan purge must not write disk");
}

#[test]
fn project_migrate_no_op_when_no_legacy_comments() {
    use crate::operations::migrate::MigrateIdentities;
    let (system, config, _first) = seed_with_comment();

    let (before, after) = projections::project_migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
    )
    .unwrap();

    assert_eq!(
        before.to_markdown(),
        after.to_markdown(),
        "migrate with no legacy comments must be a noop"
    );
}

#[test]
fn project_migrate_converts_legacy_markers_to_comments() {
    use crate::operations::migrate::MigrateIdentities;
    let content = "\
# Test

```agent comments [done:2026-04-05]
Agent response from the before-times.
```
";
    let system = system_with_doc(content);
    let config = open_config();

    let (before, after) = projections::project_migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
    )
    .unwrap();

    assert!(
        !before.legacy_comments().is_empty(),
        "fixture must parse as at least one legacy comment"
    );
    assert!(
        after.legacy_comments().is_empty(),
        "every legacy marker must be converted"
    );
    assert_eq!(
        after.comments().len(),
        1,
        "one legacy marker must produce one comment"
    );
    let new_cm = &after.comments()[0];
    assert_eq!(new_cm.author, "legacy-agent");
    assert!(
        !new_cm.ack.is_empty(),
        "`[done:DATE]` must produce an ack entry"
    );
}

#[test]
fn project_sandbox_add_projects_frontmatter_entry() {
    let (system, config, _first) = seed_with_comment();
    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();

    let (_before, after) =
        projections::project_sandbox_add(&system, Path::new("/docs/test.md"), &config).unwrap();

    assert!(
        after.to_markdown().contains("sandbox:"),
        "sandbox-add projection must rewrite the frontmatter"
    );
    let after_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(
        before_bytes, after_bytes,
        "plan sandbox-add must not write disk"
    );
}

#[test]
fn project_sandbox_add_is_idempotent_second_call_is_noop() {
    let (system, config, _first) = seed_with_comment();

    // First projection — not idempotent against the on-disk doc yet.
    let (_b1, _a1) =
        projections::project_sandbox_add(&system, Path::new("/docs/test.md"), &config).unwrap();

    // Actually stage the sandbox on disk so the second projection sees
    // an existing entry.
    sandbox_ops::add_to_files(
        &system,
        &[PathBuf::from("/docs/test.md")],
        "eduardo",
        &config,
    )
    .unwrap();

    let (before, after) =
        projections::project_sandbox_add(&system, Path::new("/docs/test.md"), &config).unwrap();
    assert_eq!(
        before.to_markdown(),
        after.to_markdown(),
        "second sandbox-add must project a noop when an entry already exists"
    );
}

#[test]
fn project_sandbox_remove_clears_entry_without_writing_disk() {
    let (system, config, _first) = seed_with_comment();
    sandbox_ops::add_to_files(
        &system,
        &[PathBuf::from("/docs/test.md")],
        "eduardo",
        &config,
    )
    .unwrap();

    let before_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert!(before_bytes.contains("sandbox:"));

    let (before, after) =
        projections::project_sandbox_remove(&system, Path::new("/docs/test.md"), &config).unwrap();
    assert!(before.to_markdown().contains("sandbox:"));
    assert!(
        !after.to_markdown().contains("sandbox:"),
        "sandbox-remove must strip the frontmatter key when it was the only entry"
    );

    let after_bytes = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert_eq!(
        before_bytes, after_bytes,
        "plan sandbox-remove must not write disk"
    );
}

#[test]
fn project_sandbox_add_rejects_non_markdown_path() {
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.txt"), b"not markdown")
        .unwrap();
    let config = open_config();

    let err = projections::project_sandbox_add(&system, Path::new("/docs/test.txt"), &config)
        .unwrap_err();
    assert!(
        err.to_string().contains("not a markdown file"),
        "expected `not a markdown file` error, got: {err}"
    );
}

// ---- project_sign ---------------------------------------------------------
// Exercises the `plan sign` projection added under rem-7y3. Unlike the
// other `project_*` helpers, project_sign deliberately loads the signing
// key because its whole purpose is the signature — a projection that
// skipped key loading would produce misleading `noop: true` plans.

/// Two-comment document: eduardo's note + alice's note, both unsigned,
/// checksums pre-computed so the verify gate stays neutral.
fn two_author_doc_for_sign() -> String {
    use crate::crypto;
    let eduardo_content = "eduardo's note";
    let alice_content = "alice's note";
    let eduardo_cksum = crypto::compute_checksum(eduardo_content, &[]);
    let alice_cksum = crypto::compute_checksum(alice_content, &[]);
    format!(
        "\
---
title: Test
---

# Doc

```remargin
---
id: ed1
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: {eduardo_cksum}
---
{eduardo_content}
```

```remargin
---
id: al1
author: alice
type: human
ts: 2026-04-06T13:00:00-04:00
checksum: {alice_cksum}
---
{alice_content}
```
"
    )
}

#[test]
fn project_sign_all_mine_signs_only_own_comments() {
    let system = sign_system(&two_author_doc_for_sign());
    let config = sign_config();

    let (before, after) = projections::project_sign(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &sign::SignSelection::AllMine,
    )
    .unwrap();

    // Before: neither is signed.
    let before_ed = before
        .segments
        .iter()
        .find_map(|s| match s {
            parser::Segment::Comment(c) if c.id == "ed1" => Some(c),
            parser::Segment::Body(_)
            | parser::Segment::Comment(_)
            | parser::Segment::LegacyComment(_) => None,
        })
        .unwrap();
    assert!(before_ed.signature.is_none());

    // After: eduardo's comment is signed, alice's is untouched.
    let after_ed = after
        .segments
        .iter()
        .find_map(|s| match s {
            parser::Segment::Comment(c) if c.id == "ed1" => Some(c),
            parser::Segment::Body(_)
            | parser::Segment::Comment(_)
            | parser::Segment::LegacyComment(_) => None,
        })
        .unwrap();
    let after_al = after
        .segments
        .iter()
        .find_map(|s| match s {
            parser::Segment::Comment(c) if c.id == "al1" => Some(c),
            parser::Segment::Body(_)
            | parser::Segment::Comment(_)
            | parser::Segment::LegacyComment(_) => None,
        })
        .unwrap();
    assert!(
        after_ed.signature.is_some(),
        "eduardo's comment must be signed under --all-mine"
    );
    assert!(
        after_al.signature.is_none(),
        "alice's comment must remain untouched under --all-mine"
    );
}

#[test]
fn project_sign_ids_signs_only_listed() {
    let system = sign_system(&two_author_doc_for_sign());
    let config = sign_config();

    let (_, after) = projections::project_sign(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &sign::SignSelection::Ids(vec![String::from("ed1")]),
    )
    .unwrap();

    let after_ed = after
        .segments
        .iter()
        .find_map(|s| match s {
            parser::Segment::Comment(c) if c.id == "ed1" => Some(c),
            parser::Segment::Body(_)
            | parser::Segment::Comment(_)
            | parser::Segment::LegacyComment(_) => None,
        })
        .unwrap();
    assert!(after_ed.signature.is_some());
}

#[test]
fn project_sign_ids_rejects_foreign_author() {
    let system = sign_system(&two_author_doc_for_sign());
    let config = sign_config();

    let err = projections::project_sign(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &sign::SignSelection::Ids(vec![String::from("al1")]),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("al1") || msg.to_lowercase().contains("author"),
        "forgery guard must fire for a foreign-authored id; got: {msg}"
    );
}

#[test]
fn project_sign_ids_unknown_errors_out() {
    let system = sign_system(&two_author_doc_for_sign());
    let config = sign_config();

    let err = projections::project_sign(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &sign::SignSelection::Ids(vec![String::from("does-not-exist")]),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("does-not-exist"),
        "unknown id must surface in the error; got: {err}"
    );
}

#[test]
fn project_sign_missing_key_bails() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/test.md"),
            two_author_doc_for_sign().as_bytes(),
        )
        .unwrap();
    let mut config = sign_config();
    config.key_path = None;

    let err = projections::project_sign(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &sign::SignSelection::AllMine,
    )
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("key"),
        "missing key must surface in the error; got: {err}"
    );
}

#[test]
fn project_sign_already_signed_stays_preserved() {
    // Build a doc that already has a signed comment by running
    // project_sign once, then feed its output back into project_sign to
    // check that --all-mine leaves the signature untouched.
    let system1 = sign_system(&two_author_doc_for_sign());
    let config = sign_config();
    let (_, pre_signed) = projections::project_sign(
        &system1,
        Path::new("/docs/test.md"),
        &config,
        &sign::SignSelection::AllMine,
    )
    .unwrap();
    let pre_signed_md = pre_signed.to_markdown();

    let system2 = sign_system(&pre_signed_md);
    let (_, after) = projections::project_sign(
        &system2,
        Path::new("/docs/test.md"),
        &config,
        &sign::SignSelection::AllMine,
    )
    .unwrap();

    // Signatures should be byte-identical on the second projection.
    let first_sig = pre_signed
        .segments
        .iter()
        .find_map(|s| match s {
            parser::Segment::Comment(c) if c.id == "ed1" => c.signature.clone(),
            parser::Segment::Body(_)
            | parser::Segment::Comment(_)
            | parser::Segment::LegacyComment(_) => None,
        })
        .unwrap();
    let second_sig = after
        .segments
        .iter()
        .find_map(|s| match s {
            parser::Segment::Comment(c) if c.id == "ed1" => c.signature.clone(),
            parser::Segment::Body(_)
            | parser::Segment::Comment(_)
            | parser::Segment::LegacyComment(_) => None,
        })
        .unwrap();
    assert_eq!(
        first_sig, second_sig,
        "already-signed comment must keep its signature under re-sign"
    );
}

// ---------------------------------------------------------------------
// Writer ban (rem-is4z): every mutating operation must refuse to touch
// `.remargin.yaml` / `.remargin-registry.yaml` — the canonical config
// and participant registry files. Each subcommand is exercised here
// against both forbidden basenames; the resulting error message must
// include "refusing to modify" and the basename, and the file contents
// must stay byte-identical on refusal.
// ---------------------------------------------------------------------

// The authoritative basename list lives at [`crate::writer::FORBIDDEN_TARGETS`].
// Tests below iterate over that same slice so adding a new forbidden
// file in one place automatically extends coverage here.

fn assert_forbidden_ops_error(err: &anyhow::Error, basename: &str) {
    let msg = format!("{err:#}");
    assert!(
        msg.contains("refusing to modify") && msg.contains(basename),
        "expected refusing-to-modify error for {basename}, got: {msg}"
    );
}

fn system_with_forbidden(basename: &str, contents: &[u8]) -> (MockSystem, PathBuf) {
    let path = PathBuf::from(format!("/docs/{basename}"));
    let system = MockSystem::new().with_file(&path, contents).unwrap();
    (system, path)
}

#[test]
fn comment_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let position = InsertPosition::Append;
        let err = create_comment(
            &system,
            &path,
            &config,
            &CreateCommentParams::new("hello", &position),
        )
        .unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn edit_comment_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let err = edit_comment(&system, &path, &config, "abc", "new body").unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn delete_comments_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let err = delete_comments(&system, &path, &config, &["abc"]).unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn ack_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let err = ack_comments(&system, &path, &config, &["abc"], false).unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn react_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let err = react(&system, &path, &config, "abc", "thumbsup", false).unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn batch_refuses_forbidden_targets() {
    use crate::operations::batch::{BatchCommentOp, batch_comment};
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let ops = vec![BatchCommentOp {
            after_comment: None,
            after_line: None,
            attachments: Vec::new(),
            auto_ack: false,
            content: String::from("one"),
            reply_to: None,
            to: Vec::new(),
        }];
        let err = batch_comment(&system, &path, &config, &ops).unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn sign_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = sign_config();

        let err = sign::sign_comments(
            &system,
            &path,
            &config,
            &sign::SignSelection::AllMine,
            sign::SignOptions::default(),
        )
        .unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn purge_refuses_forbidden_targets() {
    use crate::operations::purge::purge;
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let err = purge(&system, &path, &config).unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn migrate_refuses_forbidden_targets() {
    use crate::operations::migrate::{MigrateIdentities, migrate};
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let err = migrate(
            &system,
            &path,
            &config,
            &MigrateIdentities::default(),
            false,
        )
        .unwrap_err();

        assert_forbidden_ops_error(&err, basename);
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn sandbox_add_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let result =
            sandbox_ops::add_to_files(&system, from_ref(&path), "eduardo", &config).unwrap();

        assert_eq!(result.changed.len(), 0);
        assert_eq!(result.failed.len(), 1);
        assert!(
            result.failed[0].reason.contains("refusing to modify")
                && result.failed[0].reason.contains(*basename),
            "expected refusing-to-modify in per-file failure, got: {}",
            result.failed[0].reason
        );
        assert_eq!(before, read_file(&system, &path));
    }
}

#[test]
fn sandbox_remove_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let (system, path) = system_with_forbidden(basename, b"anything: true\n");
        let before = read_file(&system, &path);
        let config = open_config();

        let result =
            sandbox_ops::remove_from_files(&system, from_ref(&path), "eduardo", &config).unwrap();

        assert_eq!(result.changed.len(), 0);
        assert_eq!(result.failed.len(), 1);
        assert!(
            result.failed[0].reason.contains("refusing to modify")
                && result.failed[0].reason.contains(*basename),
            "expected refusing-to-modify in per-file failure, got: {}",
            result.failed[0].reason
        );
        assert_eq!(before, read_file(&system, &path));
    }
}

fn read_file(system: &MockSystem, path: &Path) -> Vec<u8> {
    use std::io::Read as _;
    let mut reader = system.open(path).unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).unwrap();
    buf
}
