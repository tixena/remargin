//! Tests for sandbox frontmatter operations.

use std::path::{Path, PathBuf};

use chrono::DateTime;
use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::frontmatter;
use crate::operations::sandbox::{add_to_files, list_for_identity, remove_from_files};
use crate::parser::{self, AuthorType};

/// Open-mode config used by every sandbox test that doesn't care about
/// verify-gate severity. Sandbox ops mutate frontmatter only, so the
/// post-write verify gate is neutral by construction in open mode.
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

/// A tiny markdown document with no sandbox state.
fn simple_doc() -> &'static str {
    "\
---
title: Sample
---

# Sample

Body text.
"
}

/// A markdown document with one existing sandbox entry for `jorge`.
fn doc_with_jorge() -> &'static str {
    "\
---
title: Sample
sandbox:
- jorge@2026-04-11T12:00:00+00:00
---

# Sample

Body.
"
}

/// A markdown document that already contains a remargin comment with a
/// real checksum we will later assert survives frontmatter mutation.
///
/// Note: a `signature:` field is intentionally omitted so the
/// `signature=missing` verify status is neutral under open mode. The test
/// still asserts the full signature byte (here `None`) survives the
/// sandbox round-trip.
fn doc_with_comment() -> &'static str {
    "\
---
title: Signed
---

# Signed

```remargin
---
id: aaa111
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:2d8bd7d9bb5f85ba643f0110d50cb506a1fe439e769a22503193ea6046bb87f7
---
Hello.
```
"
}

fn write_file(system: &MockSystem, path: &str, content: &str) {
    // `with_file` semantics: implicitly creates parent directories. We use
    // the corresponding `MockSystem::create_dir_all` call here so that
    // subsequent writes succeed.
    if let Some(parent) = Path::new(path).parent() {
        system.create_dir_all(parent).unwrap();
    }
    system.write(Path::new(path), content.as_bytes()).unwrap();
}

fn read_file(system: &MockSystem, path: &str) -> String {
    system.read_to_string(Path::new(path)).unwrap()
}

// ---------------------------------------------------------------------------
// parser::parse_sandbox_entry — the QA table's #1/#2/#3 cases.
// ---------------------------------------------------------------------------

#[test]
fn parse_sandbox_entry_success() {
    let entry = parser::parse_sandbox_entry("alice@2026-04-11T12:00:00+00:00").unwrap();
    assert_eq!(entry.author, "alice");
    assert_eq!(
        entry.ts,
        DateTime::parse_from_rfc3339("2026-04-11T12:00:00+00:00").unwrap()
    );
}

#[test]
fn parse_sandbox_entry_missing_at() {
    let err = parser::parse_sandbox_entry("alice").unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("missing '@'"), "unexpected error: {msg}");
}

#[test]
fn parse_sandbox_entry_bad_timestamp() {
    let err = parser::parse_sandbox_entry("alice@not-a-date").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("invalid sandbox timestamp"),
        "unexpected error: {msg}"
    );
}

// ---------------------------------------------------------------------------
// add_to_files
// ---------------------------------------------------------------------------

#[test]
fn add_to_new_file_adds_entry() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", simple_doc());

    let files = vec![PathBuf::from("/docs/a.md")];
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    assert_eq!(result.changed.len(), 1);
    assert!(result.skipped.is_empty());
    assert!(result.failed.is_empty());

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("sandbox:"));
    assert!(content.contains("eduardo@"));
}

#[test]
fn add_is_idempotent_and_preserves_timestamp() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", simple_doc());

    let files = vec![PathBuf::from("/docs/a.md")];
    add_to_files(&system, &files, "eduardo", &open_config()).unwrap();
    let first = read_file(&system, "/docs/a.md");

    // Second add must be a no-op.
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();
    assert!(result.changed.is_empty());
    assert_eq!(result.skipped.len(), 1);

    let second = read_file(&system, "/docs/a.md");
    assert_eq!(first, second, "idempotent re-add must not touch the file");
}

#[test]
fn add_multi_identity_preserves_existing_entries() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());

    let files = vec![PathBuf::from("/docs/a.md")];
    add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@2026-04-11T12:00:00+00:00"));
    assert!(content.contains("eduardo@"));
}

#[test]
fn add_rejects_non_markdown_file() {
    let system = MockSystem::new();
    write_file(&system, "/tmp/foo.txt", "not markdown");

    let files = vec![PathBuf::from("/tmp/foo.txt")];
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    assert!(result.changed.is_empty());
    assert_eq!(result.failed.len(), 1);
    assert!(result.failed[0].reason.contains("not a markdown file"));

    let content = read_file(&system, "/tmp/foo.txt");
    assert_eq!(content, "not markdown", "must not mutate rejected files");
}

#[test]
fn add_partial_failure_best_effort() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", simple_doc());
    write_file(&system, "/docs/c.md", simple_doc());

    // b.md is intentionally missing.
    let files = vec![
        PathBuf::from("/docs/a.md"),
        PathBuf::from("/docs/b.md"),
        PathBuf::from("/docs/c.md"),
    ];
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    assert_eq!(result.changed.len(), 2);
    assert_eq!(result.failed.len(), 1);
    assert_eq!(result.failed[0].path, PathBuf::from("/docs/b.md"));

    // a.md and c.md still mutated despite b.md failing.
    assert!(read_file(&system, "/docs/a.md").contains("eduardo@"));
    assert!(read_file(&system, "/docs/c.md").contains("eduardo@"));
}

// ---------------------------------------------------------------------------
// remove_from_files
// ---------------------------------------------------------------------------

