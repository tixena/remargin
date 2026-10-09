//! `remargin mv` and `plan mv` runs against temp dirs on the real filesystem.

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

/// A same-directory rename lands on disk and reports a non-zero `bytes_moved` in JSON mode.
#[test]
fn renames_within_same_dir_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), b"hello\n").unwrap();

    let out = run_in(realm.path(), &["mv", "a.md", "b.md", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["bytes_moved"], 6_u64);
    assert_eq!(value["overwritten"], json!(false));
    assert_eq!(value["fallback_copy"], json!(false));
    assert_eq!(value["noop_same_path"], json!(false));

    assert!(!realm.path().join("a.md").exists());
    let body = fs::read_to_string(realm.path().join("b.md")).unwrap();
    assert_eq!(body, "hello\n");
}

/// Cross-directory move keeps the bytes intact.
#[test]
fn moves_across_directories_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("notes")).unwrap();
    fs::create_dir_all(realm.path().join("archive")).unwrap();
    fs::write(realm.path().join("notes/foo.md"), b"x").unwrap();

    let out = run_in(
        realm.path(),
        &["mv", "notes/foo.md", "archive/foo.md", "--json"],
    );
    assert_status(&out, 0);

    assert!(!realm.path().join("notes/foo.md").exists());
    assert!(realm.path().join("archive/foo.md").exists());
}

/// `--force` overwrites an existing destination with the source's content.
#[test]
fn force_overwrites_destination_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), b"new\n").unwrap();
    fs::write(realm.path().join("b.md"), b"old\n").unwrap();

    let no_force = run_in(realm.path(), &["mv", "a.md", "b.md"]);
    assert_ne!(no_force.status.code(), Some(0_i32));
    let stderr = String::from_utf8_lossy(&no_force.stderr);
    assert!(
        stderr.contains("destination exists"),
        "expected destination-exists refusal, got: {stderr}"
    );

    assert!(realm.path().join("a.md").exists());

    let with_force = run_in(realm.path(), &["mv", "a.md", "b.md", "--force", "--json"]);
    assert_status(&with_force, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&with_force.stdout).unwrap()).unwrap();
    assert_eq!(value["overwritten"], json!(true));
    assert_eq!(
        fs::read_to_string(realm.path().join("b.md")).unwrap(),
        "new\n"
    );
}

/// A same-path move reports `noop_same_path` and leaves the file alone.
#[test]
fn same_path_is_noop_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), b"unchanged").unwrap();

    let out = run_in(realm.path(), &["mv", "a.md", "a.md", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["noop_same_path"], json!(true));
    assert_eq!(
        fs::read_to_string(realm.path().join("a.md")).unwrap(),
        "unchanged"
    );
}

/// Source gone and destination present: a retried `mv` succeeds with `bytes_moved == 0`.
#[test]
fn idempotent_when_already_settled_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("b.md"), b"already moved").unwrap();

    let out = run_in(realm.path(), &["mv", "a.md", "b.md", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["bytes_moved"], 0_u64);
    assert_eq!(value["overwritten"], json!(false));
    assert_eq!(value["noop_same_path"], json!(false));
}

/// Refuses moves whose source is not visible (sandbox escape).
#[test]
fn refuses_path_escape_via_cli() {
    let realm = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("escape.md"), b"x").unwrap();

    let out = run_in(
        realm.path(),
        &[
            "mv",
            outside.path().join("escape.md").to_str().unwrap(),
            "b.md",
        ],
    );
    assert_ne!(out.status.code(), Some(0_i32));
}

/// The moved document parses with the same comment id and content checksum it had at the source.
#[test]
fn preserves_comments_and_frontmatter_across_rename() {
    let realm = TempDir::new().unwrap();
    let source = "---\ntitle: Sample\n---\n\n# Sample\n\nBody text.\n\n```remargin\n---\nid: aaa111\nauthor: alice\ntype: human\nts: 2026-04-29T10:00:00+00:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n";
    fs::write(realm.path().join("src.md"), source).unwrap();

    let out = run_in(realm.path(), &["mv", "src.md", "dst.md"]);
    assert_status(&out, 0);

    let after = fs::read_to_string(realm.path().join("dst.md")).unwrap();
    assert_eq!(
        after, source,
        "comments + frontmatter must survive byte-for-byte"
    );
}

/// `plan mv` touches nothing and emits the `mv_diff` shape with `would_commit = true`.
#[test]
fn plan_mv_emits_mv_diff() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), b"plan me").unwrap();

    let out = run_in(realm.path(), &["plan", "mv", "a.md", "b.md", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["op"], "mv");
    assert_eq!(value["would_commit"], json!(true));
    assert_eq!(value["noop"], json!(false));
    let mv_diff = &value["mv_diff"];
    assert!(mv_diff.is_object(), "mv_diff missing: {value}");
    assert_eq!(mv_diff["dst_exists"], json!(false));
    assert_eq!(mv_diff["src_exists"], json!(true));
    assert_eq!(mv_diff["noop_same_path"], json!(false));
    assert_eq!(mv_diff["idempotent_already_settled"], json!(false));

    assert!(realm.path().join("a.md").exists());
    assert!(!realm.path().join("b.md").exists());
}

