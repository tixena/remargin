//! `remargin query --json` payload shape and `base_path`, run against temp realms.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};

use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const CONFIG: &str = "identity: alice\ntype: human\nmode: open\n";

const DOC: &str = "\
---
title: Query
---

```remargin
---
id: aaa
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
to: [bob]
checksum: sha256:aaa
---
First comment.
```

```remargin
---
id: bbb
author: bob
type: agent
ts: 2026-04-06T11:00:00-04:00
reply-to: aaa
thread: aaa
checksum: sha256:bbb
ack:
  - alice@2026-04-06T12:00:00-04:00
---
Reply comment.
```
";

fn setup() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".remargin.yaml"), CONFIG).unwrap();
    fs::write(tmp.path().join("doc.md"), DOC).unwrap();
    let path = tmp.path().to_path_buf();
    (tmp, path)
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

/// `--json` is verbose: named-field comment objects carrying a checksum, no columnar header.
#[test]
fn query_verbose_json_unchanged() {
    let (_tmp, cwd) = setup();
    let out = run(&cwd, &["query", ".", "--json"]);
    assert!(out.status.success(), "command failed: {out:?}");

    let raw = str::from_utf8(&out.stdout).unwrap();
    assert!(
        raw.lines().count() > 3,
        "verbose stays pretty-printed: {raw:?}"
    );

    let payload: Value = serde_json::from_str(raw).unwrap();
    assert!(payload.get("comment_cols").is_none());
    let comments = payload["results"][0]["comments"].as_array().unwrap();
    let first = comments[0].as_object().unwrap();
    assert!(first.contains_key("id"));
    assert!(first.contains_key("checksum"));
    assert!(first.contains_key("file"));
}

/// A realm holding the same document at the root and one level down, so a
/// file argument can be checked with and without a parent directory.
fn setup_nested() -> (TempDir, PathBuf) {
    let (tmp, cwd) = setup();
    fs::create_dir_all(cwd.join("notes")).unwrap();
    fs::write(cwd.join("notes/nested.md"), DOC).unwrap();
    (tmp, cwd)
}

fn base_path_of(payload: &Value) -> &str {
    payload["base_path"].as_str().unwrap()
}

#[test]
fn query_verbose_json_file_base_path_is_parent_directory() {
    let (_tmp, cwd) = setup_nested();
    let out = run(&cwd, &["query", "notes/nested.md", "--json"]);
    assert!(out.status.success(), "command failed: {out:?}");

    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(base_path_of(&payload), "notes/");
}

#[test]
fn compact_flag_no_longer_exists() {
    let (_tmp, root) = setup();
    let out = run(&root, &["query", ".", "--json", "--compact"]);
    assert!(!out.status.success());
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(stderr.contains("--compact"), "{stderr}");
}