#[test]
fn remove_last_entry_deletes_key() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", simple_doc());
    let files = vec![PathBuf::from("/docs/a.md")];
    add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    let result = remove_from_files(&system, &files, "eduardo", &open_config()).unwrap();
    assert_eq!(result.changed.len(), 1);

    let content = read_file(&system, "/docs/a.md");
    assert!(
        !content.contains("sandbox:"),
        "last entry should delete the key entirely, got:\n{content}",
    );
}

#[test]
fn remove_preserves_other_identities() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());
    let files = vec![PathBuf::from("/docs/a.md")];

    // Eduardo joins, then Eduardo leaves — jorge must still be there.
    add_to_files(&system, &files, "eduardo", &open_config()).unwrap();
    let result = remove_from_files(&system, &files, "eduardo", &open_config()).unwrap();
    assert_eq!(result.changed.len(), 1);

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@2026-04-11T12:00:00+00:00"));
    assert!(!content.contains("eduardo@"));
}

#[test]
fn remove_noop_when_no_entry() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());
    let files = vec![PathBuf::from("/docs/a.md")];

    // Eduardo removes even though only jorge is staged.
    let result = remove_from_files(&system, &files, "eduardo", &open_config()).unwrap();
    assert!(result.changed.is_empty());
    assert_eq!(result.skipped.len(), 1);

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@2026-04-11T12:00:00+00:00"));
}

#[test]
fn remove_does_not_touch_other_identity_entries() {
    let system = MockSystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());

    // Eduardo tries to remove jorge's entry — must be a no-op.
    let result = remove_from_files(
        &system,
        &[PathBuf::from("/docs/a.md")],
        "eduardo",
        &open_config(),
    )
    .unwrap();
    assert!(result.changed.is_empty());

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@"));
}

// ---------------------------------------------------------------------------
// list_for_identity
// ---------------------------------------------------------------------------

#[test]
fn list_walks_and_filters_by_identity() {
    let system = MockSystem::new();
    write_file(&system, "/root/a.md", simple_doc());
    write_file(&system, "/root/nested/b.md", simple_doc());
    write_file(&system, "/root/nested/c.md", simple_doc());
    write_file(&system, "/root/nested/d.md", doc_with_jorge());

    // Stage a.md and b.md as eduardo; d.md only has jorge.
    add_to_files(
        &system,
        &[
            PathBuf::from("/root/a.md"),
            PathBuf::from("/root/nested/b.md"),
        ],
        "eduardo",
        &open_config(),
    )
    .unwrap();

    let listings = list_for_identity(&system, Path::new("/root"), "eduardo").unwrap();
    let paths: Vec<&Path> = listings.iter().map(|l| l.path.as_path()).collect();

    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&Path::new("/root/a.md")));
    assert!(paths.contains(&Path::new("/root/nested/b.md")));
}

#[test]
fn list_filters_jorge_returns_jorge_only_files() {
    let system = MockSystem::new();
    write_file(&system, "/root/shared.md", doc_with_jorge());
    add_to_files(
        &system,
        &[PathBuf::from("/root/shared.md")],
        "eduardo",
        &open_config(),
    )
    .unwrap();

    let jorge = list_for_identity(&system, Path::new("/root"), "jorge").unwrap();
    let eduardo = list_for_identity(&system, Path::new("/root"), "eduardo").unwrap();

    assert_eq!(jorge.len(), 1);
    assert_eq!(eduardo.len(), 1);
    assert_eq!(jorge[0].path, Path::new("/root/shared.md"));
    assert_eq!(eduardo[0].path, Path::new("/root/shared.md"));
}

// ---------------------------------------------------------------------------
// Integrity: sandbox mutations must not invalidate existing signatures.
// Comment-level checksums and signatures do not include any document-level
// frontmatter, so the test asserts the serialized comment byte range is
// untouched after a sandbox add.
// ---------------------------------------------------------------------------

#[test]
fn sandbox_mutation_preserves_signed_comment_payload() {
    let system = MockSystem::new();
    write_file(&system, "/docs/signed.md", doc_with_comment());

    let before = read_file(&system, "/docs/signed.md");
    let before_doc = parser::parse(&before).unwrap();
    let before_comment = before_doc.comments()[0].clone();

    add_to_files(
        &system,
        &[PathBuf::from("/docs/signed.md")],
        "eduardo",
        &open_config(),
    )
    .unwrap();

    let after = read_file(&system, "/docs/signed.md");
    assert!(after.contains("sandbox:"));
    assert!(after.contains("eduardo@"));

    let after_doc = parser::parse(&after).unwrap();
    let after_comment = after_doc.comments()[0].clone();

    // Every field that participates in the signature payload is unchanged.
    assert_eq!(after_comment.id, before_comment.id);
    assert_eq!(after_comment.author, before_comment.author);
    assert_eq!(after_comment.author_type, before_comment.author_type);
    assert_eq!(after_comment.ts, before_comment.ts);
    assert_eq!(after_comment.to, before_comment.to);
    assert_eq!(after_comment.reply_to, before_comment.reply_to);
    assert_eq!(after_comment.thread, before_comment.thread);
    assert_eq!(after_comment.attachments, before_comment.attachments);
    assert_eq!(after_comment.content, before_comment.content);
    assert_eq!(after_comment.checksum, before_comment.checksum);
    assert_eq!(after_comment.signature, before_comment.signature);
}

// ---------------------------------------------------------------------------
// frontmatter::read_sandbox_entries / write_sandbox_entries round-trip.
// ---------------------------------------------------------------------------

#[test]
fn frontmatter_sandbox_round_trip() {
    let doc = parser::parse(doc_with_jorge()).unwrap();
    let entries = frontmatter::read_sandbox_entries(&doc).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].author, "jorge");
}
