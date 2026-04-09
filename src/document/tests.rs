//! Tests for the document access layer.

use std::io::Read as _;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::document::{self, WriteOptions, allowlist};
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
        unrestricted: false,
    }
}

fn read_bytes(system: &MockSystem, path: &Path) -> Vec<u8> {
    let mut reader = system.open(path).unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).unwrap();
    buf
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
        unrestricted: false,
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

#[test]
fn allowlist_is_text_pen() {
    assert!(allowlist::is_text(Path::new("design.pen")));
}

#[test]
fn allowlist_is_text_pen_uppercase() {
    assert!(allowlist::is_text(Path::new("design.PEN")));
}

#[test]
fn allowlist_is_text_png_still_false() {
    assert!(!allowlist::is_text(Path::new("image.png")));
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

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        false,
        false,
    )
    .unwrap();
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

    let result = document::get(
        &system,
        Path::new("/project"),
        Path::new(".env"),
        None,
        false,
        false,
    );
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

    let result = document::get(
        &system,
        Path::new("/project"),
        Path::new("main.rs"),
        None,
        false,
        false,
    );
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
        false,
        false,
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
        false,
        false,
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

    let meta =
        document::metadata(&system, Path::new("/project"), Path::new("doc.md"), false).unwrap();
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
        WriteOptions::default(),
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
        WriteOptions::default(),
    );
    result.unwrap_err();
}

// ---------------------------------------------------------------------------
// Write --create tests
// ---------------------------------------------------------------------------

// Test: create new file succeeds
#[test]
fn write_create_new_file() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/docs"))
        .unwrap();

    let config = open_config();
    let content = "---\ntitle: New Document\n---\n\n# New Document\n\nSome content.\n";

    document::write(
        &system,
        Path::new("/project"),
        Path::new("docs/new.md"),
        content,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system
        .read_to_string(Path::new("/project/docs/new.md"))
        .unwrap();
    assert!(result.contains("New Document"));
}

// Test: create fails if file already exists
#[test]
fn write_create_rejects_existing_file() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# Existing")
        .unwrap();

    let config = open_config();

    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "# Overwrite attempt",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("already exists"),
        "expected 'already exists' error, got: {err}"
    );
}

// Test: create fails if parent directory does not exist
#[test]
fn write_create_rejects_missing_parent() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();

    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("nonexistent/dir/new.md"),
        "# New",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    );
    result.unwrap_err();
}

// Test: create fails if path escapes sandbox
#[test]
fn write_create_rejects_escape() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/other"))
        .unwrap();

    let config = open_config();

    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("../../other/new.md"),
        "# Escape attempt",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    );
    result.unwrap_err();
}

// Test: create rejects non-visible extensions
#[test]
fn write_create_rejects_disallowed_extension() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();

    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("script.rs"),
        "fn main() {}",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    );
    result.unwrap_err();
}

// Test: create rejects dotfiles
#[test]
fn write_create_rejects_dotfile() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();

    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new(".secret.md"),
        "# Hidden",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    );
    result.unwrap_err();
}

// ---------------------------------------------------------------------------
// Write --raw tests
// ---------------------------------------------------------------------------

