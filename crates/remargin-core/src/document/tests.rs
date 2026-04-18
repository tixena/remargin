//! Tests for the document access layer.

use std::io::Read as _;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::document::{self, WriteOptions, WriteProjection, allowlist};
use crate::parser::AuthorType;

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
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
---
First comment.
```

```remargin
---
id: def
author: alice
type: human
ts: 2026-04-06T13:00:00-04:00
checksum: sha256:904b58fc3d0b777a58bd2afe36e349d24278364d74e63664923c3b826f997008
ack:
  - eduardo@2026-04-06T14:00:00-04:00
---
Second comment (acked).
```
";

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
    assert!(allowlist::is_visible(Path::new("main.rs"), false));
}

#[test]
fn allowlist_is_text_rs() {
    assert!(allowlist::is_text(Path::new("main.rs")));
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

#[test]
fn allowlist_source_code_extensions_visible_and_text() {
    // One representative extension per added language family.
    let cases = &[
        "main.rs",
        "main.ts",
        "main.tsx",
        "main.mts",
        "main.cts",
        "main.js",
        "main.mjs",
        "main.cjs",
        "main.jsx",
        "app.py",
        "App.java",
        "Main.kt",
        "Program.cs",
        "node.cpp",
        "vec.cc",
        "node.hpp",
        "header.h",
        "main.go",
        "app.rb",
        "index.php",
        "App.swift",
        "Main.scala",
        "deploy.sh",
        "activate.ps1",
        "schema.sql",
        "config.yaml",
        "Cargo.toml",
        "index.html",
        "style.css",
    ];
    for path in cases {
        assert!(
            allowlist::is_visible(Path::new(path), false),
            "expected {path} to be visible"
        );
        assert!(
            allowlist::is_text(Path::new(path)),
            "expected {path} to be text"
        );
    }
}

#[test]
fn allowlist_source_code_case_insensitive() {
    assert!(allowlist::is_visible(Path::new("Main.RS"), false));
    assert!(allowlist::is_text(Path::new("Main.RS")));
    assert!(allowlist::is_visible(Path::new("APP.PY"), false));
    assert!(allowlist::is_text(Path::new("APP.PY")));
}

#[test]
fn allowlist_unsupported_extension_not_visible() {
    assert!(!allowlist::is_visible(Path::new("foo.exe"), false));
}

#[test]
fn allowlist_dotfile_still_hidden_even_for_env() {
    // Named env/conf files are visible, but dotfiles are still hidden.
    assert!(!allowlist::is_visible(Path::new(".env"), false));
    assert!(allowlist::is_visible(Path::new("app.env"), false));
    assert!(allowlist::is_visible(Path::new("server.conf"), false));
}

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

#[test]
fn get_disallowed_extension() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/app.exe"), b"binary")
        .unwrap();

    let result = document::get(
        &system,
        Path::new("/project"),
        Path::new("app.exe"),
        None,
        false,
        false,
    );
    result.unwrap_err();
}

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

#[test]
fn metadata_correct_counts() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let meta =
        document::metadata(&system, Path::new("/project"), Path::new("doc.md"), false).unwrap();
    assert_eq!(meta.comment_count, Some(2));
    assert_eq!(meta.pending_count, Some(1)); // abc is unacked
    assert_eq!(meta.pending_for, vec!["alice"]); // abc has to: [alice]
    assert!(meta.last_activity.is_some());
    assert!(meta.frontmatter.is_some());
    assert!(!meta.binary);
    assert_eq!(meta.mime, "text/markdown");
    assert!(meta.line_count.is_some());
}

#[test]
fn metadata_binary_file_returns_file_level_fields_only() {
    // PNG file: is allowlisted, binary, no markdown parse step.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), &[0x89, b'P', b'N', b'G'])
        .unwrap();

    let meta =
        document::metadata(&system, Path::new("/project"), Path::new("pic.png"), false).unwrap();

    assert!(meta.binary);
    assert_eq!(meta.mime, "image/png");
    assert!(meta.path.ends_with("pic.png"));
    // Markdown-shaped fields must be absent for binary files.
    assert_eq!(meta.comment_count, None);
    assert_eq!(meta.line_count, None);
    assert_eq!(meta.pending_count, None);
    assert!(meta.pending_for.is_empty());
    assert!(meta.last_activity.is_none());
    assert!(meta.frontmatter.is_none());
}

#[test]
fn metadata_non_md_text_file_returns_markdown_fields() {
    // .txt is text/plain — still text, so we parse it (no comments expected).
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/notes.txt"), b"line one\nline two\n")
        .unwrap();

    let meta = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("notes.txt"),
        false,
    )
    .unwrap();

    assert!(!meta.binary);
    assert_eq!(meta.mime, "text/plain");
    assert_eq!(meta.comment_count, Some(0));
    assert_eq!(meta.pending_count, Some(0));
    assert!(meta.line_count.is_some());
}

#[test]
fn metadata_pdf_is_binary() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.pdf"), b"%PDF-1.4")
        .unwrap();

    let meta =
        document::metadata(&system, Path::new("/project"), Path::new("doc.pdf"), false).unwrap();

    assert!(meta.binary);
    assert_eq!(meta.mime, "application/pdf");
    assert_eq!(meta.comment_count, None);
}

#[test]
fn metadata_missing_file_errors() {
    let system = MockSystem::new().with_current_dir("/project").unwrap();

    let result = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("nonexistent.md"),
        false,
    );
    result.unwrap_err();
}

#[test]
fn read_binary_returns_bytes_and_mime() {
    let bytes: &[u8] = &[0x89, b'P', b'N', b'G', 1, 2, 3];
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), bytes)
        .unwrap();

    let payload =
        document::read_binary(&system, Path::new("/project"), Path::new("pic.png"), false).unwrap();

    assert_eq!(payload.bytes, bytes);
    assert_eq!(payload.mime, "image/png");
    assert!(payload.path.ends_with("pic.png"));
}

#[test]
fn read_binary_rejects_markdown() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# hi\n")
        .unwrap();

    let err = document::read_binary(&system, Path::new("/project"), Path::new("doc.md"), false)
        .unwrap_err();
    assert!(format!("{err}").contains("cannot fetch .md as binary"));
}

#[test]
fn read_binary_unknown_extension_is_octet_stream() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/blob.bin"), b"raw")
        .unwrap();

    // `.bin` is NOT allowlisted — this should error on visibility.
    let result =
        document::read_binary(&system, Path::new("/project"), Path::new("blob.bin"), false);
    result.unwrap_err();
}

#[test]
fn read_binary_rejects_dotfile() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/.env"), b"secret")
        .unwrap();

    let result = document::read_binary(&system, Path::new("/project"), Path::new(".env"), false);
    result.unwrap_err();
}

#[test]
fn read_binary_escape_attempt() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/etc/passwd"), b"root:x:0:0")
        .unwrap();

    let result = document::read_binary(
        &system,
        Path::new("/project"),
        Path::new("../../etc/passwd"),
        false,
    );
    result.unwrap_err();
}

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
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
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

#[test]
fn write_create_auto_creates_parent_dirs() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let content = "---\ntitle: Nested\n---\n\n# Nested Document\n\nContent here.\n";

    document::write(
        &system,
        Path::new("/project"),
        Path::new("newdir/subdir/file.md"),
        content,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    // File was created with correct content.
    let result = system
        .read_to_string(Path::new("/project/newdir/subdir/file.md"))
        .unwrap();
    assert!(result.contains("Nested Document"));

    // Parent directories were created.
    assert!(system.is_dir(Path::new("/project/newdir")).unwrap());
    assert!(system.is_dir(Path::new("/project/newdir/subdir")).unwrap());
}

#[test]
fn write_create_deeply_nested() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let content = "---\ntitle: Deep\n---\n\n# Deep Document\n";

    document::write(
        &system,
        Path::new("/project"),
        Path::new("a/b/c/d/file.md"),
        content,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let result = system
        .read_to_string(Path::new("/project/a/b/c/d/file.md"))
        .unwrap();
    assert!(result.contains("Deep Document"));

    // All intermediate directories exist.
    assert!(system.is_dir(Path::new("/project/a")).unwrap());
    assert!(system.is_dir(Path::new("/project/a/b")).unwrap());
    assert!(system.is_dir(Path::new("/project/a/b/c")).unwrap());
    assert!(system.is_dir(Path::new("/project/a/b/c/d")).unwrap());
}

#[test]
fn write_create_existing_parent_unchanged() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/project/docs"))
        .unwrap();

    let config = open_config();
    let content = "---\ntitle: Existing Parent\n---\n\n# Document\n";

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
    assert!(result.contains("Existing Parent"));
}

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
        Path::new("app.exe"),
        "binary",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    );
    result.unwrap_err();
}

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

#[test]
fn write_create_parent_outside_sandbox() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/"))
        .unwrap();

    let config = open_config();

    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("../../outside/file.md"),
        "# Escape",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("escapes sandbox"),
        "expected sandbox error, got: {err}"
    );
}

#[test]
fn write_no_create_missing_parent_still_fails() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();

    let result = document::write(
        &system,
        Path::new("/project"),
        Path::new("nonexistent/dir/doc.md"),
        "---\ntitle: Test\n---\n\n# Test\n",
        &config,
        WriteOptions::new(),
    );
    result.unwrap_err();
}

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
            lines: None,
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

// ---------------------------------------------------------------------
// Partial writes (rem-24p): `--lines START-END` replaces a range of
// lines in place, leaving every other byte identical. Comment blocks
// inside the range must be re-included by id, and the post-write verify
// gate still runs. Tests cover the happy path, preservation rejects,
// boundary conditions, and incompatibility with create/raw/binary.
// ---------------------------------------------------------------------

#[test]
fn splice_lines_replaces_single_line() {
    // Replacing one line with one line leaves the rest byte-identical.
    let out = document::splice_lines("A\nB\nC\nD\nE", 3, 3, "X");
    assert_eq!(out, "A\nB\nX\nD\nE");
}

#[test]
fn splice_lines_expanding_range_inserts_lines() {
    // Replacing one line with three lines grows the file by two lines.
    let out = document::splice_lines("A\nB\nC\nD\nE", 3, 3, "X\nY\nZ");
    assert_eq!(out, "A\nB\nX\nY\nZ\nD\nE");
}

#[test]
fn splice_lines_shrinking_range_removes_lines() {
    // Replacing three lines with one drops two lines net.
    let out = document::splice_lines("A\nB\nC\nD\nE", 2, 4, "Q");
    assert_eq!(out, "A\nQ\nE");
}

#[test]
fn splice_lines_strips_one_trailing_newline() {
    // `--lines 3-3 "X"` and `--lines 3-3 "X\n"` must behave identically,
    // so a single trailing newline is stripped before splicing.
    let out = document::splice_lines("A\nB\nC\nD", 3, 3, "X\n");
    assert_eq!(out, "A\nB\nX\nD");
}

#[test]
fn splice_lines_preserves_trailing_newline_in_existing() {
    // If the existing file ends with `\n`, the spliced output must too.
    let out = document::splice_lines("A\nB\nC\n", 2, 2, "Q");
    assert_eq!(out, "A\nQ\nC\n");
}

#[test]
fn splice_lines_clamps_end_past_eof() {
    // Overshooting end clamps to the real line count rather than erroring.
    let out = document::splice_lines("A\nB\nC", 2, 99, "Q");
    assert_eq!(out, "A\nQ");
}

#[test]
fn splice_lines_first_line() {
    // Boundary: line 1 is a legal start.
    let out = document::splice_lines("A\nB\nC", 1, 1, "X");
    assert_eq!(out, "X\nB\nC");
}

#[test]
fn splice_lines_last_line() {
    // Boundary: the final line is a legal end.
    let out = document::splice_lines("A\nB\nC", 3, 3, "X");
    assert_eq!(out, "A\nB\nX");
}

#[test]
fn write_partial_replaces_range_only() {
    // Acceptance: lines outside [start..=end] are byte-identical after
    // a partial write. We use a document whose frontmatter already
    // carries every field `ensure_frontmatter` would otherwise inject,
    // so the post-parse -> to_markdown round-trip is a no-op.
    let original = "\
---
title: Test
description: ''
author: eduardo
created: 2026-04-18T00:00:00+00:00
remargin_pending: 0
remargin_pending_for: []
remargin_last_activity: null
---

# Header

line 10
line 11
line 12

more content here
";
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), original.as_bytes())
        .unwrap();

    let config = open_config();

    // Count lines: the frontmatter occupies lines 1..=9, blank line 10,
    // `# Header` line 11, blank line 12, `line 10` at line 13.
    let line_thirteen_original = original.lines().nth(12).unwrap();
    assert_eq!(line_thirteen_original, "line 10");

    // Replace line 13 only.
    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "LINE 10 NEW",
        &config,
        WriteOptions::new().lines(Some((13, 13))),
    )
    .unwrap();

    let result = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    // Everything outside the range is byte-identical; only the target
    // line changed.
    assert!(
        result.contains("\n\nLINE 10 NEW\nline 11\nline 12\n"),
        "unexpected slice around line 13: {result}"
    );
    // Prefix (frontmatter + header) is untouched.
    assert!(result.starts_with("---\ntitle: Test\n"));
    // Suffix is untouched.
    assert!(result.contains("more content here"));
}