/// An existing destination without `--force` sets `would_commit = false` and a `reject_reason`.
#[test]
fn plan_mv_rejects_existing_destination_without_force() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), b"src").unwrap();
    fs::write(realm.path().join("b.md"), b"dst").unwrap();

    let out = run_in(realm.path(), &["plan", "mv", "a.md", "b.md", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["would_commit"], json!(false));
    assert!(
        value["reject_reason"]
            .as_str()
            .is_some_and(|s| s.contains("destination exists")),
        "missing destination-exists reject reason: {value}"
    );
}

/// With `--force` the same projection reports `would_commit = true`, as the live op would.
#[test]
fn plan_mv_force_clears_existing_destination_rejection() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), b"src").unwrap();
    fs::write(realm.path().join("b.md"), b"dst").unwrap();

    let out = run_in(
        realm.path(),
        &["plan", "mv", "a.md", "b.md", "--force", "--json"],
    );
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["would_commit"], json!(true));
    assert_eq!(value["mv_diff"]["dst_exists"], json!(true));
}

/// A directory rename is filesystem-level, so nested comments survive byte-for-byte.
#[test]
fn renames_directory_with_nested_comments_preserved() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("notes")).unwrap();
    let source = "---\ntitle: Sample\n---\n\n# Sample\n\nBody text.\n\n```remargin\n---\nid: aaa111\nauthor: alice\ntype: human\nts: 2026-04-29T10:00:00+00:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n";
    fs::write(realm.path().join("notes/a.md"), source).unwrap();
    fs::write(realm.path().join("notes/plain.txt"), b"plain bytes").unwrap();

    let out = run_in(realm.path(), &["mv", "notes", "archive", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["is_directory"], json!(true));
    assert_eq!(value["nested_files_moved"], 2_u64);

    let after = fs::read_to_string(realm.path().join("archive/a.md")).unwrap();
    assert_eq!(after, source);
    assert_eq!(
        fs::read_to_string(realm.path().join("archive/plain.txt")).unwrap(),
        "plain bytes"
    );
    assert!(!realm.path().join("notes").exists());
}

#[test]
fn directory_same_path_is_noop_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("notes")).unwrap();
    fs::write(realm.path().join("notes/a.md"), b"keep").unwrap();

    let out = run_in(realm.path(), &["mv", "notes", "notes", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["noop_same_path"], json!(true));
    assert_eq!(value["is_directory"], json!(true));
    assert!(realm.path().join("notes/a.md").exists());
}

#[test]
fn directory_force_overwrites_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("src")).unwrap();
    fs::create_dir_all(realm.path().join("dst")).unwrap();
    fs::write(realm.path().join("src/a.md"), b"new").unwrap();
    fs::write(realm.path().join("dst/old.md"), b"erased").unwrap();

    let out = run_in(realm.path(), &["mv", "src", "dst", "--force", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["overwritten"], json!(true));
    assert_eq!(value["is_directory"], json!(true));
    assert!(!realm.path().join("src").exists());
    assert!(!realm.path().join("dst/old.md").exists());
    assert_eq!(
        fs::read_to_string(realm.path().join("dst/a.md")).unwrap(),
        "new"
    );
}

/// `plan mv <dir> <new>` reports `is_directory` and the nested-file count, writing nothing.
#[test]
fn plan_mv_for_directory_emits_is_directory() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("src")).unwrap();
    fs::write(realm.path().join("src/a.md"), b"x").unwrap();
    fs::write(realm.path().join("src/b.md"), b"y").unwrap();

    let out = run_in(realm.path(), &["plan", "mv", "src", "dst", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["op"], "mv");
    assert_eq!(value["would_commit"], json!(true));
    let mv_diff = &value["mv_diff"];
    assert_eq!(mv_diff["is_directory"], json!(true));
    assert_eq!(mv_diff["nested_files_moved"], 2_u64);
    assert_eq!(mv_diff["dst_exists"], json!(false));
    assert_eq!(mv_diff["src_exists"], json!(true));

    assert!(realm.path().join("src/a.md").exists());
    assert!(!realm.path().join("dst").exists());
}

/// `restrict` writes no `Bash(mv ...)` rules; the `PreToolUse` hook covers `mv` on managed paths.
#[test]
fn restrict_projects_no_mv_deny_set() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join(".claude")).unwrap();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");

    let out = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&out, 0);

    assert!(
        !realm.path().join(".claude/settings.local.json").exists(),
        "no project-scope settings should be projected"
    );
    assert!(
        !user_settings.exists(),
        "no user-scope settings should be projected"
    );
}
