//! Tests for the document access layer.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System as _;
use os_shim::mock::MemorySystem;

use crate::config::{Mode, ResolvedConfig};
use crate::document::{
    self, RmDirReport, RmOutcome, RmResult, WriteOptions, WriteProjection, allowlist,
};
use crate::operations::links::Link;
use crate::operations::purge::{purge, purge_dir};
use crate::parser::AuthorType;
use crate::writer::FORBIDDEN_TARGETS;

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

const REGISTERED_REALM_YAML: &str = "identity: eduardo-burgos\ntype: human\nmode: registered\n";

const STRICT_REALM_YAML: &str = "identity: eduardo-burgos\ntype: human\nmode: strict\n";

const AUTHOR_REALM_REGISTRY_YAML: &str = "\
participants:
  eduardo-burgos:
    type: human
    status: active
    pubkeys: []
  alice:
    type: human
    status: active
    pubkeys: []
";

/// The single-file variant of an [`RmOutcome`], or `None` for a
/// directory report. Call sites `.unwrap()` to assert the variant.
fn rm_file(outcome: &RmOutcome) -> Option<&RmResult> {
    match outcome {
        RmOutcome::File(result) => Some(result),
        RmOutcome::Directory(_) => None,
    }
}

/// The directory-report variant of an [`RmOutcome`], or `None` for a
/// single-file result. Call sites `.unwrap()` to assert the variant.
fn rm_dir(outcome: &RmOutcome) -> Option<&RmDirReport> {
    match outcome {
        RmOutcome::Directory(report) => Some(report),
        RmOutcome::File(_) => None,
    }
}

fn config_with_ignore(patterns: Vec<String>) -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: patterns,
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn read_bytes(system: &MemorySystem, path: &Path) -> Vec<u8> {
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
        source_path: None,
        trusted_roots: Vec::new(),
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
fn allowlist_is_text_base() {
    assert!(allowlist::is_text(Path::new("projects.base")));
}

#[test]
fn allowlist_is_text_base_uppercase() {
    assert!(allowlist::is_text(Path::new("projects.BASE")));
}

#[test]
fn allowlist_is_visible_base() {
    assert!(allowlist::is_visible(Path::new("projects.base"), false));
}

#[test]
fn allowlist_source_code_extensions_visible_and_text() {
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
    assert!(!allowlist::is_visible(Path::new(".env"), false));
    assert!(allowlist::is_visible(Path::new("app.env"), false));
    assert!(allowlist::is_visible(Path::new("server.conf"), false));
}

#[test]
fn allowlist_terraform_family_visible_and_text() {
    for path in &["main.tf", "prod.tfvars", "backend.hcl"] {
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
fn not_visible_message_names_disallowed_extension() {
    let msg = allowlist::not_visible_message(Path::new("app.exe"));
    assert!(
        msg.contains("app.exe") && msg.contains("extension .exe is not in the allowlist"),
        "extension-only rejection must name the extension, got: {msg}"
    );
}

#[test]
fn not_visible_message_bare_for_dotfile() {
    let msg = allowlist::not_visible_message(Path::new(".secret.md"));
    assert_eq!(msg, "file not visible: .secret.md");
}

#[test]
fn ls_visible_files() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
        &open_config(),
    )
    .unwrap();
    assert_eq!(content, "# Hello\nWorld");
}

#[test]
fn get_with_links_excludes_comment_blocks() {
    let doc = "\
# Doc

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
---
A comment mentioning [[Hidden]].
```

Body links to [[Real]].
";
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), doc.as_bytes())
        .unwrap()
        .with_file(Path::new("/project/Real.md"), b"# Real")
        .unwrap()
        .with_file(Path::new("/project/Hidden.md"), b"# Hidden")
        .unwrap();

    let result = document::get_with_links(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        false,
        &open_config(),
    )
    .unwrap();

    assert_eq!(result.content, doc);
    assert_eq!(result.links.len(), 1);
    assert_eq!(result.links[0].target, "Real");
    assert_eq!(result.links[0].path.as_deref(), Some("Real.md"));
}

#[test]
fn get_with_links_slice_relative_references() {
    let doc = "line 1\nline 2\nsee [[Alpha]]\nline 4\n[[Beta]] here\n";
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), doc.as_bytes())
        .unwrap()
        .with_file(Path::new("/project/Alpha.md"), b"# Alpha")
        .unwrap()
        .with_file(Path::new("/project/Beta.md"), b"# Beta")
        .unwrap();

    let whole = document::get_with_links(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        false,
        &open_config(),
    )
    .unwrap();
    assert_eq!(whole.links.len(), 2);
    let whole_alpha = whole.links.iter().find(|l| l.target == "Alpha").unwrap();
    assert_eq!(whole_alpha.lines[0], 3);

    let sliced = document::get_with_links(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        Some((3, 5)),
        false,
        &open_config(),
    )
    .unwrap();
    assert_eq!(sliced.links.len(), 2);
    let sliced_alpha = sliced.links.iter().find(|l| l.target == "Alpha").unwrap();
    assert_eq!(sliced_alpha.lines[0], 1);
    let sliced_beta = sliced.links.iter().find(|l| l.target == "Beta").unwrap();
    assert_eq!(sliced_beta.lines[0], 3);
}

#[test]
fn get_with_links_empty_when_no_links() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# Hello\nWorld")
        .unwrap();

    let result = document::get_with_links(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        None,
        false,
        &open_config(),
    )
    .unwrap();
    assert_eq!(result.content, "# Hello\nWorld");
    assert_eq!(result.links, [] as [Link; 0]);
}

#[test]
fn get_dotfile_hidden() {
    let system = MemorySystem::new()
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
        &open_config(),
    );
    result.unwrap_err();
}

#[test]
fn get_disallowed_extension() {
    let system = MemorySystem::new()
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
        &open_config(),
    );
    result.unwrap_err();
}

#[test]
fn get_with_lines() {
    let system = MemorySystem::new()
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
        &open_config(),
    )
    .unwrap();
    assert_eq!(content, "line2\nline3\nline4");
}

#[test]
fn resolve_line_window_supports_half_open_ranges() {
    use crate::document::resolve_line_window;
    assert_eq!(resolve_line_window(Some(3), Some(5)), Some((3, 5)));
    assert_eq!(resolve_line_window(Some(3), None), Some((3, usize::MAX)));
    assert_eq!(resolve_line_window(None, Some(5)), Some((1, 5)));
    assert_eq!(resolve_line_window(None, None), None);
}

#[test]
fn get_escape_attempt() {
    let system = MemorySystem::new()
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
        &open_config(),
    );
    result.unwrap_err();
}

#[test]
fn metadata_correct_counts() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let meta = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &open_config(),
    )
    .unwrap();
    assert_eq!(meta.comment_count, Some(2));
    assert_eq!(meta.pending_count, Some(1));
    assert_eq!(meta.pending_for, vec!["alice"]);
    assert!(meta.last_activity.is_some());
    assert!(meta.frontmatter.is_some());
    assert!(!meta.binary);
    assert_eq!(meta.mime, "text/markdown");
    assert!(meta.line_count.is_some());
}

#[test]
fn metadata_binary_file_returns_file_level_fields_only() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), &[0x89, b'P', b'N', b'G'])
        .unwrap();

    let meta = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("pic.png"),
        &open_config(),
    )
    .unwrap();

    assert!(meta.binary);
    assert_eq!(meta.mime, "image/png");
    assert!(meta.path.ends_with("pic.png"));
    assert_eq!(meta.comment_count, None);
    assert_eq!(meta.line_count, None);
    assert_eq!(meta.pending_count, None);
    assert_eq!(meta.pending_for, [] as [String; 0]);
    assert!(meta.last_activity.is_none());
    assert!(meta.frontmatter.is_none());
}

#[test]
fn metadata_non_md_text_file_returns_markdown_fields() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/notes.txt"), b"line one\nline two\n")
        .unwrap();

    let meta = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("notes.txt"),
        &open_config(),
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
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.pdf"), b"%PDF-1.4")
        .unwrap();

    let meta = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("doc.pdf"),
        &open_config(),
    )
    .unwrap();

    assert!(meta.binary);
    assert_eq!(meta.mime, "application/pdf");
    assert_eq!(meta.comment_count, None);
}

#[test]
fn metadata_directed_with_third_party_ack_is_pending_for_addressee() {
    const DOC: &str = "\
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
to: [eduardo]
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
ack:
  - agent@2026-04-06T13:00:00-04:00
---
Self-addressed note acked by an agent only.
```
";
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC.as_bytes())
        .unwrap();

    let meta = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &open_config(),
    )
    .unwrap();
    assert_eq!(meta.comment_count, Some(1));
    assert_eq!(meta.pending_count, Some(1));
    assert_eq!(meta.pending_for, vec![String::from("eduardo")]);
}