#[test]
fn write_partial_rejects_destroyed_comment() {
    // A partial write whose range overlaps a comment block and DOES NOT
    // reinclude the comment must fail with a preservation diagnostic
    // that names the destroyed comment id.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

    // Find the line range that covers the first comment block (id=abc).
    // DOC_WITH_COMMENTS has the block at lines 7..=17 (fence + frontmatter
    // + body + closing fence). We replace with plain text — no comment.
    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "plain text replacement",
        &config,
        WriteOptions::new().lines(Some((7, 17))),
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("abc") && msg.contains("preservation"),
        "expected preservation error naming the destroyed comment, got: {msg}"
    );
}

#[test]
fn write_partial_accepts_reincluded_comment() {
    // A partial write whose range covers a comment block IS accepted
    // as long as the replacement reincludes the comment verbatim.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

    // Replace the block covering comment abc with the same block back
    // (verbatim), and nothing else — the fence markers are part of the
    // replacement so preservation round-trips.
    let replacement = "\
```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
---
First comment.
```";

    // The `abc` block in DOC_WITH_COMMENTS spans lines 7..=17. Replace
    // that range with the same block — preservation must pass.
    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        replacement,
        &config,
        WriteOptions::new().lines(Some((7, 17))),
    )
    .unwrap();

    // Sanity: both comments still present after the write.
    let result = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(result.contains("id: abc"));
    assert!(result.contains("id: def"));
}

#[test]
fn write_partial_rejects_with_create() {
    // `--lines` and `--create` are mutually exclusive: partial writes
    // require an existing file to splice into.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();

    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("new.md"),
        "hi",
        &config,
        WriteOptions::new().create(true).lines(Some((1, 1))),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--lines is incompatible with --create")
    );
}

#[test]
fn write_partial_rejects_invalid_range() {
    // Start > end is nonsense; caller must get a specific diagnostic.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"A\nB\nC\n")
        .unwrap();

    let config = open_config();
    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "x",
        &config,
        WriteOptions::new().lines(Some((5, 3))),
    )
    .unwrap_err();
    assert!(err.to_string().contains("--lines range is invalid"));
}

#[test]
fn write_partial_rejects_with_raw() {
    // `--lines` and `--raw` are mutually exclusive: partial writes own
    // the comment-preservation invariant and need to parse the result.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/config.json"), b"{}\n")
        .unwrap();

    let config = open_config();
    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("config.json"),
        "x",
        &config,
        WriteOptions::new().raw(true).lines(Some((1, 1))),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--lines is incompatible with --raw")
    );
}

#[test]
fn write_partial_rejects_start_zero() {
    // 0-indexed callers are a common mistake; reject explicitly.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"A\nB\nC\n")
        .unwrap();

    let config = open_config();
    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "x",
        &config,
        WriteOptions::new().lines(Some((0, 3))),
    )
    .unwrap_err();
    assert!(err.to_string().contains("--lines range is invalid"));
}

#[test]
fn write_whole_file_unchanged_when_lines_omitted() {
    // Regression guard: omitting --lines preserves the pre-rem-24p
    // whole-file write semantics exactly.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();
    let modified = DOC_WITH_COMMENTS.replace("# Test", "# Still Test");
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
    assert!(result.contains("# Still Test"));
    assert!(result.contains("id: abc"));
    assert!(result.contains("id: def"));
}

// ---------- no-op detection (rem-1f2) ----------

#[test]
fn write_noop_when_identical_bytes_back_to_back() {
    // First write: canonical content gets written. Second write: same
    // input content, so the serialized output is byte-identical — must
    // return noop=true without touching the file. We verify the file
    // bytes are preserved exactly across the no-op.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();
    let config = open_config();

    let first = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        DOC_WITH_COMMENTS,
        &config,
        WriteOptions::default(),
    )
    .unwrap();
    // The first write may or may not be a true no-op depending on
    // whether the input is already canonical; either way, capture what
    // ended up on disk so we can assert the next call doesn't change it.
    let after_first = system.read_to_string(Path::new("/project/doc.md")).unwrap();

    let second = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &after_first,
        &config,
        WriteOptions::default(),
    )
    .unwrap();

    // Second call with the on-disk canonical bytes must be a no-op.
    assert!(
        second.noop,
        "expected second write of canonical content to be a no-op"
    );
    let after_second = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert_eq!(
        after_first, after_second,
        "no-op write must not touch the file bytes"
    );
    // And the first write itself: its outcome tells the caller whether
    // the input already matched disk. No other assertion here — the
    // round-trip equality above is the behavioural contract.
    let _: bool = first.noop;
}

#[test]
fn write_noop_reports_false_when_content_differs() {
    // Baseline: two distinct writes must each report noop=false so
    // callers can reliably branch on the flag.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();
    let config = open_config();

    let modified = DOC_WITH_COMMENTS.replace("# Test", "# Changed Test");
    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &modified,
        &config,
        WriteOptions::default(),
    )
    .unwrap();
    assert!(!outcome.noop, "content change must report noop=false");
    let after = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(after.contains("# Changed Test"));
}

#[test]
fn write_noop_raw_when_bytes_match() {
    // Raw writes bypass the markdown pipeline but still honor the
    // byte-identical no-op guard so `remargin write --raw` is retry-safe
    // for plain text / source files too.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();
    let config = open_config();
    let initial = "hello world\n";
    document::write(
        &system,
        Path::new("/project"),
        Path::new("notes.txt"),
        initial,
        &config,
        WriteOptions::new().create(true).raw(true),
    )
    .unwrap();

    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("notes.txt"),
        initial,
        &config,
        WriteOptions::new().raw(true),
    )
    .unwrap();
    assert!(outcome.noop, "raw write of identical bytes must be no-op");

    let changed = document::write(
        &system,
        Path::new("/project"),
        Path::new("notes.txt"),
        "hello world\nand more\n",
        &config,
        WriteOptions::new().raw(true),
    )
    .unwrap();
    assert!(!changed.noop, "raw write of new bytes must not be no-op");
}

#[test]
fn write_noop_binary_when_bytes_match() {
    // Mirror of the raw case for binary mode. MockSystem exposes
    // `read_to_string` only, so the no-op short-circuit for binary
    // files only trips when the existing bytes are valid UTF-8 — good
    // enough for this test since the payload decodes to ASCII.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();
    let config = open_config();
    // "hello" base64-encoded
    let payload = BASE64_STANDARD.encode(b"hello");
    document::write(
        &system,
        Path::new("/project"),
        Path::new("data.png"),
        &payload,
        &config,
        WriteOptions::new().create(true).binary(true),
    )
    .unwrap();

    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("data.png"),
        &payload,
        &config,
        WriteOptions::new().binary(true),
    )
    .unwrap();
    assert!(
        outcome.noop,
        "binary write of identical bytes must be no-op"
    );
}

#[test]
fn write_create_never_reports_noop() {
    // `create` writes a brand-new file — the noop short-circuit
    // must not fire (file doesn't exist yet to compare against).
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();
    let config = open_config();

    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("new.md"),
        "# New\n\nBody\n",
        &config,
        WriteOptions::new().create(true),
    )
    .unwrap();
    assert!(!outcome.noop, "create write must always report noop=false");
}

#[test]
fn list_entry_json_shape_matches_schema() {
    use std::path::PathBuf;

    let entry = document::ListEntry {
        is_dir: false,
        path: PathBuf::from("foo/bar.md"),
        remargin_last_activity: Some(String::from("2026-04-06T12:00:00-04:00")),
        remargin_pending: Some(2_u32),
        size: Some(1024_u64),
    };

    let value = serde_json::to_value(&entry).unwrap();
    let obj = value.as_object().unwrap();

    // Required keys always present.
    assert!(obj.contains_key("is_dir"));
    assert!(obj.contains_key("path"));
    assert_eq!(obj["path"], serde_json::json!("foo/bar.md"));

    // Populated optionals serialize their values.
    assert_eq!(obj["size"], serde_json::json!(1024_u64));
    assert_eq!(obj["remargin_pending"], serde_json::json!(2_u32));
    assert_eq!(
        obj["remargin_last_activity"],
        serde_json::json!("2026-04-06T12:00:00-04:00")
    );

    // Empty optionals are omitted entirely so the generated Zod
    // `strictObject` schema treats them as `undefined` rather than
    // rejecting an explicit `null`.
    let bare = document::ListEntry {
        is_dir: true,
        path: PathBuf::from("dir"),
        remargin_last_activity: None,
        remargin_pending: None,
        size: None,
    };
    let bare_value = serde_json::to_value(&bare).unwrap();
    let bare_obj = bare_value.as_object().unwrap();
    for key in ["size", "remargin_pending", "remargin_last_activity"] {
        assert!(
            !bare_obj.contains_key(key),
            "optional key `{key}` should be skipped when None"
        );
    }
}

// --- project_write tests (rem-imc) ---
//
// `project_write` is the projection-only sibling of `write` used by the
// `remargin plan write` subcommand. These tests pin three invariants:
//
// 1. The disk state is never mutated (file bytes stay byte-identical).
// 2. Binary / raw modes degrade to `WriteProjection::Unsupported` with a
//    human-readable reason, never to a bogus `Markdown` projection.
// 3. The returned `before` / `after` pair mirrors what `write` would
//    actually parse — same frontmatter normalization, same comment-
//    preservation rejection, same empty-doc shape for `--create`.

#[test]
fn project_write_happy_path_projects_markdown_without_mutating_disk() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();
    let before_bytes = read_bytes(&system, Path::new("/project/doc.md"));

    // Write appends a new body line; preserves both existing comments.
    let new_content = format!("{DOC_WITH_COMMENTS}\nA trailing paragraph.\n");
    let projection = document::project_write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &new_content,
        &config,
        WriteOptions::default(),
    )
    .unwrap();

    assert!(
        matches!(projection, WriteProjection::Markdown { .. }),
        "expected Markdown projection, got {projection:?}"
    );
    let WriteProjection::Markdown {
        after,
        before,
        noop,
    } = projection
    else {
        return;
    };
    assert!(!noop, "expected a real diff, got noop projection");
    assert_eq!(
        before.comments().len(),
        2,
        "before should reflect the on-disk document"
    );
    assert_eq!(
        after.comments().len(),
        2,
        "after should still carry the preserved comments"
    );

    // Core invariant: on-disk bytes are byte-identical post-projection.
    let after_bytes = read_bytes(&system, Path::new("/project/doc.md"));
    assert_eq!(
        before_bytes, after_bytes,
        "project_write must not mutate disk"
    );
}

#[test]
fn project_write_detects_noop_when_content_matches() {
    // Seed a file and do a real write first, so the on-disk bytes are
    // already in the shape `ensure_frontmatter` produces. Re-submitting
    // the same content should then trip the byte-identical noop path.
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        DOC_WITH_COMMENTS,
        &config,
        WriteOptions::default(),
    )
    .unwrap();

    // Capture the canonicalized on-disk bytes after the real write.
    let canonical = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    let before_bytes = canonical.clone().into_bytes();

    // Now project_write with those exact bytes — this is the true noop
    // case a caller would observe (planning a re-save of the current
    // document). `ensure_frontmatter` is idempotent on an already-
    // normalized document, so `after.to_markdown()` should match disk.
    let projection = document::project_write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &canonical,
        &config,
        WriteOptions::default(),
    )
    .unwrap();

    assert!(
        matches!(projection, WriteProjection::Markdown { .. }),
        "expected Markdown projection, got {projection:?}"
    );
    let WriteProjection::Markdown { noop, .. } = projection else {
        return;
    };
    assert!(noop, "re-submitting canonical bytes should be a noop");

    // And project_write still must not mutate disk on a noop.
    let after_bytes = read_bytes(&system, Path::new("/project/doc.md"));
    assert_eq!(before_bytes, after_bytes);
}

#[test]
fn project_write_create_returns_empty_before_and_leaves_disk_untouched() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let content = "---\ntitle: New\n---\n\n# Brand new\n";

    let projection = document::project_write(
        &system,
        Path::new("/project"),
        Path::new("new.md"),
        content,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(
        matches!(projection, WriteProjection::Markdown { .. }),
        "expected Markdown projection, got {projection:?}"
    );
    let WriteProjection::Markdown {
        after,
        before,
        noop,
    } = projection
    else {
        return;
    };
    // `before` is the parsed empty doc for --create.
    assert!(
        before.comments().is_empty(),
        "create projections must have an empty before-doc"
    );
    assert_eq!(after.comments().len(), 0);
    // --create projections are never considered noop: the file
    // does not exist yet, so the byte-identical shortcut is skipped.
    assert!(!noop);

    // Core invariant: plan must not create the file.
    assert!(
        system.read_to_string(Path::new("/project/new.md")).is_err(),
        "project_write(--create) must not touch disk"
    );
}

#[test]
fn project_write_raw_mode_returns_unsupported() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/design.pen"), b"existing")
        .unwrap();

    let config = open_config();

    let projection = document::project_write(
        &system,
        Path::new("/project"),
        Path::new("design.pen"),
        "new pen payload",
        &config,
        WriteOptions {
            raw: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(
        matches!(projection, WriteProjection::Unsupported { .. }),
        "raw mode must degrade to Unsupported, got {projection:?}"
    );
    let WriteProjection::Unsupported { reason } = projection else {
        return;
    };
    assert!(
        reason.to_lowercase().contains("raw"),
        "reason should mention raw mode, got: {reason}"
    );
}

#[test]
fn project_write_binary_mode_returns_unsupported() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/image.png"), b"\x89PNG\r\n")
        .unwrap();

    let config = open_config();

    // base64("new") — content is irrelevant because binary mode bails
    // out before parsing.
    let projection = document::project_write(
        &system,
        Path::new("/project"),
        Path::new("image.png"),
        "bmV3",
        &config,
        WriteOptions {
            binary: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(
        matches!(projection, WriteProjection::Unsupported { .. }),
        "binary mode must degrade to Unsupported, got {projection:?}"
    );
    let WriteProjection::Unsupported { reason } = projection else {
        return;
    };
    assert!(
        reason.to_lowercase().contains("binary"),
        "reason should mention binary mode, got: {reason}"
    );
}

#[test]
fn project_write_missing_comment_rejected_like_real_write() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();
    // Strip both comments — comment-preservation must refuse this.
    let new_content = "---\ntitle: Test\n---\n\n# Test\n";
    let before_bytes = read_bytes(&system, Path::new("/project/doc.md"));

    let err = document::project_write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        new_content,
        &config,
        WriteOptions::default(),
    )
    .unwrap_err();

    assert!(
        err.to_string().to_lowercase().contains("comment"),
        "expected comment-preservation error, got: {err}"
    );

    // Even on rejection, disk must stay byte-identical.
    let after_bytes = read_bytes(&system, Path::new("/project/doc.md"));
    assert_eq!(
        before_bytes, after_bytes,
        "project_write rejection must not mutate disk"
    );
}