#[test]
fn write_raw_pen_file() {
    let raw_json = r#"{"nodes":[{"id":"abc"}]}"#;
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/design.pen"), b"{}")
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("design.pen"),
        raw_json,
        &config,
        WriteOptions {
            raw: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system
        .read_to_string(Path::new("/project/design.pen"))
        .unwrap();
    assert_eq!(result, raw_json);
}

#[test]
fn write_raw_json_file() {
    let raw_content = r#"{"key": "value"}"#;
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/data.json"), b"{}")
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("data.json"),
        raw_content,
        &config,
        WriteOptions {
            raw: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system
        .read_to_string(Path::new("/project/data.json"))
        .unwrap();
    assert_eq!(result, raw_content);
}

#[test]
fn write_raw_create_new_file() {
    let raw_content = r#"{"created": true}"#;
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("new.json"),
        raw_content,
        &config,
        WriteOptions {
            create: true,
            raw: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system
        .read_to_string(Path::new("/project/new.json"))
        .unwrap();
    assert_eq!(result, raw_content);
}

#[test]
fn write_raw_overwrites_without_comment_check() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(
            Path::new("/project/design.pen"),
            DOC_WITH_COMMENTS.as_bytes(),
        )
        .unwrap();

    let config = open_config();
    let raw_content = "completely different content";
    document::write(
        &system,
        Path::new("/project"),
        Path::new("design.pen"),
        raw_content,
        &config,
        WriteOptions {
            raw: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system
        .read_to_string(Path::new("/project/design.pen"))
        .unwrap();
    assert_eq!(result, raw_content);
}

#[test]
fn write_raw_rejected_for_markdown() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# Hello")
        .unwrap();

    let config = open_config();
    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "raw content",
        &config,
        WriteOptions {
            raw: true,
            ..Default::default()
        },
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string()
            .contains("raw mode is not supported for markdown files"),
        "expected raw mode error, got: {err}"
    );
}

#[test]
fn write_default_still_adds_frontmatter() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let content = r#"{"nodes":[]}"#;
    document::write(
        &system,
        Path::new("/project"),
        Path::new("design.pen"),
        content,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system
        .read_to_string(Path::new("/project/design.pen"))
        .unwrap();
    // Non-raw write adds frontmatter, so content should differ from raw input.
    assert!(
        result.contains("---"),
        "expected frontmatter injection, got: {result}"
    );
}

// ---------------------------------------------------------------------------
// Write --binary tests
// ---------------------------------------------------------------------------

#[test]
fn write_binary_png_content() {
    // Minimal valid PNG header bytes.
    let png_bytes: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    let b64 = BASE64_STANDARD.encode(png_bytes);

    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("image.png"),
        &b64,
        &config,
        WriteOptions {
            binary: true,
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let on_disk = read_bytes(&system, Path::new("/project/image.png"));
    assert_eq!(on_disk.as_slice(), png_bytes);
}

#[test]
fn write_binary_implies_raw() {
    let content_bytes = b"binary content here";
    let b64 = BASE64_STANDARD.encode(content_bytes);

    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("data.json"),
        &b64,
        &config,
        WriteOptions {
            binary: true,
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let on_disk = read_bytes(&system, Path::new("/project/data.json"));
    // No frontmatter should be added since binary implies raw.
    assert_eq!(on_disk.as_slice(), content_bytes);
}

#[test]
fn write_binary_create_new_file() {
    let content_bytes = b"new binary file";
    let b64 = BASE64_STANDARD.encode(content_bytes);

    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("output.png"),
        &b64,
        &config,
        WriteOptions {
            binary: true,
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let on_disk = read_bytes(&system, Path::new("/project/output.png"));
    assert_eq!(on_disk.as_slice(), content_bytes);
}

#[test]
fn write_binary_with_raw_flag() {
    let content_bytes = b"binary takes precedence";
    let b64 = BASE64_STANDARD.encode(content_bytes);

    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("output.png"),
        &b64,
        &config,
        WriteOptions {
            binary: true,
            create: true,
            raw: true,
        },
    )
    .unwrap();

    let on_disk = read_bytes(&system, Path::new("/project/output.png"));
    assert_eq!(on_disk.as_slice(), content_bytes);
}

#[test]
fn write_binary_rejected_for_markdown() {
    let b64 = BASE64_STANDARD.encode(b"binary md");

    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# Hello")
        .unwrap();

    let config = open_config();
    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &b64,
        &config,
        WriteOptions {
            binary: true,
            ..Default::default()
        },
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string()
            .contains("binary mode is not supported for markdown files"),
        "expected binary mode error, got: {err}"
    );
}

#[test]
fn write_binary_invalid_base64() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/image.png"), b"PNG")
        .unwrap();

    let config = open_config();
    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("image.png"),
        "not-valid-base64!!!@@@",
        &config,
        WriteOptions {
            binary: true,
            ..Default::default()
        },
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("invalid base64"),
        "expected base64 error, got: {err}"
    );
}

#[test]
fn write_non_binary_still_text() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "# Hello\n\nWorld",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    // Normal text write adds frontmatter.
    assert!(
        result.contains("---"),
        "expected frontmatter, got: {result}"
    );
    assert!(result.contains("Hello"));
}