#[test]
fn metadata_missing_file_errors() {
    let system = MemorySystem::new().with_current_dir("/project").unwrap();

    let result = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("nonexistent.md"),
        &open_config(),
    );
    result.unwrap_err();
}

#[test]
fn read_binary_returns_bytes_and_mime() {
    let bytes: &[u8] = &[0x89, b'P', b'N', b'G', 1, 2, 3];
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), bytes)
        .unwrap();

    let payload = document::read_binary(
        &system,
        Path::new("/project"),
        Path::new("pic.png"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(payload.bytes, bytes);
    assert_eq!(payload.mime, "image/png");
    assert!(payload.path.ends_with("pic.png"));
}

#[test]
fn read_binary_rejects_markdown() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# hi\n")
        .unwrap();

    let err = document::read_binary(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &open_config(),
    )
    .unwrap_err();
    assert!(format!("{err}").contains("cannot fetch markdown file as binary"));
}

#[test]
fn read_binary_unknown_extension_is_octet_stream() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/blob.bin"), b"raw")
        .unwrap();

    let result = document::read_binary(
        &system,
        Path::new("/project"),
        Path::new("blob.bin"),
        &open_config(),
    );
    result.unwrap_err();
}

#[test]
fn read_binary_rejects_dotfile() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/.env"), b"secret")
        .unwrap();

    let result = document::read_binary(
        &system,
        Path::new("/project"),
        Path::new(".env"),
        &open_config(),
    );
    result.unwrap_err();
}

#[test]
fn read_binary_escape_attempt() {
    let system = MemorySystem::new()
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
        &open_config(),
    );
    result.unwrap_err();
}

#[test]
fn write_preserves_comments() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

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
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

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
    let system = MemorySystem::new()
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
fn write_skips_frontmatter_injection_for_non_md_extensions() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let content = "opaque pen content\nno frontmatter expected\n";

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
    assert_eq!(
        result, content,
        "non-markdown extension must round-trip byte-for-byte; \
         frontmatter injection is markdown-only"
    );
}

#[test]
fn write_create_rejects_existing_file() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

    let result = system
        .read_to_string(Path::new("/project/newdir/subdir/file.md"))
        .unwrap();
    assert!(result.contains("Nested Document"));

    assert!(system.is_dir(Path::new("/project/newdir")).unwrap());
    assert!(system.is_dir(Path::new("/project/newdir/subdir")).unwrap());
}

#[test]
fn write_create_deeply_nested() {
    let system = MemorySystem::new()
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

    assert!(system.is_dir(Path::new("/project/a")).unwrap());
    assert!(system.is_dir(Path::new("/project/a/b")).unwrap());
    assert!(system.is_dir(Path::new("/project/a/b/c")).unwrap());
    assert!(system.is_dir(Path::new("/project/a/b/c/d")).unwrap());
}

