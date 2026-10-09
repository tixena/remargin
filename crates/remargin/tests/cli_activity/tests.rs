//! `remargin activity` output modes, the `--since` cutoff and flag validation.

use core::str;
use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

fn realm_with(files: &[(&str, &str)]) -> TempDir {
    let realm = TempDir::new().unwrap();
    fs::write(
        realm.path().join(".remargin.yaml"),
        "identity: alice\ntype: human\n",
    )
    .unwrap();
    for (rel, body) in files {
        let path = realm.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
    }
    realm
}

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

fn doc(id: &str, author: &str, ts: &str) -> String {
    format!(
        "---\ntitle: t\n---\n\n# Body\n\n```remargin\n---\nid: {id}\nauthor: {author}\ntype: human\nts: {ts}\nchecksum: sha256:t\n---\nBody.\n```\n"
    )
}

/// With no mode flag, `remargin activity` prints the `ActivityResult` as JSON on stdout.
#[test]
fn json_output_is_default() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &["activity", "--identity", "alice", "--type", "human"],
    );
    assert_status(&out, 0);
    let stdout = str::from_utf8(&out.stdout).unwrap();
    let value: Value = serde_json::from_str(stdout).unwrap();
    let files = value["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let changes = files[0]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["kind"], json!("comment"));
}

/// `--pretty` renders the timeline on stderr, each per-file block opening with its cutoff header.
#[test]
fn pretty_output_renders_timeline() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--pretty",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(stderr.contains("comment"), "{stderr}");
    assert!(stderr.contains("c1 by bob"), "{stderr}");
    assert!(
        stderr.contains("since the beginning"),
        "expected initial-touch fallback header in: {stderr}"
    );
    assert!(
        !stderr.contains("YOUR-LAST-ACTION"),
        "header must not leak the placeholder string: {stderr}"
    );
}

/// An explicit `--since` is echoed in the `--pretty` header line.
#[test]
fn pretty_output_renders_explicit_since_header() {
    let realm = realm_with(&[
        ("a.md", &doc("c1", "bob", "2026-04-08T12:00:00-04:00")),
        ("b.md", &doc("c2", "bob", "2026-04-06T12:00:00-04:00")),
    ]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--pretty",
            "--since",
            "2026-04-07T00:00:00-04:00",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(
        stderr.contains("(since 2026-04-07 00:00)"),
        "expected explicit-since header in: {stderr}"
    );
}

/// `--since` parses ISO 8601 and drops a comment older than the cutoff.
#[test]
fn since_cutoff_filters_comments() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--since",
            "2026-04-06T13:00:00-04:00",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(
        value["files"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );
}

#[test]
fn malformed_since_errors() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--since",
            "not-a-date",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(stderr.contains("--since"), "{stderr}");
}

#[test]
fn pretty_and_json_are_mutually_exclusive() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--pretty",
            "--json",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(stderr.contains("mutually exclusive"), "{stderr}");
}

/// `--json` keeps the verbose shape: pretty-printed, tagged `Change` objects with named fields.
#[test]
fn cli_activity_verbose_json_unchanged() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--json",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let raw = str::from_utf8(&out.stdout).unwrap();
    assert!(raw.lines().count() > 3, "pretty-printed: {raw:?}");

    let payload: Value = serde_json::from_str(raw).unwrap();
    assert!(payload.get("change_cols").is_none(), "no columnar header");
    let change = &payload["files"][0]["changes"][0];
    assert!(change.is_object(), "verbose change is an object: {change}");
    assert_eq!(change["kind"], json!("comment"));
    assert_eq!(change["comment_id"], json!("c1"));
    assert!(change.get("line_start").is_some());
}
