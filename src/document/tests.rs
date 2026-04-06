//! Tests for the document access layer.

use std::path::Path;

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::document::{self, allowlist};
use crate::parser::AuthorType;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// A markdown document with comments for metadata testing.
const DOC_WITH_COMMENTS: &str = "\
---
title: Test
---

# Test

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:aaa
---
First comment.
```

```remargin
---
id: def
author: alice
type: human
ts: 2026-04-06T13:00:00-04:00
checksum: sha256:bbb
ack:
  - eduardo@2026-04-06T14:00:00-04:00
---
Second comment (acked).
```
";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn config_with_ignore(patterns: Vec<String>) -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: patterns,
        key_path: None,
        mode: Mode::Open,
        registry: None,
    }
}

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

// ---------------------------------------------------------------------------
// Allowlist unit tests
// ---------------------------------------------------------------------------

#[test]
fn allowlist_is_text_md() {
    assert!(allowlist::is_text(Path::new("doc.md")));
}

#[test]
fn allowlist_is_text_png() {
    assert!(!allowlist::is_text(Path::new("image.png")));
}

#[test]
fn allowlist_is_visible_directory() {
    assert!(allowlist::is_visible(Path::new("src"), true));
}

#[test]
fn allowlist_is_visible_dotfile() {
    assert!(!allowlist::is_visible(Path::new(".env"), false));
}

#[test]
fn allowlist_is_visible_md() {
    assert!(allowlist::is_visible(Path::new("doc.md"), false));
}

#[test]
fn allowlist_is_visible_rs() {
    assert!(!allowlist::is_visible(Path::new("main.rs"), false));
}

// ---------------------------------------------------------------------------
// ls tests
// ---------------------------------------------------------------------------

// Test 1: ls visible files
#[test]
fn ls_visible_files() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/project/readme.md"), b"# Hello")
        .unwrap()
        .with_file(Path::new("/project/image.png"), b"PNG")
        .unwrap()
        .with_file(Path::new("/project/.env"), b"SECRET=123")
        .unwrap();

    let config = open_config();
    let entries = document::ls(&system, Path::new("/project"), Path::new("."), &config).unwrap();

    let names: Vec<String> = entries
        .iter()
        .map(|e| e.path.display().to_string())
        .collect();
    assert!(names.contains(&String::from("readme.md")));
    assert!(names.contains(&String::from("image.png")));
    assert!(!names.contains(&String::from(".env")));
}

// Test 2: ls dot-directory hidden
#[test]
fn ls_dot_directory_hidden() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/.git"))
        .unwrap()
        .with_dir(Path::new("/project/src"))
        .unwrap();

    let config = open_config();
    let entries = document::ls(&system, Path::new("/project"), Path::new("."), &config).unwrap();

    let names: Vec<String> = entries
        .iter()
        .map(|e| e.path.display().to_string())
        .collect();
    assert!(names.contains(&String::from("src")));
    assert!(!names.contains(&String::from(".git")));
}

// Test 3: ls ignore patterns
#[test]
fn ls_ignore_patterns() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/src"))
        .unwrap()
        .with_dir(Path::new("/project/node_modules"))
        .unwrap();

    let config = config_with_ignore(vec![String::from("node_modules")]);
    let entries = document::ls(&system, Path::new("/project"), Path::new("."), &config).unwrap();

    let names: Vec<String> = entries
        .iter()
        .map(|e| e.path.display().to_string())
        .collect();
    assert!(names.contains(&String::from("src")));
    assert!(!names.contains(&String::from("node_modules")));
}

// ---------------------------------------------------------------------------
// get tests
// ---------------------------------------------------------------------------

// Test 5: get markdown
#[test]
fn get_markdown() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# Hello\nWorld")
        .unwrap();

    let content = document::get(&system, Path::new("/project"), Path::new("doc.md"), None).unwrap();
    assert_eq!(content, "# Hello\nWorld");
}

// Test 7: get dotfile -- not visible
#[test]
fn get_dotfile_hidden() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/.env"), b"SECRET=123")
        .unwrap();

    let result = document::get(&system, Path::new("/project"), Path::new(".env"), None);
    result.unwrap_err();
}

// Test 8: get disallowed extension
#[test]
fn get_disallowed_extension() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/main.rs"), b"fn main() {}")
        .unwrap();

    let result = document::get(&system, Path::new("/project"), Path::new("main.rs"), None);
    result.unwrap_err();
}

// Test 9: get --lines
#[test]
fn get_with_lines() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(
            Path::new("/project/doc.md"),
            b"line1\nline2\nline3\nline4\nline5",
        )
        .unwrap();

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        Some((2, 4)),
    )
    .unwrap();
    assert_eq!(content, "line2\nline3\nline4");
}

// Test 10: get escape attempt
#[test]
fn get_escape_attempt() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/etc/passwd"), b"root:x:0:0")
        .unwrap();

    let result = document::get(
        &system,
        Path::new("/project"),
        Path::new("../../etc/passwd"),
        None,
    );
    result.unwrap_err();
}

// ---------------------------------------------------------------------------
// metadata tests
// ---------------------------------------------------------------------------

// Test 14: metadata
#[test]
fn metadata_correct_counts() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let meta = document::metadata(&system, Path::new("/project"), Path::new("doc.md")).unwrap();
    assert_eq!(meta.comment_count, 2);
    assert_eq!(meta.pending_count, 1); // abc is unacked
    assert_eq!(meta.pending_for, vec!["alice"]); // abc has to: [alice]
    assert!(meta.last_activity.is_some());
    assert!(meta.frontmatter.is_some());
}

// ---------------------------------------------------------------------------
// Write tests
// ---------------------------------------------------------------------------

// Test 11: write preserves comments
#[test]
fn write_preserves_comments() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

    // Modify body text but keep comments intact.
    let modified = DOC_WITH_COMMENTS.replace("# Test", "# Updated Test");
    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &modified,
        &config,
    )
    .unwrap();

    let result = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(result.contains("Updated Test"));
}

// Test 12: write missing comment -- rejected
#[test]
fn write_missing_comment_rejected() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

    // Content with one comment removed.
    let stripped = "\
---
title: Test
---

# Test

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:aaa
---
First comment.
```
";
    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        stripped,
        &config,
    );
    result.unwrap_err();
}