#[test]
fn write_create_existing_parent_unchanged() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
fn write_raw_create_terraform_file() {
    let raw_content = "resource \"null_resource\" \"x\" {}\n";
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("terraform/main.tf"),
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
        .read_to_string(Path::new("/project/terraform/main.tf"))
        .unwrap();
    assert_eq!(result, raw_content);
}

#[test]
fn write_raw_overwrites_without_comment_check() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
fn write_binary_png_content() {
    // Minimal valid PNG header bytes.
    let png_bytes: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    let b64 = BASE64_STANDARD.encode(png_bytes);

    let system = MemorySystem::new()
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

    let system = MemorySystem::new()
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
    assert_eq!(on_disk.as_slice(), content_bytes);
}

#[test]
fn write_binary_create_new_file() {
    let content_bytes = b"new binary file";
    let b64 = BASE64_STANDARD.encode(content_bytes);

    let system = MemorySystem::new()
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

    let system = MemorySystem::new()
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

    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    assert!(
        result.contains("---"),
        "expected frontmatter, got: {result}"
    );
    assert!(result.contains("Hello"));
}

// Note: MemorySystem::canonicalize does not resolve `..` components, so parent
// traversal tests use absolute paths (which canonicalize handles correctly).

#[test]
fn sandbox_blocks_absolute_escape() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("path escapes sandbox"),
        "expected 'path escapes sandbox', got: {err}"
    );
}

#[test]
fn sandbox_create_blocks_absolute_escape() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("path escapes sandbox"),
        "expected 'path escapes sandbox', got: {err}"
    );
}

#[test]
fn sandbox_allows_child_path() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/project/src/main.rs"));
}

#[test]
fn sandbox_create_allows_child() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/project/src/new.md"));
}

#[test]
fn unrestricted_allows_absolute_path() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/home/user/file.md"));
}

#[test]
fn unrestricted_allows_relative_within_sandbox() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/project/src/main.rs"));
}

#[test]
fn unrestricted_create_allows_absolute_escape() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/other/new.md"));
}

#[test]
fn unrestricted_create_absolute() {
    let system = MemorySystem::new()
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
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/tmp/new.md"));
}

#[test]
fn sandbox_under_base_allowed_with_no_trusted_roots() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/project/doc.md"), b"# d")
        .unwrap();
    let result = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        false,
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/project/doc.md"));
}

#[test]
fn sandbox_under_trusted_root_outside_base_allowed() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/notes"))
        .unwrap()
        .with_file(Path::new("/notes/widening.md"), b"# w")
        .unwrap();
    let trusted = vec![PathBuf::from("/notes")];
    let result = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/notes/widening.md"),
        false,
        &trusted,
    )
    .unwrap();
    assert_eq!(result, Path::new("/notes/widening.md"));
}

#[test]
fn sandbox_outside_base_and_trusted_roots_rejected() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/notes"))
        .unwrap()
        .with_file(Path::new("/elsewhere/foo.md"), b"# x")
        .unwrap();
    let trusted = vec![PathBuf::from("/notes")];
    let err = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/elsewhere/foo.md"),
        false,
        &trusted,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("path escapes sandbox"),
        "expected 'path escapes sandbox', got: {err}",
    );
}

#[test]
fn sandbox_create_under_trusted_root_outside_base_allowed() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_dir(Path::new("/notes"))
        .unwrap();
    let trusted = vec![PathBuf::from("/notes")];
    let result = allowlist::resolve_sandboxed_create(
        &system,
        Path::new("/project"),
        Path::new("/notes/new.md"),
        false,
        &trusted,
    )
    .unwrap();
    assert_eq!(result, Path::new("/notes/new.md"));
}

#[test]
fn sandboxed_absolute_blocked_but_unrestricted_allows() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(Path::new("/home/user/notes.md"), b"# Notes")
        .unwrap();

    let err = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/home/user/notes.md"),
        false,
        &[],
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("path escapes sandbox"),
        "expected 'path escapes sandbox', got: {err}"
    );

    let result = allowlist::resolve_sandboxed(
        &system,
        Path::new("/project"),
        Path::new("/home/user/notes.md"),
        true,
        &[],
    )
    .unwrap();
    assert_eq!(result, Path::new("/home/user/notes.md"));
}

#[test]
fn line_numbers_full_file() {
    let system = MemorySystem::new()
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
        &open_config(),
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

    let system = MemorySystem::new()
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
        &open_config(),
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

    let system = MemorySystem::new()
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
        &open_config(),
    )
    .unwrap();
    assert_eq!(
        content,
        " 998\u{2502} line998\n 999\u{2502} line999\n1000\u{2502} line1000"
    );
}

#[test]
fn line_numbers_off_by_default() {
    let system = MemorySystem::new()
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
        &open_config(),
    )
    .unwrap();
    assert_eq!(content, "hello\nworld");
}

#[test]
fn line_numbers_single_line() {
    let system = MemorySystem::new()
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
        &open_config(),
    )
    .unwrap();
    assert_eq!(content, "1\u{2502} only line");
}

#[test]
fn line_numbers_empty_lines() {
    let system = MemorySystem::new()
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
        &open_config(),
    )
    .unwrap();
    assert_eq!(content, "1\u{2502} first\n2\u{2502} \n3\u{2502} third");
}

#[test]
fn line_numbers_binary_rejected() {
    let system = MemorySystem::new()
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
        &open_config(),
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not supported for binary"),
        "expected binary error, got: {err}"
    );
}

#[test]
fn rm_deletes_existing_file() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/notes.md"), b"# Notes")
        .unwrap();

    let config = open_config();
    let outcome = document::rm(
        &system,
        Path::new("/project"),
        Path::new("notes.md"),
        &config,
    )
    .unwrap();

    assert!(rm_file(&outcome).unwrap().existed);
    system
        .read_to_string(Path::new("/project/notes.md"))
        .unwrap_err();
}

