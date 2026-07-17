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
