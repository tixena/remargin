//! Tests for the purge operation.

use std::path::Path;

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::operations::purge::purge;
use crate::parser::{self, AuthorType};

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

fn doc_with_comments() -> &'static str {
    "\
---
title: Test
remargin_pending: 2
remargin_pending_for:
  - alice
remargin_last_activity: 2026-04-06T13:00:00-04:00
---

# Test Document

Some text before.

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:aaa
---
First comment.
```

More text between.

```remargin
---
id: def
author: alice
type: human
ts: 2026-04-06T13:00:00-04:00
checksum: sha256:bbb
---
Second comment.
```

Text after.
"
}

#[test]
fn simple_purge() {
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc_with_comments().as_bytes())
        .unwrap();

    let config = open_config();
    let result = purge(&system, Path::new("/docs/test.md"), &config).unwrap();

    assert_eq!(result.comments_removed, 2);

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let doc = parser::parse(&content).unwrap();
    assert!(doc.comments().is_empty());
}

#[test]
fn body_text_preserved() {
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc_with_comments().as_bytes())
        .unwrap();

    let config = open_config();
    purge(&system, Path::new("/docs/test.md"), &config).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    assert!(content.contains("Some text before."));
    assert!(content.contains("More text between."));
    assert!(content.contains("Text after."));
}

#[test]
fn frontmatter_cleanup() {
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc_with_comments().as_bytes())
        .unwrap();

    let config = open_config();
    purge(&system, Path::new("/docs/test.md"), &config).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    // User field preserved.
    assert!(content.contains("title: Test"));
    // Remargin fields removed.
    assert!(!content.contains("remargin_pending"));
    assert!(!content.contains("remargin_pending_for"));
    assert!(!content.contains("remargin_last_activity"));
}

// Note: per-op `--dry-run` was removed in rem-0ry; `plan purge` covers
// that preview path now.

// ---------------------------------------------------------------------
// Layer 1 op-guard wiring (rem-yj1j.2 / T23) — purge is the
// representative integration. The follow-up ticket wires the remaining
// mutating ops; the op_guard helper itself is exhaustively tested under
// `permissions::op_guard::tests`.
// ---------------------------------------------------------------------

#[test]
fn purge_refused_when_target_outside_allow_list() {
    let yaml = "permissions:\n  restrict:\n    - path: elsewhere\n";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/docs/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let err = purge(&system, Path::new("/docs/test.md"), &open_config()).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("outside the allow-list"),
        "expected outside-allow-list refusal, got {chain}"
    );
}

#[test]
fn purge_refused_when_deny_ops_lists_purge() {
    let yaml = "permissions:\n  deny_ops:\n    - path: test.md\n      ops: [purge]\n";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/docs/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let err = purge(&system, Path::new("/docs/test.md"), &open_config()).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("denied by `deny_ops`"),
        "expected deny_ops refusal, got {chain}"
    );
}

#[test]
fn purge_allowed_when_deny_ops_lists_other_op() {
    let yaml = "permissions:\n  deny_ops:\n    - path: test.md\n      ops: [delete]\n";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/docs/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    purge(&system, Path::new("/docs/test.md"), &open_config()).unwrap();
}

#[test]
fn no_comments() {
    let plain = "---\ntitle: Plain\n---\n\n# Just text\n";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), plain.as_bytes())
        .unwrap();

    let config = open_config();
    let result = purge(&system, Path::new("/docs/test.md"), &config).unwrap();

    assert_eq!(result.comments_removed, 0);
}

#[test]
fn no_excessive_blank_lines() {
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc_with_comments().as_bytes())
        .unwrap();

    let config = open_config();
    purge(&system, Path::new("/docs/test.md"), &config).unwrap();

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    // Should not have 3+ consecutive newlines.
    assert!(
        !content.contains("\n\n\n"),
        "should not have triple newlines after purge"
    );
}