#[test]
fn rm_idempotent_missing_file() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let outcome = document::rm(
        &system,
        Path::new("/project"),
        Path::new("nonexistent.md"),
        &config,
    )
    .unwrap();

    assert!(!rm_file(&outcome).unwrap().existed);
}

#[test]
fn rm_rejects_hidden_file() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
fn rm_removes_empty_directory() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project/subdir"))
        .unwrap();

    let config = open_config();
    let outcome =
        document::rm(&system, Path::new("/project"), Path::new("subdir"), &config).unwrap();

    let report = rm_dir(&outcome).unwrap();
    assert_eq!(report.files_deleted, [] as [PathBuf; 0]);
    assert_eq!(
        report.folders_removed,
        vec![PathBuf::from("/project/subdir")]
    );
    assert_eq!(report.folders_left_behind, [] as [PathBuf; 0]);
    assert!(!system.exists(Path::new("/project/subdir")).unwrap());
}

#[test]
fn rm_deletes_non_markdown_binary_file() {
    // Real PNG signature — invalid UTF-8 (starts with 0x89).
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xd8];
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/assets/probe.png"), png)
        .unwrap();
    let config = open_config();

    let payload = document::read_binary(
        &system,
        Path::new("/project"),
        Path::new("assets/probe.png"),
        &config,
    )
    .unwrap();
    assert_eq!(
        payload.bytes.as_slice(),
        png,
        "read layer must see the binary file"
    );

    let outcome = document::rm(
        &system,
        Path::new("/project"),
        Path::new("assets/probe.png"),
        &config,
    )
    .unwrap();
    assert!(
        rm_file(&outcome).unwrap().existed,
        "existed must be true for a file that is present on disk"
    );
    assert!(
        !system
            .exists(Path::new("/project/assets/probe.png"))
            .unwrap(),
        "the file must be gone from disk after rm"
    );
}

#[test]
fn rm_can_delete_anything_the_read_layer_sees() {
    let bytes: &[u8] = &[0x00, 0x01, 0xfe, 0xff, 0x80, 0x90];
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/assets/figure.png"), bytes)
        .unwrap();
    let config = open_config();

    let meta = document::metadata(
        &system,
        Path::new("/project"),
        Path::new("assets/figure.png"),
        &config,
    )
    .unwrap();
    assert_eq!(meta.size_bytes, bytes.len() as u64);

    let outcome = document::rm(
        &system,
        Path::new("/project"),
        Path::new("assets/figure.png"),
        &config,
    )
    .unwrap();
    assert!(rm_file(&outcome).unwrap().existed);
    assert!(
        !system
            .exists(Path::new("/project/assets/figure.png"))
            .unwrap(),
        "read-visible file must be deletable"
    );
}

#[test]
fn rm_dir_removes_all_visible_files_and_reports_them() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/docs/a.md"), b"a")
        .unwrap()
        .with_file(Path::new("/project/docs/b.md"), b"b")
        .unwrap()
        .with_file(Path::new("/project/docs/c.txt"), b"c")
        .unwrap();

    let config = open_config();
    let outcome = document::rm(&system, Path::new("/project"), Path::new("docs"), &config).unwrap();

    let report = rm_dir(&outcome).unwrap();
    assert_eq!(report.files_deleted.len(), 3, "all three files reported");
    assert_eq!(report.folders_removed, vec![PathBuf::from("/project/docs")]);
    assert_eq!(report.folders_left_behind, [] as [PathBuf; 0]);
    assert!(!system.exists(Path::new("/project/docs")).unwrap());
}

#[test]
fn rm_dir_removes_nested_subdirs_bottom_up() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/tree/top.md"), b"t")
        .unwrap()
        .with_file(Path::new("/project/tree/mid/m.md"), b"m")
        .unwrap()
        .with_file(Path::new("/project/tree/mid/deep/d.md"), b"d")
        .unwrap();

    let config = open_config();
    let outcome = document::rm(&system, Path::new("/project"), Path::new("tree"), &config).unwrap();

    let report = rm_dir(&outcome).unwrap();
    assert_eq!(report.files_deleted.len(), 3);
    assert_eq!(
        report.folders_removed,
        vec![
            PathBuf::from("/project/tree/mid/deep"),
            PathBuf::from("/project/tree/mid"),
            PathBuf::from("/project/tree"),
        ]
    );
    assert_eq!(report.folders_left_behind, [] as [PathBuf; 0]);
    assert!(!system.exists(Path::new("/project/tree")).unwrap());
}

#[test]
fn rm_dir_with_only_hidden_file_leaves_folder_behind() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/box/visible.md"), b"v")
        .unwrap()
        .with_file(Path::new("/project/box/.secret"), b"hidden")
        .unwrap();

    let config = open_config();
    let outcome = document::rm(&system, Path::new("/project"), Path::new("box"), &config).unwrap();

    let report = rm_dir(&outcome).unwrap();
    assert_eq!(
        report.files_deleted,
        vec![PathBuf::from("/project/box/visible.md")]
    );
    assert_eq!(report.folders_removed, [] as [PathBuf; 0]);
    assert_eq!(
        report.folders_left_behind,
        vec![PathBuf::from("/project/box")]
    );
    assert!(!system.exists(Path::new("/project/box/visible.md")).unwrap());
    assert!(system.exists(Path::new("/project/box/.secret")).unwrap());
}

#[test]
fn rm_dir_with_nested_realm_config_leaves_realm_folder_intact() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/outer/top.md"), b"t")
        .unwrap()
        .with_file(
            Path::new("/project/outer/realm/.remargin.yaml"),
            b"mode: open\n",
        )
        .unwrap()
        .with_file(Path::new("/project/outer/realm/doc.md"), b"d")
        .unwrap();

    let config = open_config();
    let outcome =
        document::rm(&system, Path::new("/project"), Path::new("outer"), &config).unwrap();

    let report = rm_dir(&outcome).unwrap();
    assert!(
        report
            .files_deleted
            .contains(&PathBuf::from("/project/outer/top.md"))
    );
    assert!(
        report
            .files_deleted
            .contains(&PathBuf::from("/project/outer/realm/doc.md"))
    );
    assert!(
        report
            .folders_left_behind
            .contains(&PathBuf::from("/project/outer/realm"))
    );
    assert!(
        report
            .folders_left_behind
            .contains(&PathBuf::from("/project/outer"))
    );
    assert!(
        system
            .exists(Path::new("/project/outer/realm/.remargin.yaml"))
            .unwrap(),
        "nested realm config must survive"
    );
}

