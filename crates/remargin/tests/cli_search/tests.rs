use core::str;
use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

const DOC: &str = "---\ntitle: Test\n---\n\n# Hello\n\nneedle is in the body\n";

const NEEDLES: &str = "# Hello\n\nneedle 1\nneedle 2\nneedle 3\nneedle 4\nneedle 5\n";

#[test]
fn search_finds_literal_pattern_in_body() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), DOC).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("needle"),
        "stdout missing match: {stdout:?}"
    );
}

#[test]
fn search_json_mode_emits_match_array() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), DOC).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = str::from_utf8(&output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout).unwrap();
    let matches = json["matches"].as_array().unwrap();
    assert!(!matches.is_empty(), "expected matches: {stdout}");
}

#[test]
fn search_limit_offset_json_carries_total() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), NEEDLES).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .arg("--limit")
        .arg("2")
        .arg("--offset")
        .arg("1")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = str::from_utf8(&output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout).unwrap();
    let matches = json["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2, "expected a 2-match page: {stdout}");
    assert_eq!(
        json["total"].as_u64().unwrap(),
        5,
        "expected total 5: {stdout}"
    );
}

#[test]
fn search_compact_emits_grouped_minified_payload() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), DOC).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .arg("--json")
        .arg("--compact")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let raw = str::from_utf8(&output.stdout).unwrap();
    // Minified: only the trailing newline breaks the single payload line.
    assert_eq!(
        raw.trim_end_matches('\n').lines().count(),
        1,
        "compact payload must be minified: {raw:?}"
    );

    let payload: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    // Grouped columnar shape `{total, match_cols, files}` — matches the MCP
    // contract. No verbose top-level `matches`.
    assert!(payload.get("matches").is_none());
    let cols = payload["match_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 4);
    assert_eq!(cols[3], "comment_id");

    let files = payload["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"].as_str().unwrap(), "doc.md");
    let row = files[0]["matches"][0].as_array().unwrap();
    // Body match: lowercase location, null comment_id column.
    assert_eq!(row[1], "body");
    assert!(row[3].is_null());
    assert!(row[2].as_str().unwrap().contains("needle"));
}

#[test]
fn search_compact_context_widens_columns() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), NEEDLES).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .arg("-C")
        .arg("1")
        .arg("--json")
        .arg("--compact")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let payload: serde_json::Value =
        serde_json::from_str(str::from_utf8(&output.stdout).unwrap().trim()).unwrap();
    let cols = payload["match_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 6);
    assert_eq!(cols[4], "before");
    assert_eq!(cols[5], "after");
    // Rows widen to 6-tuples with before / after string arrays.
    let row = payload["files"][0]["matches"][0].as_array().unwrap();
    assert_eq!(row.len(), 6);
    assert!(row[4].is_array());
    assert!(row[5].is_array());
}

/// Regression: verbose `--json` (no `--compact`) stays byte-identical to
/// the pre-change payload — flat `SearchMatch` objects with `PascalCase`
/// `location`, always-present `before` / `after`, string `path`, and the
/// `{matches, total}` envelope. The Obsidian plugin parses exactly this.
#[test]
fn search_verbose_json_unchanged_by_compact() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), DOC).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let payload: serde_json::Value =
        serde_json::from_str(str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert!(payload.get("match_cols").is_none());
    assert!(payload.get("files").is_none());
    let matches = payload["matches"].as_array().unwrap();
    let first = matches[0].as_object().unwrap();
    assert_eq!(first["location"].as_str().unwrap(), "Body");
    assert!(first.contains_key("before"));
    assert!(first.contains_key("after"));
    assert_eq!(first["path"].as_str().unwrap(), "doc.md");
}

#[test]
fn search_compact_requires_json() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), DOC).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .arg("--compact")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "expected clap failure: {output:?}"
    );
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("--json"),
        "clap requires error must name --json: {stderr}"
    );
}

#[test]
fn search_paged_human_output_footers_total() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doc.md"), NEEDLES).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("search")
        .arg("needle")
        .arg("--limit")
        .arg("2")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("showing 2 of 5"),
        "stdout missing paging footer: {stdout:?}"
    );
}
