//! Tests for the purge operation.

use std::path::Path;

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::operations::purge::{purge, purge_dir};
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

// Note: per-op `--dry-run` was removed in; `plan purge` covers
// that preview path now.

// ---------------------------------------------------------------------
// Layer 1 op-guard wiring — purge is the
// representative integration. The follow-up ticket wires the remaining
// mutating ops; the op_guard helper itself is exhaustively tested under
// `permissions::op_guard::tests`.
// ---------------------------------------------------------------------

#[test]
fn purge_refused_when_target_outside_allow_list() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: elsewhere\n";
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

// ---------------------------------------------------------------------
// Directory purge. Recursive `purge --recursive` walks a
// directory and applies a per-file op_guard check + purge to every
// visible `.md` file under it. Per-file refusals never abort the
// rest of the walk.
// ---------------------------------------------------------------------

#[test]
fn purge_dir_purges_every_md_file() {
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/a.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/b.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/c.md"), doc_with_comments().as_bytes())
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert_eq!(result.purged.len(), 3);
    assert!(result.failed.is_empty());
    assert!(result.skipped.is_empty());
    assert_eq!(result.comments_removed_total(), 6);

    for name in ["a.md", "b.md", "c.md"] {
        let path = Path::new("/realm").join(name);
        let content = system.read_to_string(&path).unwrap();
        let doc = parser::parse(&content).unwrap();
        assert!(
            doc.comments().is_empty(),
            "{name} should have zero comments after recursive purge"
        );
    }
}

#[test]
fn purge_dir_skips_non_markdown_files() {
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/a.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/keep.txt"), b"plain text body")
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert_eq!(result.purged.len(), 1, "only a.md is markdown");
    assert_eq!(result.purged[0].path, Path::new("/realm/a.md"));
    // Plain-text file untouched.
    let txt = system.read_to_string(Path::new("/realm/keep.txt")).unwrap();
    assert_eq!(txt, "plain text body");
}

#[test]
fn purge_dir_empty_dir_is_noop() {
    let system = MockSystem::new().with_dir(Path::new("/empty")).unwrap();

    let result = purge_dir(&system, Path::new("/empty"), &open_config()).unwrap();

    assert!(result.purged.is_empty());
    assert!(result.failed.is_empty());
    assert!(result.skipped.is_empty());
}

#[test]
fn purge_dir_zero_md_files_is_noop() {
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/a.txt"), b"plain text")
        .unwrap()
        .with_file(Path::new("/realm/b.json"), b"{\"k\":\"v\"}")
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert!(result.purged.is_empty());
    assert!(result.failed.is_empty());
    assert!(result.skipped.is_empty());
}

#[test]
fn purge_dir_missing_directory_errors() {
    let system = MockSystem::new();
    let err = purge_dir(&system, Path::new("/nope"), &open_config()).unwrap_err();
    assert!(
        format!("{err}").contains("does not exist"),
        "expected missing-dir error, got: {err}"
    );
}

#[test]
fn purge_dir_target_is_a_file_errors() {
    let system = MockSystem::new()
        .with_file(Path::new("/realm/a.md"), b"# header\n")
        .unwrap();
    let err = purge_dir(&system, Path::new("/realm/a.md"), &open_config()).unwrap_err();
    assert!(
        format!("{err}").contains("not a directory"),
        "expected not-a-directory error, got: {err}"
    );
}

#[test]
fn purge_dir_records_skipped_when_no_comments() {
    let plain = "---\ntitle: Plain\n---\n\n# Just text\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/a.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/b.md"), plain.as_bytes())
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert_eq!(result.purged.len(), 1, "only a.md had comments");
    assert_eq!(result.purged[0].path, Path::new("/realm/a.md"));
    assert_eq!(result.skipped.len(), 1, "b.md was a no-op skip");
    assert_eq!(result.skipped[0], Path::new("/realm/b.md"));
}