#[test]
fn rm_dir_leaves_folder_holding_registry_dotfile_intact() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/cfg/keep.md"), b"k")
        .unwrap()
        .with_file(Path::new("/project/cfg/.remargin-registry.yaml"), b"x")
        .unwrap();

    let config = open_config();
    let outcome = document::rm(&system, Path::new("/project"), Path::new("cfg"), &config).unwrap();
    let report = rm_dir(&outcome).unwrap();
    assert_eq!(
        report.files_deleted,
        vec![PathBuf::from("/project/cfg/keep.md")]
    );
    assert_eq!(
        report.folders_left_behind,
        vec![PathBuf::from("/project/cfg")]
    );
}

#[test]
fn rm_dir_refuses_path_outside_sandbox() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/outside/secret.md"), b"s")
        .unwrap();

    let config = open_config();
    let result = document::rm(
        &system,
        Path::new("/project"),
        Path::new("../outside"),
        &config,
    );

    result.unwrap_err();
}

#[test]
fn rm_dir_report_to_json_shape() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/d/one.md"), b"1")
        .unwrap();

    let config = open_config();
    let outcome = document::rm(&system, Path::new("/project"), Path::new("d"), &config).unwrap();

    let value = outcome.to_json("d");
    assert_eq!(value["deleted"], "d");
    assert_eq!(value["is_directory"], true);
    assert_eq!(value["files_deleted"].as_array().unwrap().len(), 1);
    assert_eq!(value["folders_removed"].as_array().unwrap().len(), 1);
    assert_eq!(value["folders_left_behind"].as_array().unwrap().len(), 0);
}

#[test]
fn rm_refuses_commented_markdown_file() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/note.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();
    let result = document::rm(
        &system,
        Path::new("/project"),
        Path::new("note.md"),
        &config,
    );

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("comment"),
        "error names the comment count: {err}"
    );
    assert!(err.contains("purge"), "error points at purge: {err}");
    assert!(
        system.exists(Path::new("/project/note.md")).unwrap(),
        "commented file must survive a refused rm"
    );
}

#[test]
fn rm_deletes_comment_free_markdown_file() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(
            Path::new("/project/plain.md"),
            b"# Plain\n\nNo comments here.",
        )
        .unwrap();

    let config = open_config();
    let outcome = document::rm(
        &system,
        Path::new("/project"),
        Path::new("plain.md"),
        &config,
    )
    .unwrap();

    assert!(rm_file(&outcome).unwrap().existed);
    assert!(!system.exists(Path::new("/project/plain.md")).unwrap());
}

#[test]
fn rm_purge_then_rm_deletes_previously_commented_file() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/note.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();
    purge(&system, Path::new("/project/note.md"), &config).unwrap();

    let outcome = document::rm(
        &system,
        Path::new("/project"),
        Path::new("note.md"),
        &config,
    )
    .unwrap();
    assert!(rm_file(&outcome).unwrap().existed);
    assert!(!system.exists(Path::new("/project/note.md")).unwrap());
}

#[test]
fn rm_dir_aborts_when_any_nested_file_has_comments() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/tree/top.md"), b"t")
        .unwrap()
        .with_file(
            Path::new("/project/tree/mid/deep/commented.md"),
            DOC_WITH_COMMENTS.as_bytes(),
        )
        .unwrap();

    let config = open_config();
    let result = document::rm(&system, Path::new("/project"), Path::new("tree"), &config);

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("commented.md"),
        "error names the offending file: {err}"
    );
    assert!(
        err.contains("nothing deleted"),
        "all-or-nothing wording: {err}"
    );
    assert!(
        system.exists(Path::new("/project/tree/top.md")).unwrap(),
        "sibling file must survive an aborted dir rm"
    );
    assert!(
        system
            .exists(Path::new("/project/tree/mid/deep/commented.md"))
            .unwrap(),
        "commented file must survive an aborted dir rm"
    );
}

#[test]
fn rm_dir_deletes_tree_with_no_commented_files() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/clean/a.md"), b"a")
        .unwrap()
        .with_file(Path::new("/project/clean/sub/b.md"), b"b")
        .unwrap();

    let config = open_config();
    let outcome =
        document::rm(&system, Path::new("/project"), Path::new("clean"), &config).unwrap();

    let report = rm_dir(&outcome).unwrap();
    assert_eq!(report.files_deleted.len(), 2);
    assert!(!system.exists(Path::new("/project/clean")).unwrap());
}

#[test]
fn rm_purge_dir_then_rm_dir_deletes_tree() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(
            Path::new("/project/tree/commented.md"),
            DOC_WITH_COMMENTS.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/project/tree/plain.md"), b"p")
        .unwrap();

    let config = open_config();
    purge_dir(&system, Path::new("/project/tree"), &config).unwrap();

    let outcome = document::rm(&system, Path::new("/project"), Path::new("tree"), &config).unwrap();
    let report = rm_dir(&outcome).unwrap();
    assert_eq!(report.files_deleted.len(), 2);
    assert!(!system.exists(Path::new("/project/tree")).unwrap());
}

#[test]
fn splice_lines_replaces_single_line() {
    let out = document::splice_lines("A\nB\nC\nD\nE", 3, 3, "X");
    assert_eq!(out, "A\nB\nX\nD\nE");
}

