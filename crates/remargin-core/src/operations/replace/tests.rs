//! Tests for the body-only find/replace engine.
//!
//! The dedicated comment-safety tests (`comment_also_contains_pattern`
//! and `comment_only_match_is_noop`) assert byte-level comment
//! preservation — the serialized comment block and its `checksum` field
//! must be identical before and after — not merely that the op
//! succeeded.

use std::path::Path;

use os_shim::System as _;
use os_shim::mock::MemorySystem;

use super::{ReplaceOptions, replace};
use crate::config::{Mode, ResolvedConfig};
use crate::crypto::compute_checksum;
use crate::parser::AuthorType;

/// Open-mode config rooted at `/project` for a `human` identity.
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

/// A remargin comment block whose stored checksum matches `content`, so
/// the file carries zero pre-existing anomalies and the verify gate is
/// satisfied by construction.
fn remargin_block(id: &str, content: &str) -> String {
    let checksum = compute_checksum(content, &[]);
    format!(
        "```remargin\n\
         ---\n\
         id: {id}\n\
         author: eduardo\n\
         type: human\n\
         ts: 2026-04-06T14:32:00-04:00\n\
         checksum: {checksum}\n\
         ---\n\
         {content}\n\
         ```\n"
    )
}

fn system_with(path: &str, body: &str) -> MemorySystem {
    MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new(path), body.as_bytes())
        .unwrap()
}

fn read(system: &MemorySystem, path: &str) -> String {
    system.read_to_string(Path::new(path)).unwrap()
}

fn opts(pattern: &str, replacement: &str) -> ReplaceOptions {
    ReplaceOptions::new(String::from(pattern), String::from(replacement))
}

#[test]
fn literal_body_replace() {
    let system = system_with("/project/doc.md", "# Title\n\nThe foo system.\n");
    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("foo", "bar"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 1);
    assert_eq!(report.files_changed, 1);
    assert_eq!(report.files_failed, 0);
    assert!(report.files[0].changed);
    assert!(read(&system, "/project/doc.md").contains("The bar system."));
}

/// The comment block must be byte-identical afterwards.
#[test]
fn comment_also_contains_pattern() {
    let comment = remargin_block("c1", "Remember to handle foo here.");
    let body = format!("# Title\n\nThe foo system.\n\n{comment}");
    let system = system_with("/project/doc.md", &body);

    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("foo", "bar"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 1);
    let after = read(&system, "/project/doc.md");
    assert!(after.contains("The bar system."));

    assert!(
        after.contains(&comment),
        "comment block must be byte-identical; got:\n{after}"
    );
    assert!(
        after.contains("Remember to handle foo here."),
        "comment content must still contain the original pattern"
    );
    assert!(
        after.contains(&format!(
            "checksum: {}",
            compute_checksum("Remember to handle foo here.", &[])
        )),
        "comment checksum field must be unchanged"
    );
}

#[test]
fn comment_only_match_is_noop() {
    let comment = remargin_block("c1", "The foo lives only here.");
    let body = format!("# Title\n\nNo match in body.\n\n{comment}");
    let system = system_with("/project/doc.md", &body);
    let before = read(&system, "/project/doc.md");

    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("foo", "bar"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 0);
    assert_eq!(report.files_changed, 0);
    assert!(!report.files[0].changed);
    assert_eq!(read(&system, "/project/doc.md"), before);
}

#[test]
fn regex_capture_group() {
    let system = system_with("/project/doc.md", "build id=42 here\n");
    let options = opts(r"id=(\d+)", "id=[$1]").regex(true);
    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &options,
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 1);
    assert!(read(&system, "/project/doc.md").contains("id=[42]"));
}

/// A `$` in a literal replacement is inserted verbatim, not read as a capture reference.
#[test]
fn literal_replacement_with_dollar() {
    let system = system_with("/project/doc.md", "the price tag\n");
    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("price", "$5"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 1);
    assert!(read(&system, "/project/doc.md").contains("the $5 tag"));
}

#[test]
fn case_insensitive() {
    let system = system_with("/project/doc.md", "Foo and foo\n");
    let options = opts("foo", "bar").ignore_case(true);
    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &options,
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 2);
    assert!(read(&system, "/project/doc.md").contains("bar and bar"));
}

#[test]
fn folder_walk_skips_non_markdown() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project/d"))
        .unwrap()
        .with_dir(Path::new("/project/d/sub"))
        .unwrap()
        .with_file(Path::new("/project/d/a.md"), b"foo a\n")
        .unwrap()
        .with_file(Path::new("/project/d/sub/b.md"), b"foo b\n")
        .unwrap()
        .with_file(Path::new("/project/d/c.png"), b"foo png\n")
        .unwrap();

    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("d"),
        &opts("foo", "bar"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.files_changed, 2);
    assert_eq!(report.total_replacements, 2);
    assert!(read(&system, "/project/d/a.md").contains("bar a"));
    assert!(read(&system, "/project/d/sub/b.md").contains("bar b"));
    assert_eq!(read(&system, "/project/d/c.png"), "foo png\n");
}

#[test]
fn dry_run_writes_nothing() {
    let system = system_with("/project/doc.md", "foo foo foo\n");
    let before = read(&system, "/project/doc.md");
    let options = opts("foo", "bar").dry_run(true);

    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &options,
        &open_config(),
    )
    .unwrap();

    assert!(report.dry_run);
    assert_eq!(report.total_replacements, 3);
    assert_eq!(report.files_changed, 1);
    assert!(report.files[0].changed);
    assert_eq!(read(&system, "/project/doc.md"), before);
}

#[test]
fn no_matches() {
    let system = system_with("/project/doc.md", "nothing here\n");
    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("foo", "bar"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 0);
    assert_eq!(report.files_changed, 0);
    assert!(!report.files[0].changed);
}

