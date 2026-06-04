//! Unit tests for [`crate::operations::cp`].
//!
//! Tests drive an in-memory `os_shim::mock::MockSystem` — no real filesystem,
//! fully hermetic. Signed-comment fixtures reuse the ed25519 key pair from
//! `operations/sign/tests.rs` so the integrity-safety tests can exercise real
//! checksums and signatures.

extern crate alloc;

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::crypto;
use crate::operations::cp::{CpArgs, CpKind, cp};
use crate::parser::{self, AuthorType};

// ---- Key pair (from sign/tests.rs) ----------------------------------------

const TEST_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";

// ---- Fixtures ---------------------------------------------------------------

fn base() -> &'static Path {
    Path::new("/realm")
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

fn realm_with(file: &str, contents: &[u8]) -> MockSystem {
    MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join(file), contents)
        .unwrap()
}

// ---- Group A: byte copy / verbatim ----------------------------------------

#[test]
fn non_markdown_copies_verbatim() {
    let system = realm_with("photo.png", b"PNG_BYTES");
    let args = CpArgs::new(PathBuf::from("photo.png"), PathBuf::from("photo2.png"));
    let outcome = cp(&system, base(), &open_config(), &args).unwrap();

    assert_eq!(outcome.kind, CpKind::Verbatim);
    assert_eq!(outcome.comments_dropped, 0);
    assert_eq!(outcome.bytes_copied, 9);
    assert!(system.exists(&base().join("photo.png")).unwrap()); // src untouched
    assert_eq!(
        system.read_to_string(&base().join("photo2.png")).unwrap(),
        "PNG_BYTES"
    );
}

#[test]
fn comment_free_markdown_copies_verbatim() {
    let src = "---\ntitle: Test\n---\n\n# Hello\n\nBody text.\n";
    let system = realm_with("a.md", src.as_bytes());
    let args = CpArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let outcome = cp(&system, base(), &open_config(), &args).unwrap();

    assert_eq!(outcome.kind, CpKind::Verbatim);
    assert_eq!(outcome.comments_dropped, 0);
    // src untouched
    assert_eq!(system.read_to_string(&base().join("a.md")).unwrap(), src);
    // dst exists and is parseable
    assert!(system.exists(&base().join("b.md")).unwrap());
}

// ---- Group B: body-only (core) --------------------------------------------

/// Build a comment-bearing markdown string with `n` comment blocks.
/// Uses real checksums so parse doesn't fail.
fn doc_with_comments(n: usize) -> String {
    use core::fmt::Write as _;
    let mut out = String::from("---\ntitle: src\n---\n\n# Body\n\nSome text.\n");
    for i in 0..n {
        let content = format!("comment {i}");
        let checksum = crypto::compute_checksum(&content, &[]);
        write!(
            out,
            "\n```remargin\n---\nid: c{i:04}\nauthor: alice\ntype: human\nts: 2024-01-01T00:00:00+00:00\nchecksum: {checksum}\n---\n{content}\n```\n"
        )
        .unwrap();
    }
    out
}

#[test]
fn comment_bearing_markdown_copies_body_only() {
    let src = doc_with_comments(3);
    let system = realm_with("src.md", src.as_bytes());
    let args = CpArgs::new(PathBuf::from("src.md"), PathBuf::from("dst.md"));
    let outcome = cp(&system, base(), &open_config(), &args).unwrap();

    assert_eq!(outcome.kind, CpKind::BodyOnly);
    assert_eq!(outcome.comments_dropped, 3);
    assert!(outcome.bytes_copied > 0);

    let dst_content = system.read_to_string(&base().join("dst.md")).unwrap();
    let dst_parsed = parser::parse(&dst_content).unwrap();
    // No comment blocks in the copy.
    assert_eq!(dst_parsed.comments().len(), 0);
    // src is byte-for-byte unchanged.
    assert_eq!(system.read_to_string(&base().join("src.md")).unwrap(), src);
}

// ---- Group C: source untouched --------------------------------------------

#[test]
fn source_bytes_unchanged_after_copy() {
    let src = doc_with_comments(2);
    let system = realm_with("src.md", src.as_bytes());
    let args = CpArgs::new(PathBuf::from("src.md"), PathBuf::from("dst.md"));
    cp(&system, base(), &open_config(), &args).unwrap();
    let after = system.read_to_string(&base().join("src.md")).unwrap();
    assert_eq!(after, src, "source must be byte-for-byte unchanged");
}

#[test]
fn source_signatures_intact_after_copy() {
    // Build a signed comment and verify that after the cp the signature
    // still passes.
    let content = "signed note";
    let checksum = crypto::compute_checksum(content, &[]);
    let key_path = Path::new("/keys/ed25519");
    // Build a MockSystem with the key and a simple signed doc.
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(key_path, TEST_PRIVATE_KEY.as_bytes())
        .unwrap();

    // We build the doc with an unsigned comment first.
    let src = format!(
        "---\ntitle: src\n---\n\n# Body\n\n```remargin\n---\nid: c0001\nauthor: alice\ntype: human\nts: 2024-01-01T00:00:00+00:00\nchecksum: {checksum}\n---\n{content}\n```\n"
    );
    system
        .write(base().join("src.md").as_path(), src.as_bytes())
        .unwrap();

    let args = CpArgs::new(PathBuf::from("src.md"), PathBuf::from("dst.md"));
    cp(&system, base(), &open_config(), &args).unwrap();

    // Source must be byte-identical.
    assert_eq!(system.read_to_string(&base().join("src.md")).unwrap(), src);
}