#[test]
fn splice_lines_expanding_range_inserts_lines() {
    let out = document::splice_lines("A\nB\nC\nD\nE", 3, 3, "X\nY\nZ");
    assert_eq!(out, "A\nB\nX\nY\nZ\nD\nE");
}

#[test]
fn splice_lines_shrinking_range_removes_lines() {
    let out = document::splice_lines("A\nB\nC\nD\nE", 2, 4, "Q");
    assert_eq!(out, "A\nQ\nE");
}

#[test]
fn splice_lines_strips_one_trailing_newline() {
    let out = document::splice_lines("A\nB\nC\nD", 3, 3, "X\n");
    assert_eq!(out, "A\nB\nX\nD");
}

#[test]
fn splice_lines_preserves_trailing_newline_in_existing() {
    let out = document::splice_lines("A\nB\nC\n", 2, 2, "Q");
    assert_eq!(out, "A\nQ\nC\n");
}

#[test]
fn splice_lines_clamps_end_past_eof() {
    let out = document::splice_lines("A\nB\nC", 2, 99, "Q");
    assert_eq!(out, "A\nQ");
}

#[test]
fn splice_lines_first_line() {
    let out = document::splice_lines("A\nB\nC", 1, 1, "X");
    assert_eq!(out, "X\nB\nC");
}

#[test]
fn splice_lines_last_line() {
    let out = document::splice_lines("A\nB\nC", 3, 3, "X");
    assert_eq!(out, "A\nB\nX");
}

#[test]
fn write_partial_replaces_range_only() {
    let original = "\
---
title: Test
description: ''
author: eduardo
created: 2026-04-18T00:00:00+00:00
remargin_last_activity: null
---

# Header

line 10
line 11
line 12

more content here
";
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), original.as_bytes())
        .unwrap();

    let config = open_config();

    let line_eleven_original = original.lines().nth(10).unwrap();
    assert_eq!(line_eleven_original, "line 10");

    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "LINE 10 NEW",
        &config,
        WriteOptions::new().lines(Some((11, 11))),
    )
    .unwrap();

    let result = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(
        result.contains("\n\nLINE 10 NEW\nline 11\nline 12\n"),
        "unexpected slice around line 13: {result}"
    );
    assert!(result.starts_with("---\ntitle: Test\n"));
    assert!(result.contains("more content here"));
}

#[test]
fn write_partial_rejects_destroyed_comment() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

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
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();

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

    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        replacement,
        &config,
        WriteOptions::new().lines(Some((7, 17))),
    )
    .unwrap();

    let result = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(result.contains("id: abc"));
    assert!(result.contains("id: def"));
}

#[test]
fn write_partial_rejects_with_create() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

#[test]
fn write_noop_when_identical_bytes_back_to_back() {
    let system = MemorySystem::new()
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

    assert!(
        second.noop,
        "expected second write of canonical content to be a no-op"
    );
    let after_second = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert_eq!(
        after_first, after_second,
        "no-op write must not touch the file bytes"
    );
    let _: bool = first.noop;
}

#[test]
fn write_noop_reports_false_when_content_differs() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

    assert!(obj.contains_key("is_dir"));
    assert!(obj.contains_key("path"));
    assert_eq!(obj["path"], serde_json::json!("foo/bar.md"));

    assert_eq!(obj["size"], serde_json::json!(1024_u64));
    assert_eq!(obj["remargin_pending"], serde_json::json!(2_u32));
    assert_eq!(
        obj["remargin_last_activity"],
        serde_json::json!("2026-04-06T12:00:00-04:00")
    );

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

#[test]
fn project_write_happy_path_projects_markdown_without_mutating_disk() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();
    let before_bytes = read_bytes(&system, Path::new("/project/doc.md"));

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
        matches!(
            projection,
            WriteProjection::Markdown {
                after: _,
                before: _,
                noop: _
            }
        ),
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

    let after_bytes = read_bytes(&system, Path::new("/project/doc.md"));
    assert_eq!(
        before_bytes, after_bytes,
        "project_write must not mutate disk"
    );
}

#[test]
fn project_write_detects_noop_when_content_matches() {
    let system = MemorySystem::new()
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

    let canonical = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    let before_bytes = canonical.clone().into_bytes();

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
        matches!(
            projection,
            WriteProjection::Markdown {
                after: _,
                before: _,
                noop: _
            }
        ),
        "expected Markdown projection, got {projection:?}"
    );
    let WriteProjection::Markdown {
        noop,
        after: _,
        before: _,
    } = projection
    else {
        return;
    };
    assert!(noop, "re-submitting canonical bytes should be a noop");

    let after_bytes = read_bytes(&system, Path::new("/project/doc.md"));
    assert_eq!(before_bytes, after_bytes);
}

#[test]
fn project_write_create_returns_empty_before_and_leaves_disk_untouched() {
    let system = MemorySystem::new()
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
        matches!(
            projection,
            WriteProjection::Markdown {
                after: _,
                before: _,
                noop: _
            }
        ),
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
    assert!(
        before.comments().is_empty(),
        "create projections must have an empty before-doc"
    );
    assert_eq!(after.comments().len(), 0);
    assert!(!noop);

    assert!(
        system.read_to_string(Path::new("/project/new.md")).is_err(),
        "project_write(--create) must not touch disk"
    );
}

#[test]
fn project_write_raw_mode_returns_unsupported() {
    let system = MemorySystem::new()
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
        matches!(projection, WriteProjection::Unsupported { reason: _ }),
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
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/image.png"), b"\x89PNG\r\n")
        .unwrap();

    let config = open_config();

    // base64("new"); binary mode bails out before parsing it.
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
        matches!(projection, WriteProjection::Unsupported { reason: _ }),
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
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.md"), DOC_WITH_COMMENTS.as_bytes())
        .unwrap();

    let config = open_config();
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

    let after_bytes = read_bytes(&system, Path::new("/project/doc.md"));
    assert_eq!(
        before_bytes, after_bytes,
        "project_write rejection must not mutate disk"
    );
}