#[test]
fn purge_dir_partial_block_with_deny_ops() {
    // deny_ops blocks purge only on b.md; a.md should still be purged.
    let yaml = "permissions:\n  deny_ops:\n    - path: b.md\n      ops: [purge]\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/a.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/b.md"), doc_with_comments().as_bytes())
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert_eq!(result.purged.len(), 1, "a.md should be purged");
    assert_eq!(result.purged[0].path, Path::new("/realm/a.md"));
    assert_eq!(result.failed.len(), 1, "b.md should be refused");
    assert_eq!(result.failed[0].path, Path::new("/realm/b.md"));
    assert!(
        result.failed[0].reason.contains("denied by `deny_ops`"),
        "expected deny_ops refusal, got: {}",
        result.failed[0].reason
    );
}

#[test]
fn purge_dir_deny_ops_on_parent_blocks_every_file() {
    // deny_ops `path: .` covers every nested file via op_guard: every
    // file is refused with DeniedOp, no file is mutated.
    let yaml = "permissions:\n  deny_ops:\n    - path: .\n      ops: [purge]\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/a.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/b.md"), doc_with_comments().as_bytes())
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert!(result.purged.is_empty(), "no files should be purged");
    assert_eq!(result.failed.len(), 2);
    for failure in &result.failed {
        assert!(
            failure.reason.contains("denied by `deny_ops`"),
            "expected deny_ops refusal on {}, got: {}",
            failure.path.display(),
            failure.reason
        );
    }
    // Comments survive on disk.
    let a = system.read_to_string(Path::new("/realm/a.md")).unwrap();
    let doc = parser::parse(&a).unwrap();
    assert_eq!(doc.comments().len(), 2);
}

#[test]
fn purge_dir_skips_dot_folders() {
    // walk_dir(hidden=false) excludes dot-folders entirely; the .git
    // file should not be visited.
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_dir(Path::new("/realm/.git"))
        .unwrap()
        .with_file(
            Path::new("/realm/.git/log.md"),
            doc_with_comments().as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/realm/keep.md"), doc_with_comments().as_bytes())
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert_eq!(result.purged.len(), 1, "only keep.md should be purged");
    assert_eq!(result.purged[0].path, Path::new("/realm/keep.md"));
    // Dot-folder file untouched.
    let log_content = system
        .read_to_string(Path::new("/realm/.git/log.md"))
        .unwrap();
    let log_doc = parser::parse(&log_content).unwrap();
    assert_eq!(log_doc.comments().len(), 2, ".git/log.md should be ignored");
}

#[test]
fn purge_dir_recurses_into_subdirectories() {
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_dir(Path::new("/realm/notes"))
        .unwrap()
        .with_dir(Path::new("/realm/notes/sub"))
        .unwrap()
        .with_file(Path::new("/realm/a.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(
            Path::new("/realm/notes/b.md"),
            doc_with_comments().as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/realm/notes/sub/c.md"),
            doc_with_comments().as_bytes(),
        )
        .unwrap();

    let result = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();

    assert_eq!(result.purged.len(), 3, "should recurse into subdirs");
    let mut paths: Vec<_> = result.purged.iter().map(|p| p.path.clone()).collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            Path::new("/realm/a.md").to_path_buf(),
            Path::new("/realm/notes/b.md").to_path_buf(),
            Path::new("/realm/notes/sub/c.md").to_path_buf(),
        ]
    );
}

#[test]
fn purge_dir_replan_after_apply_is_noop() {
    // Apply -> re-walk: every file should land in `skipped` because
    // the comments are gone.
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/a.md"), doc_with_comments().as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/b.md"), doc_with_comments().as_bytes())
        .unwrap();

    // First pass: both purged.
    let first = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();
    assert_eq!(first.purged.len(), 2);

    // Second pass: both already comment-free -> skipped.
    let second = purge_dir(&system, Path::new("/realm"), &open_config()).unwrap();
    assert!(second.purged.is_empty(), "re-run should be a noop");
    assert_eq!(second.skipped.len(), 2);
}
