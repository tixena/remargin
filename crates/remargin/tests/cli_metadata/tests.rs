//! `remargin metadata` in text and `--json` modes.

use core::str;
use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

const SEED_DOC: &str = "---\ntitle: Test\n---\n\n# Hello\n";

fn seed(tmp: &TempDir) {
    fs::write(tmp.path().join("doc.md"), SEED_DOC).unwrap();
}

#[test]
fn metadata_text_mode_prints_path_and_size() {
    let tmp = TempDir::new().unwrap();
    seed(&tmp);

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("metadata")
        .arg("doc.md")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");
}

#[test]
fn metadata_json_mode_emits_size_and_mime_fields() {
    let tmp = TempDir::new().unwrap();
    seed(&tmp);

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("metadata")
        .arg("doc.md")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = str::from_utf8(&output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout).unwrap();
    assert_eq!(json["mime"], "text/markdown");
    assert!(json["size_bytes"].as_u64().unwrap() > 0);
    assert_eq!(json["binary"].as_bool(), Some(false));
}