fn caller_config(identity: &str, mode: Mode) -> ResolvedConfig {
    ResolvedConfig {
        identity: Some(String::from(identity)),
        mode,
        ..open_config()
    }
}

/// A `/project` realm whose `.remargin.yaml` declares `realm_yaml`, with a
/// registry and a `doc.md` seeded with `doc`.
fn realm_with_doc(realm_yaml: &str, doc: &str) -> MemorySystem {
    MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/.remargin.yaml"), realm_yaml.as_bytes())
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            AUTHOR_REALM_REGISTRY_YAML.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/project/doc.md"), doc.as_bytes())
        .unwrap()
}

#[test]
fn write_create_stamps_caller_identity_ignoring_spoof() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();
    let config = open_config();

    document::write(
        &system,
        Path::new("/project"),
        Path::new("new.md"),
        "---\ntitle: New\nauthor: someone_else\n---\n\n# New\n",
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    let disk = system.read_to_string(Path::new("/project/new.md")).unwrap();
    assert!(disk.contains("author: eduardo"), "got:\n{disk}");
    assert!(
        !disk.contains("someone_else"),
        "spoofed author must be dropped:\n{disk}"
    );
}

/// The realm's mode governs, not the open caller's; the disk is untouched.
#[test]
fn write_edit_author_change_to_unregistered_rejected_in_registered_realm() {
    let doc = "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nBody.\n";
    let system = realm_with_doc(REGISTERED_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Open);

    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "---\ntitle: Doc\nauthor: nobody\n---\n\n# Doc\n\nBody.\n",
        &config,
        WriteOptions::default(),
    )
    .unwrap_err();

    assert!(
        format!("{err:#}").contains("not an active registry participant"),
        "got: {err:#}"
    );
    assert_eq!(
        system.read_to_string(Path::new("/project/doc.md")).unwrap(),
        doc,
        "rejected write must not mutate disk"
    );
}

#[test]
fn write_edit_author_change_to_active_allowed_in_registered_realm() {
    let doc = "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nBody.\n";
    let system = realm_with_doc(REGISTERED_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Open);

    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "---\ntitle: Doc\nauthor: eduardo-burgos\n---\n\n# Doc\n\nBody.\n",
        &config,
        WriteOptions::default(),
    )
    .unwrap();

    let disk = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(disk.contains("author: eduardo-burgos"), "got:\n{disk}");
}

#[test]
fn write_edit_existing_author_immutable_in_strict_realm() {
    let doc = "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nBody.\n";
    let system = realm_with_doc(STRICT_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Strict);

    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "---\ntitle: Doc\nauthor: eduardo-burgos\n---\n\n# Doc\n\nBody.\n",
        &config,
        WriteOptions::default(),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("author is immutable in strict mode"),
        "got: {err}"
    );
    assert_eq!(
        system.read_to_string(Path::new("/project/doc.md")).unwrap(),
        doc,
        "rejected write must not mutate disk"
    );
}

#[test]
fn write_edit_first_author_matching_caller_allowed_in_strict_realm() {
    let doc = "---\ntitle: Doc\n---\n\n# Doc\n\nBody.\n";
    let system = realm_with_doc(STRICT_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Strict);

    document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "---\ntitle: Doc\nauthor: eduardo-burgos\n---\n\n# Doc\n\nBody.\n",
        &config,
        WriteOptions::default(),
    )
    .unwrap();

    let disk = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(disk.contains("author: eduardo-burgos"), "got:\n{disk}");
}

#[test]
fn write_edit_first_author_mismatch_rejected_in_strict_realm() {
    let doc = "---\ntitle: Doc\n---\n\n# Doc\n\nBody.\n";
    let system = realm_with_doc(STRICT_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Strict);

    let err = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nBody.\n",
        &config,
        WriteOptions::default(),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("a first-time author must be your own identity"),
        "got: {err}"
    );
    assert_eq!(
        system.read_to_string(Path::new("/project/doc.md")).unwrap(),
        doc,
        "rejected write must not mutate disk"
    );
}

/// The `plan write` projection reports the same refusal as a real write and never touches disk.
#[test]
fn project_write_author_change_rejected_in_registered_realm() {
    let doc = "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nBody.\n";
    let system = realm_with_doc(REGISTERED_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Open);
    let before = read_bytes(&system, Path::new("/project/doc.md"));

    let err = document::project_write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "---\ntitle: Doc\nauthor: nobody\n---\n\n# Doc\n\nBody.\n",
        &config,
        WriteOptions::default(),
    )
    .unwrap_err();

    assert!(
        format!("{err:#}").contains("not an active registry participant"),
        "got: {err:#}"
    );
    assert_eq!(
        before,
        read_bytes(&system, Path::new("/project/doc.md")),
        "plan projection must not mutate disk"
    );
}

#[test]
fn project_commit_markdown_author_change_rejected_in_registered_realm() {
    let doc = "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nBody.\n";
    let system = realm_with_doc(REGISTERED_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Open);

    let err = document::project_commit_markdown(
        &system,
        &config,
        Path::new("/project/doc.md"),
        "---\ntitle: Doc\nauthor: nobody\n---\n\n# Doc\n\nBody.\n",
    )
    .unwrap_err();

    assert!(
        format!("{err:#}").contains("not an active registry participant"),
        "got: {err:#}"
    );
}

#[test]
fn commit_markdown_body_only_preserves_author_in_registered_realm() {
    let doc = "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nOld body.\n";
    let system = realm_with_doc(REGISTERED_REALM_YAML, doc);
    let config = caller_config("eduardo-burgos", Mode::Open);

    let outcome = document::commit_markdown(
        &system,
        &config,
        Path::new("/project/doc.md"),
        "---\ntitle: Doc\nauthor: alice\n---\n\n# Doc\n\nNew body.\n",
        false,
    )
    .unwrap();

    assert!(!outcome.noop);
    let disk = system.read_to_string(Path::new("/project/doc.md")).unwrap();
    assert!(disk.contains("author: alice"), "got:\n{disk}");
    assert!(disk.contains("New body."), "got:\n{disk}");
}