// ---------------------------------------------------------------------------
// Path sandbox tests: sandboxed (unrestricted = false)
// ---------------------------------------------------------------------------

// Note: MockSystem::canonicalize does not resolve `..` components, so parent
// traversal tests use absolute paths (which canonicalize handles correctly).

#[test]
fn sandbox_blocks_absolute_escape() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/home/user/other.md"), b"# Other")
        .unwrap();

    let err = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/home/user/other.md"),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("path escapes sandbox"),
        "expected 'path escapes sandbox', got: {err}"
    );
}

#[test]
fn sandbox_create_blocks_absolute_escape() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/other"))
        .unwrap();

    let err = allowlist::resolve_sandboxed_create(
        &system,
        Path::new("/project"),
        Path::new("/other/new.md"),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("path escapes sandbox"),
        "expected 'path escapes sandbox', got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Path sandbox tests: sandboxed (paths within sandbox)
// ---------------------------------------------------------------------------

#[test]
fn sandbox_allows_child_path() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/src"))
        .unwrap()
        .with_file(Path::new("/project/src/main.rs"), b"fn main() {}")
        .unwrap();

    let result = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("src/main.rs"),
        false,
    )
    .unwrap();
    assert_eq!(result, Path::new("/project/src/main.rs"));
}

#[test]
fn sandbox_create_allows_child() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/src"))
        .unwrap();

    let result = allowlist::resolve_sandboxed_create(
        &system,
        Path::new("/project"),
        Path::new("src/new.md"),
        false,
    )
    .unwrap();
    assert_eq!(result, Path::new("/project/src/new.md"));
}

// ---------------------------------------------------------------------------
// Path sandbox tests: unrestricted mode (unrestricted = true)
// ---------------------------------------------------------------------------

#[test]
fn unrestricted_allows_absolute_path() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/home/user/file.md"), b"# Hello")
        .unwrap();

    let result = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/home/user/file.md"),
        true,
    )
    .unwrap();
    assert_eq!(result, Path::new("/home/user/file.md"));
}

#[test]
fn unrestricted_allows_relative_within_sandbox() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/src"))
        .unwrap()
        .with_file(Path::new("/project/src/main.rs"), b"fn main() {}")
        .unwrap();

    let result = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("src/main.rs"),
        true,
    )
    .unwrap();
    assert_eq!(result, Path::new("/project/src/main.rs"));
}

#[test]
fn unrestricted_create_allows_absolute_escape() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/other"))
        .unwrap();

    let result = allowlist::resolve_sandboxed_create(
        &system,
        Path::new("/project"),
        Path::new("/other/new.md"),
        true,
    )
    .unwrap();
    assert_eq!(result, Path::new("/other/new.md"));
}

#[test]
fn unrestricted_create_absolute() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/tmp"))
        .unwrap();

    let result = allowlist::resolve_sandboxed_create(
        &system,
        Path::new("/project"),
        Path::new("/tmp/new.md"),
        true,
    )
    .unwrap();
    assert_eq!(result, Path::new("/tmp/new.md"));
}

#[test]
fn sandboxed_absolute_blocked_but_unrestricted_allows() {
    // Same path: sandboxed blocks it, unrestricted allows it.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/home/user/notes.md"), b"# Notes")
        .unwrap();

    // Sandboxed: blocked.
    let err = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/home/user/notes.md"),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("path escapes sandbox"),
        "expected 'path escapes sandbox', got: {err}"
    );

    // Unrestricted: allowed.
    let result = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/home/user/notes.md"),
        true,
    )
    .unwrap();
    assert_eq!(result, Path::new("/home/user/notes.md"));
}