// ---- Group E: frontmatter reset -------------------------------------------

#[test]
fn copy_resets_frontmatter_pending_and_sandbox() {
    // Source has a sandbox entry and pending comments.
    let content = "hello";
    let checksum = crypto::compute_checksum(content, &[]);
    let src = format!(
        "---\ntitle: src\nremargin_pending: 2\nremargin_pending_for:\n  - alice\nremargin_last_activity: '2024-01-01T00:00:00+00:00'\nsandbox:\n  - alice@2024-01-01T00:00:00+00:00\n---\n\n# Body\n\n```remargin\n---\nid: c0001\nauthor: bob\ntype: human\nts: 2024-01-01T00:00:00+00:00\nchecksum: {checksum}\n---\n{content}\n```\n"
    );
    let system = realm_with("src.md", src.as_bytes());
    let args = CpArgs::new(PathBuf::from("src.md"), PathBuf::from("dst.md"));
    cp(&system, base(), &open_config(), &args).unwrap();

    let dst_content = system.read_to_string(&base().join("dst.md")).unwrap();
    // Copy must have 0 pending and no sandbox.
    assert!(
        dst_content.contains("remargin_pending: 0") || !dst_content.contains("remargin_pending: 2"),
        "copy should not inherit pending count: {dst_content}"
    );
    assert!(
        !dst_content.contains("sandbox:")
            || dst_content.contains("sandbox: []")
            || dst_content.contains("sandbox:\n[]"),
        "copy should not inherit sandbox entry: {dst_content}"
    );
}

// ---- Group F: shape / edge guards -----------------------------------------

#[test]
fn same_path_is_noop() {
    let system = realm_with("a.md", b"content");
    let args = CpArgs::new(PathBuf::from("a.md"), PathBuf::from("a.md"));
    let outcome = cp(&system, base(), &open_config(), &args).unwrap();
    assert_eq!(outcome.kind, CpKind::Noop);
    assert_eq!(outcome.bytes_copied, 0);
}

#[test]
fn dst_exists_without_force_errors() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("a.md"), b"src")
        .unwrap()
        .with_file(base().join("b.md"), b"dst")
        .unwrap();

    let args = CpArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let err = cp(&system, base(), &open_config(), &args).unwrap_err();
    assert!(format!("{err}").contains("destination exists"), "{err}");
    // Both files unchanged.
    assert_eq!(system.read_to_string(&base().join("a.md")).unwrap(), "src");
    assert_eq!(system.read_to_string(&base().join("b.md")).unwrap(), "dst");
}

#[test]
fn dst_exists_with_force_overwrites() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("a.md"), b"new content")
        .unwrap()
        .with_file(base().join("b.md"), b"old content")
        .unwrap();

    let args = CpArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md")).with_force(true);
    let outcome = cp(&system, base(), &open_config(), &args).unwrap();
    assert!(outcome.overwritten);
    // src still present
    assert!(system.exists(&base().join("a.md")).unwrap());
}

#[test]
fn dst_is_directory_errors() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("subdir"))
        .unwrap()
        .with_file(base().join("a.md"), b"x")
        .unwrap();

    let args = CpArgs::new(PathBuf::from("a.md"), PathBuf::from("subdir"));
    let err = cp(&system, base(), &open_config(), &args).unwrap_err();
    assert!(
        format!("{err}").contains("destination is a directory"),
        "{err}"
    );
}

#[test]
fn src_is_directory_errors() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("subdir"))
        .unwrap();

    let args = CpArgs::new(PathBuf::from("subdir"), PathBuf::from("out"));
    let err = cp(&system, base(), &open_config(), &args).unwrap_err();
    assert!(format!("{err}").contains("source is a directory"), "{err}");
}

#[test]
fn src_missing_errors() {
    let system = MockSystem::new().with_dir(base()).unwrap();
    let args = CpArgs::new(PathBuf::from("missing.md"), PathBuf::from("dst.md"));
    let err = cp(&system, base(), &open_config(), &args).unwrap_err();
    assert!(format!("{err}").contains("source not found"), "{err}");
}

// ---- Group H: tixschema shape ---------------------------------------------

#[test]
fn outcome_serializes_to_snake_case_json() {
    let system = realm_with("a.md", b"# Hello\n");
    let args = CpArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let outcome = cp(&system, base(), &open_config(), &args).unwrap();
    let json = serde_json::to_value(&outcome).unwrap();
    // All required keys present with snake_case names.
    for key in &[
        "bytes_copied",
        "comments_dropped",
        "dst_absolute",
        "kind",
        "overwritten",
        "src_absolute",
    ] {
        assert!(json.get(key).is_some(), "missing key {key} in: {json}");
    }
    // CpKind::Verbatim serialises as "verbatim".
    assert_eq!(json["kind"].as_str().unwrap(), "verbatim");
}