fn assert_forbidden_error(err: &anyhow::Error, basename: &str) {
    let msg = format!("{err:#}");
    assert!(
        msg.contains("refusing to modify") && msg.contains(basename),
        "expected refusing-to-modify error for {basename}, got: {msg}"
    );
}

#[test]
fn write_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let path = format!("/project/{basename}");
        let system = MemorySystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new(&path), b"existing: true\n")
            .unwrap();

        let config = open_config();
        let before = read_bytes(&system, Path::new(&path));

        let err = document::write(
            &system,
            Path::new("/project"),
            Path::new(basename),
            "mutated: true\n",
            &config,
            WriteOptions::default(),
        )
        .unwrap_err();

        assert_forbidden_error(&err, basename);

        let after = read_bytes(&system, Path::new(&path));
        assert_eq!(before, after, "disk must be untouched after refusal");
    }
}

#[test]
fn write_refuses_forbidden_targets_nested() {
    for basename in FORBIDDEN_TARGETS {
        let nested = format!("/project/nested/{basename}");
        let system = MemorySystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_dir(Path::new("/project/nested"))
            .unwrap()
            .with_file(Path::new(&nested), b"existing: true\n")
            .unwrap();

        let config = open_config();
        let requested = format!("nested/{basename}");

        let err = document::write(
            &system,
            Path::new("/project"),
            Path::new(&requested),
            "mutated: true\n",
            &config,
            WriteOptions::default(),
        )
        .unwrap_err();

        assert_forbidden_error(&err, basename);
    }
}

#[test]
fn write_allows_differently_named_yaml() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/backup.remargin.yaml"), b"kept: true\n")
        .unwrap();

    let config = open_config();
    document::write(
        &system,
        Path::new("/project"),
        Path::new("backup.remargin.yaml"),
        "kept: true\nadded: true\n",
        &config,
        WriteOptions::default(),
    )
    .unwrap();
}

#[test]
fn write_create_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let system = MemorySystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_dir(Path::new("/project"))
            .unwrap();

        let config = open_config();
        let err = document::write(
            &system,
            Path::new("/project"),
            Path::new(basename),
            "fresh: true\n",
            &config,
            WriteOptions {
                create: true,
                ..WriteOptions::default()
            },
        )
        .unwrap_err();

        assert_forbidden_error(&err, basename);

        assert!(
            system
                .read_to_string(&Path::new("/project").join(basename))
                .is_err(),
            "file must not be created after refusal",
        );
    }
}

#[test]
fn rm_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let path = format!("/project/{basename}");
        let system = MemorySystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new(&path), b"existing: true\n")
            .unwrap();

        let config = open_config();
        let err =
            document::rm(&system, Path::new("/project"), Path::new(basename), &config).unwrap_err();

        assert_forbidden_error(&err, basename);

        system.read_to_string(Path::new(&path)).unwrap();
    }
}

#[test]
fn project_write_refuses_forbidden_targets() {
    for basename in FORBIDDEN_TARGETS {
        let path = format!("/project/{basename}");
        let system = MemorySystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new(&path), b"existing: true\n")
            .unwrap();

        let config = open_config();
        let err = document::project_write(
            &system,
            Path::new("/project"),
            Path::new(basename),
            "mutated: true\n",
            &config,
            WriteOptions::default(),
        )
        .unwrap_err();

        assert_forbidden_error(&err, basename);
    }
}

/// The advice rides back on the successful outcome; the prose is written as supplied.
#[test]
fn write_advises_on_hard_wrapped_prose_without_blocking() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let content =
        "# Doc\n\nGoal. Three actions per revision, Accept and\nManual Edit and Accept & Apply.\n";

    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("wrapped.md"),
        content,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(outcome.warnings.len(), 1_usize, "{:?}", outcome.warnings);
    assert_eq!(outcome.warnings[0].line, 3_usize);
    assert!(
        outcome.warnings[0].message.contains("hard-wrapped"),
        "advisory wording: {}",
        outcome.warnings[0].message
    );

    let written = system
        .read_to_string(Path::new("/project/wrapped.md"))
        .unwrap();
    assert!(
        written.contains("Accept and\nManual Edit"),
        "the write is untouched by the advice: {written}"
    );
}

/// With no advice the JSON payload carries no `warnings` key.
#[test]
fn write_stays_silent_on_continuous_prose() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();
    let content =
        "# Doc\n\nOne continuous paragraph that the viewer is free to wrap however it likes.\n";

    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("clean.md"),
        content,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let payload = outcome.to_json("clean.md", false, false);
    assert!(
        payload.get("warnings").is_none(),
        "a clean write keeps its original payload: {payload}"
    );
}

/// Advice covers only the supplied fragment, with line numbers shifted onto the file's own.
#[test]
fn write_partial_offsets_advice_onto_file_lines() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
        .with_file(
            Path::new("/project/doc.md"),
            b"# Doc\n\nline three\nline four\nline five\n",
        )
        .unwrap();

    let config = open_config();

    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        "replacement prose broken\nacross two lines\n",
        &config,
        WriteOptions {
            lines: Some((3, 5)),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(outcome.warnings.len(), 1_usize, "{:?}", outcome.warnings);
    assert_eq!(
        outcome.warnings[0].line, 3_usize,
        "fragment line 1 is file line 3"
    );
}

/// Stored comment blocks are fenced code, so an existing thread never draws advice.
#[test]
fn write_never_advises_about_stored_comment_blocks() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let config = open_config();

    let outcome = document::write(
        &system,
        Path::new("/project"),
        Path::new("threaded.md"),
        DOC_WITH_COMMENTS,
        &config,
        WriteOptions {
            create: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}
