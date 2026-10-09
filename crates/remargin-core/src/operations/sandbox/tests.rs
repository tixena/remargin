//! Tests for sandbox frontmatter operations.

use core::time::Duration;
use std::path::{Path, PathBuf};
use std::thread;

use chrono::DateTime;
use os_shim::System as _;
use os_shim::mock::MemorySystem;

use crate::config::{Mode, ResolvedConfig};
use crate::frontmatter;
use crate::operations::sandbox::{
    SandboxBulkResult, SandboxFailure, add_to_files, list_for_identity, remove_from_files,
    scan_all_entries,
};
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
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn simple_doc() -> &'static str {
    "\
---
title: Sample
---

# Sample

Body text.
"
}

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

/// A document holding a comment with a real checksum. `signature:` is omitted so the verify
/// status stays neutral in open mode.
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

fn write_file(system: &MemorySystem, path: &str, content: &str) {
    if let Some(parent) = Path::new(path).parent() {
        system.create_dir_all(parent).unwrap();
    }
    system.write(Path::new(path), content.as_bytes()).unwrap();
}

fn read_file(system: &MemorySystem, path: &str) -> String {
    system.read_to_string(Path::new(path)).unwrap()
}

#[test]
fn parse_sandbox_entry_success() {
    let entry = parser::parse_sandbox_entry("alice@2026-04-11T12:00:00+00:00").unwrap();
    assert_eq!(entry.author, "alice");
    assert_eq!(
        entry.ts,
        DateTime::parse_from_rfc3339("2026-04-11T12:00:00+00:00").unwrap()
    );
}

/// `+00:00` and `Z` read back as the same instant, and a `+00:00` entry is rewritten as `Z`.
#[test]
fn sandbox_entry_round_trips_legacy_zero_offset_to_z() {
    let legacy = parser::parse_sandbox_entry("alice@2026-04-11T12:00:00+00:00").unwrap();
    let modern = parser::parse_sandbox_entry("alice@2026-04-11T12:00:00Z").unwrap();
    assert_eq!(legacy.ts, modern.ts);
    assert_eq!(
        parser::format_sandbox_entry(&legacy),
        "alice@2026-04-11T12:00:00Z"
    );
    assert_eq!(
        parser::format_sandbox_entry(&legacy),
        parser::format_sandbox_entry(&modern)
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

#[test]
fn add_to_new_file_adds_entry() {
    let system = MemorySystem::new();
    write_file(&system, "/docs/a.md", simple_doc());

    let files = vec![PathBuf::from("/docs/a.md")];
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    assert_eq!(result.changed.len(), 1);
    assert_eq!(result.skipped, [] as [PathBuf; 0]);
    assert!(result.failed.is_empty());

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("sandbox:"));
    assert!(content.contains("eduardo@"));
}

/// Re-adding the same identity advances its timestamp while the roster stays at one entry.
#[test]
fn add_refreshes_timestamp_on_repeat() {
    let system = MemorySystem::new();
    write_file(&system, "/docs/a.md", simple_doc());

    let files = vec![PathBuf::from("/docs/a.md")];
    add_to_files(&system, &files, "eduardo", &open_config()).unwrap();
    let first = read_file(&system, "/docs/a.md");

    // Sleep a millisecond so wall-clock ts definitely advances; the
    // second add then refreshes the recorded timestamp.
    thread::sleep(Duration::from_millis(2));
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();
    assert_eq!(result.changed.len(), 1);
    assert_eq!(result.skipped, [] as [PathBuf; 0]);

    let second = read_file(&system, "/docs/a.md");
    assert_ne!(
        first, second,
        "second add must refresh the timestamp and rewrite the file"
    );
    assert_eq!(second.matches("eduardo@").count(), 1);
}

#[test]
fn add_multi_identity_preserves_existing_entries() {
    let system = MemorySystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());

    let files = vec![PathBuf::from("/docs/a.md")];
    add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@2026-04-11T12:00:00Z"));
    assert!(content.contains("eduardo@"));
}

#[test]
fn add_rejects_non_markdown_file() {
    let system = MemorySystem::new();
    write_file(&system, "/tmp/foo.txt", "not markdown");

    let files = vec![PathBuf::from("/tmp/foo.txt")];
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    assert_eq!(result.changed, [] as [PathBuf; 0]);
    assert_eq!(result.failed.len(), 1);
    assert!(result.failed[0].reason.contains("not a markdown file"));

    let content = read_file(&system, "/tmp/foo.txt");
    assert_eq!(content, "not markdown", "must not mutate rejected files");
}

#[test]
fn add_partial_failure_best_effort() {
    let system = MemorySystem::new();
    write_file(&system, "/docs/a.md", simple_doc());
    write_file(&system, "/docs/c.md", simple_doc());

    let files = vec![
        PathBuf::from("/docs/a.md"),
        PathBuf::from("/docs/b.md"),
        PathBuf::from("/docs/c.md"),
    ];
    let result = add_to_files(&system, &files, "eduardo", &open_config()).unwrap();

    assert_eq!(result.changed.len(), 2);
    assert_eq!(result.failed.len(), 1);
    assert_eq!(result.failed[0].path, PathBuf::from("/docs/b.md"));

    assert!(read_file(&system, "/docs/a.md").contains("eduardo@"));
    assert!(read_file(&system, "/docs/c.md").contains("eduardo@"));
}

#[test]
fn remove_last_entry_deletes_key() {
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());
    let files = vec![PathBuf::from("/docs/a.md")];

    add_to_files(&system, &files, "eduardo", &open_config()).unwrap();
    let result = remove_from_files(&system, &files, "eduardo", &open_config()).unwrap();
    assert_eq!(result.changed.len(), 1);

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@2026-04-11T12:00:00Z"));
    assert!(!content.contains("eduardo@"));
}