/// An injected remargin fence re-parses as a new comment and is refused before any write.
#[test]
fn injecting_comment_fence_is_refused() {
    let system = system_with("/project/doc.md", "MARK\nbody\n");
    let before = read(&system, "/project/doc.md");

    let injected = remargin_block("evil", "injected");
    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("MARK", injected.trim_end()),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.files_failed, 1);
    assert_eq!(report.files_changed, 0);
    assert!(report.files[0].error.is_some());
    assert_eq!(read(&system, "/project/doc.md"), before);
}

#[test]
fn deny_ops_governs_replace() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            b"permissions:\n  deny_ops:\n    - path: doc.md\n      ops: [replace]\n",
        )
        .unwrap()
        .with_file(Path::new("/r/doc.md"), b"foo here\n")
        .unwrap();
    let before = system.read_to_string(Path::new("/r/doc.md")).unwrap();

    let report = replace(
        &system,
        Path::new("/r"),
        Path::new("doc.md"),
        &opts("foo", "bar"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.files_failed, 1);
    let err = report.files[0].error.as_deref().unwrap();
    assert!(
        err.contains("replace") && err.contains("deny_ops"),
        "denial must cite the canonical replace deny_ops wording; got: {err}"
    );
    assert_eq!(
        system.read_to_string(Path::new("/r/doc.md")).unwrap(),
        before
    );
}

#[test]
fn trusted_roots_governs_replace() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/src"))
        .unwrap()
        .with_dir(Path::new("/r/src/secret"))
        .unwrap()
        .with_dir(Path::new("/r/src/public"))
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            b"permissions:\n  trusted_roots:\n    - path: src/secret\n",
        )
        .unwrap()
        .with_file(Path::new("/r/src/public/doc.md"), b"foo here\n")
        .unwrap();
    let before = system
        .read_to_string(Path::new("/r/src/public/doc.md"))
        .unwrap();

    let report = replace(
        &system,
        Path::new("/r"),
        Path::new("src/public/doc.md"),
        &opts("foo", "bar"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.files_failed, 1);
    let err = report.files[0].error.as_deref().unwrap();
    assert!(
        err.contains("trusted_roots"),
        "denial must cite trusted_roots; got: {err}"
    );
    assert_eq!(
        system
            .read_to_string(Path::new("/r/src/public/doc.md"))
            .unwrap(),
        before
    );
}

/// The bad file is recorded, the rest are changed, and the op returns Ok.
#[test]
fn one_bad_file_in_folder_continues() {
    let injected = remargin_block("evil", "injected");
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project/d"))
        .unwrap()
        .with_file(Path::new("/project/d/good.md"), b"MARK ok\n")
        .unwrap()
        .with_file(Path::new("/project/d/bad.md"), b"MARK bad\n")
        .unwrap();

    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("d"),
        &opts("MARK bad", injected.trim_end()),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.files_failed, 1);
    let bad = report
        .files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("bad.md"))
        .unwrap();
    assert!(bad.error.is_some());
    assert_eq!(read(&system, "/project/d/good.md"), "MARK ok\n");
}

#[test]
fn idempotent_rerun() {
    let system = system_with("/project/doc.md", "foo and foo\n");
    let base = Path::new("/project");
    let target = Path::new("doc.md");

    let first = replace(&system, base, target, &opts("foo", "bar"), &open_config()).unwrap();
    assert_eq!(first.files_changed, 1);
    assert_eq!(first.total_replacements, 2);

    let second = replace(&system, base, target, &opts("foo", "bar"), &open_config()).unwrap();
    assert_eq!(second.files_changed, 0);
    assert_eq!(second.total_replacements, 0);
}

/// Prose and ordinary fences are adjacent `Body` segments, coalesced so a match can span them.
#[test]
fn replace_matches_pattern_spanning_a_code_fence() {
    let system = system_with("/project/doc.md", "before\n\n```bash\ncmd\n```\nafter\n");
    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("before\n\n```bash", "BEFORE\n\n```bash"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(
        report.total_replacements, 1,
        "cross-fence pattern must match"
    );
    let after = read(&system, "/project/doc.md");
    assert!(after.contains("BEFORE\n\n```bash\ncmd\n```"));
}

/// A comment block stays a hard boundary that a match can never cross.
#[test]
fn replace_still_refuses_to_cross_a_remargin_comment_block() {
    let comment = remargin_block("a1", "hi");
    let body = format!("before\n\n{comment}after\n");
    let system = system_with("/project/doc.md", &body);
    let before = read(&system, "/project/doc.md");

    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("before\n\n```remargin", "X"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(
        report.total_replacements, 0,
        "must not match across a comment block"
    );
    assert_eq!(read(&system, "/project/doc.md"), before);
}

#[test]
fn absent_pattern_leaves_file_byte_identical() {
    let system = system_with(
        "/project/doc.md",
        "before\n\n```bash\ncmd\n```\n\n```yaml\nk: v\n```\nafter\n",
    );
    let before = read(&system, "/project/doc.md");

    let report = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("this pattern does not exist", "x"),
        &open_config(),
    )
    .unwrap();

    assert_eq!(report.total_replacements, 0);
    assert_eq!(report.files_changed, 0);
    assert!(!report.files[0].changed);
    assert_eq!(read(&system, "/project/doc.md"), before);
}

#[test]
fn empty_pattern_rejected() {
    let system = system_with("/project/doc.md", "foo\n");
    let result = replace(
        &system,
        Path::new("/project"),
        Path::new("doc.md"),
        &opts("", "bar"),
        &open_config(),
    );
    result.unwrap_err();
}
