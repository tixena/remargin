//! `remargin purge --recursive` and its plan projection, run against temp dirs.

use core::str;
use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn assert_status(out: &Output, expected: i32) {
    let actual = out.status.code();
    assert_eq!(
        actual,
        Some(expected),
        "remargin exited with {:?}\nstdout: {}\nstderr: {}",
        actual,
        str::from_utf8(&out.stdout).unwrap(),
        str::from_utf8(&out.stderr).unwrap(),
    );
}

fn doc_with_one_comment() -> &'static str {
    "---\ntitle: Sample\n---\n\n# Sample\n\nBody text.\n\n```remargin\n---\nid: aaa111\nauthor: alice\ntype: human\nts: 2026-04-29T10:00:00+00:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n"
}

/// `purge --recursive <dir>` purges every `.md` file and reports per-file outcomes in JSON.
#[test]
fn recursive_purge_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("notes")).unwrap();
    fs::write(realm.path().join("a.md"), doc_with_one_comment()).unwrap();
    fs::write(realm.path().join("notes/b.md"), doc_with_one_comment()).unwrap();

    let out = run_in(realm.path(), &["purge", "--recursive", ".", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(
        value["comments_removed"], 2_u64,
        "should report total across files: {value}"
    );
    let purged = value["purged"].as_array().unwrap();
    assert_eq!(purged.len(), 2);
    assert_eq!(
        value["failed"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );
    assert_eq!(
        value["skipped"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );

    for file in ["a.md", "notes/b.md"] {
        let body = fs::read_to_string(realm.path().join(file)).unwrap();
        assert!(
            !body.contains("```remargin"),
            "{file} should have no remargin block: {body}"
        );
    }
}

/// Without `--recursive` a directory is rejected, so the destructive form is always opted into.
#[test]
fn dir_target_without_recursive_errors() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("notes")).unwrap();
    fs::write(realm.path().join("notes/a.md"), doc_with_one_comment()).unwrap();

    let out = run_in(realm.path(), &["purge", "notes"]);
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("directory") && stderr.contains("--recursive"),
        "expected directory-without-recursive error, got: {stderr}"
    );

    let body = fs::read_to_string(realm.path().join("notes/a.md")).unwrap();
    assert!(body.contains("```remargin"));
}

/// A missing directory exits non-zero, so callers can tell it from an empty one.
#[test]
fn missing_directory_errors() {
    let realm = TempDir::new().unwrap();

    let out = run_in(realm.path(), &["purge", "--recursive", "missing-dir"]);
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not exist"),
        "expected missing-dir error, got: {stderr}"
    );
}

/// `plan purge --recursive <dir>` reports per-file projections without writing to disk.
#[test]
fn plan_recursive_purge_emits_purge_dir_diff() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), doc_with_one_comment()).unwrap();
    fs::write(realm.path().join("b.md"), doc_with_one_comment()).unwrap();

    let out = run_in(
        realm.path(),
        &["plan", "purge", "--recursive", ".", "--json"],
    );
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["op"], "purge");
    assert_eq!(value["would_commit"], json!(true));
    assert_eq!(value["noop"], json!(false));

    let diff = &value["purge_dir_diff"];
    assert!(diff.is_object(), "purge_dir_diff missing: {value}");
    let files = diff["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for file in files {
        assert_eq!(file["outcome"], "would_purge");
        assert_eq!(file["comments_removed"], 1_u64);
    }

    for file in ["a.md", "b.md"] {
        let body = fs::read_to_string(realm.path().join(file)).unwrap();
        assert!(
            body.contains("```remargin"),
            "plan must not write {file}: {body}"
        );
    }
}

/// Empty / zero-md directory is a successful no-op exit 0.
#[test]
fn empty_dir_recursive_purge_succeeds() {
    let realm = TempDir::new().unwrap();

    let out = run_in(realm.path(), &["purge", "--recursive", ".", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["comments_removed"], 0_u64);
    assert_eq!(
        value["purged"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );
}