#[test]
fn remove_noop_when_no_entry() {
    let system = MemorySystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());
    let files = vec![PathBuf::from("/docs/a.md")];

    let result = remove_from_files(&system, &files, "eduardo", &open_config()).unwrap();
    assert_eq!(result.changed, [] as [PathBuf; 0]);
    assert_eq!(result.skipped.len(), 1);

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@2026-04-11T12:00:00+00:00"));
}

#[test]
fn remove_does_not_touch_other_identity_entries() {
    let system = MemorySystem::new();
    write_file(&system, "/docs/a.md", doc_with_jorge());

    let result = remove_from_files(
        &system,
        &[PathBuf::from("/docs/a.md")],
        "eduardo",
        &open_config(),
    )
    .unwrap();
    assert_eq!(result.changed, [] as [PathBuf; 0]);

    let content = read_file(&system, "/docs/a.md");
    assert!(content.contains("jorge@"));
}

#[test]
fn list_walks_and_filters_by_identity() {
    let system = MemorySystem::new();
    write_file(&system, "/root/a.md", simple_doc());
    write_file(&system, "/root/nested/b.md", simple_doc());
    write_file(&system, "/root/nested/c.md", simple_doc());
    write_file(&system, "/root/nested/d.md", doc_with_jorge());

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

    let listings =
        list_for_identity(&system, Path::new("/root"), "eduardo", &open_config()).unwrap();
    let paths: Vec<&Path> = listings.iter().map(|l| l.path.as_path()).collect();

    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&Path::new("/root/a.md")));
    assert!(paths.contains(&Path::new("/root/nested/b.md")));
}

#[test]
fn list_filters_jorge_returns_jorge_only_files() {
    let system = MemorySystem::new();
    write_file(&system, "/root/shared.md", doc_with_jorge());
    add_to_files(
        &system,
        &[PathBuf::from("/root/shared.md")],
        "eduardo",
        &open_config(),
    )
    .unwrap();

    let jorge = list_for_identity(&system, Path::new("/root"), "jorge", &open_config()).unwrap();
    let eduardo =
        list_for_identity(&system, Path::new("/root"), "eduardo", &open_config()).unwrap();

    assert_eq!(jorge.len(), 1);
    assert_eq!(eduardo.len(), 1);
    assert_eq!(jorge[0].path, Path::new("/root/shared.md"));
    assert_eq!(eduardo[0].path, Path::new("/root/shared.md"));
}

#[test]
fn scan_all_entries_enumerates_every_identity() {
    let system = MemorySystem::new();
    write_file(&system, "/root/shared.md", doc_with_jorge());
    write_file(&system, "/root/nested/b.md", simple_doc());
    write_file(
        &system,
        "/root/notes.txt",
        "sandbox: ghost@2026-01-01T00:00:00+00:00",
    );

    add_to_files(
        &system,
        &[
            PathBuf::from("/root/shared.md"),
            PathBuf::from("/root/nested/b.md"),
        ],
        "eduardo",
        &open_config(),
    )
    .unwrap();

    let scanned = scan_all_entries(&system, Path::new("/root")).unwrap();

    assert_eq!(scanned.len(), 3, "unexpected: {scanned:#?}");
    let pairs: Vec<(&str, &Path)> = scanned
        .iter()
        .map(|e| (e.author.as_str(), e.path.as_path()))
        .collect();
    assert!(pairs.contains(&("jorge", Path::new("/root/shared.md"))));
    assert!(pairs.contains(&("eduardo", Path::new("/root/shared.md"))));
    assert!(pairs.contains(&("eduardo", Path::new("/root/nested/b.md"))));
}

#[test]
fn scan_all_entries_carries_author_and_timestamp() {
    let system = MemorySystem::new();
    write_file(&system, "/root/d.md", doc_with_jorge());

    let scanned = scan_all_entries(&system, Path::new("/root")).unwrap();
    assert_eq!(scanned.len(), 1);
    assert_eq!(scanned[0].author, "jorge");
    assert_eq!(
        scanned[0].since,
        DateTime::parse_from_rfc3339("2026-04-11T12:00:00+00:00").unwrap()
    );
}

#[test]
fn sandbox_mutation_preserves_signed_comment_payload() {
    let system = MemorySystem::new();
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

#[test]
fn frontmatter_sandbox_round_trip() {
    let doc = parser::parse(doc_with_jorge()).unwrap();
    let entries = frontmatter::read_sandbox_entries(&doc).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].author, "jorge");
}

#[test]
fn bulk_result_to_json_strips_base_dir_and_uses_changed_key() {
    let result = SandboxBulkResult {
        changed: vec![PathBuf::from("/docs/a.md")],
        failed: vec![SandboxFailure {
            path: PathBuf::from("/docs/c.md"),
            reason: String::from("denied"),
        }],
        skipped: vec![PathBuf::from("/docs/b.md")],
    };

    let value = result.to_json(Path::new("/docs"), "added");
    assert_eq!(value["added"], serde_json::json!(["a.md"]));
    assert_eq!(value["skipped"], serde_json::json!(["b.md"]));
    assert_eq!(
        value["failed"],
        serde_json::json!([{ "path": "c.md", "reason": "denied" }]),
    );
}

#[test]
fn bulk_result_to_json_renders_paths_outside_base_dir_verbatim() {
    let result = SandboxBulkResult {
        changed: vec![PathBuf::from("/elsewhere/out.md")],
        failed: Vec::new(),
        skipped: Vec::new(),
    };
    let value = result.to_json(Path::new("/docs"), "removed");
    assert_eq!(value["removed"], serde_json::json!(["/elsewhere/out.md"]));
}