// ---------------------------------------------------------------------------
// get --line-numbers tests
// ---------------------------------------------------------------------------

#[test]
fn line_numbers_full_file() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(
            Path::new("/project/doc.md"),
            b"alpha\nbeta\ngamma\ndelta\nepsilon",
        )
        .unwrap();

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(
        content,
        "1\u{2502} alpha\n2\u{2502} beta\n3\u{2502} gamma\n4\u{2502} delta\n5\u{2502} epsilon"
    );
}

#[test]
fn line_numbers_with_range() {
    let lines: Vec<String> = (1_i32..=100_i32).map(|i| format!("line{i}")).collect();
    let file_content = lines.join("\n");

    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), file_content.as_bytes())
        .unwrap();

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        Some((50, 55)),
        true,
        false,
    )
    .unwrap();
    assert_eq!(
        content,
        "50\u{2502} line50\n51\u{2502} line51\n52\u{2502} line52\n53\u{2502} line53\n54\u{2502} line54\n55\u{2502} line55"
    );
}

#[test]
fn line_numbers_padding() {
    let lines: Vec<String> = (1_i32..=1_000_i32).map(|i| format!("line{i}")).collect();
    let file_content = lines.join("\n");

    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), file_content.as_bytes())
        .unwrap();

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        Some((998, 1000)),
        true,
        false,
    )
    .unwrap();
    assert_eq!(
        content,
        " 998\u{2502} line998\n 999\u{2502} line999\n1000\u{2502} line1000"
    );
}

#[test]
fn line_numbers_off_by_default() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"hello\nworld")
        .unwrap();

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        false,
        false,
    )
    .unwrap();
    assert_eq!(content, "hello\nworld");
}

#[test]
fn line_numbers_single_line() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"only line")
        .unwrap();

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(content, "1\u{2502} only line");
}

#[test]
fn line_numbers_empty_lines() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"first\n\nthird")
        .unwrap();

    let content = document::get(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(content, "1\u{2502} first\n2\u{2502} \n3\u{2502} third");
}

#[test]
fn line_numbers_binary_rejected() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/image.png"), b"\x89PNG\r\n")
        .unwrap();

    let result = document::get(
        &system,
        Path::new("/project"),
        Path::new("image.png"),
        None,
        true,
        false,
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not supported for binary"),
        "expected binary error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// rm tests
// ---------------------------------------------------------------------------

#[test]
fn rm_deletes_existing_file() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/notes.md"), b"# Notes")
        .unwrap();

    let config = open_config();
    let result = document::rm(
        &system,
        Path::new("/project"),
        Path::new("notes.md"),
        &config,
    )
    .unwrap();

    assert!(result.existed);
    system
        .read_to_string(Path::new("/project/notes.md"))
        .unwrap_err();
}

#[test]
fn rm_idempotent_missing_file() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let result = document::rm(
        &system,
        Path::new("/project"),
        Path::new("nonexistent.md"),
        &config,
    )
    .unwrap();

    assert!(!result.existed);
}

#[test]
fn rm_rejects_hidden_file() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/.secret"), b"hidden")
        .unwrap();

    let config = open_config();
    let result = document::rm(
        &system,
        Path::new("/project"),
        Path::new(".secret"),
        &config,
    );

    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("not visible"),
        "expected visibility error"
    );
}

#[test]
fn rm_rejects_path_outside_sandbox() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/etc/passwd"), b"root:x:0:0")
        .unwrap();

    let config = open_config();
    let result = document::rm(
        &system,
        Path::new("/project"),
        Path::new("../../etc/passwd"),
        &config,
    );

    result.unwrap_err();
}

#[test]
fn rm_rejects_directory() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project/subdir"))
        .unwrap();

    let config = open_config();
    let result = document::rm(&system, Path::new("/project"), Path::new("subdir"), &config);

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("cannot remove directory"),
        "expected directory error"
    );
}
