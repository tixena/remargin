//! Runs `get`, `ls`, `comments` and `search` with the identity flag group.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use tempfile::TempDir;

const ALICE_CONFIG: &str = "identity: alice\ntype: human\nmode: open\n";

const TEST_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";

const TEST_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin";

const DOC: &str = "\
---
title: Read doc
---

# Read doc

Needle body text.

```remargin
---
id: cm1
author: bob
type: human
ts: 2026-04-06T10:00:00-04:00
checksum: sha256:cm1
---
A comment.
```
";

const READ_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("get", &["doc.md"]),
    ("ls", &["."]),
    ("comments", &["doc.md"]),
    ("search", &["Needle"]),
];

fn setup_open_realm() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".remargin.yaml"), ALICE_CONFIG).unwrap();
    fs::write(tmp.path().join("doc.md"), DOC).unwrap();
    let path = tmp.path().to_path_buf();
    (tmp, path)
}

/// Strict realm where the walk resolves `alice` (registered, keyed).
fn setup_strict_realm() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let registry = format!(
        "participants:\n  alice:\n    type: human\n    status: active\n    pubkeys:\n      - {TEST_PUBLIC_KEY}\n"
    );
    fs::write(tmp.path().join(".remargin-registry.yaml"), registry).unwrap();
    fs::write(tmp.path().join("alice_key"), TEST_PRIVATE_KEY).unwrap();
    fs::write(
        tmp.path().join(".remargin.yaml"),
        "identity: alice\ntype: human\nmode: strict\nkey: ./alice_key\n",
    )
    .unwrap();
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

fn run_ok(cwd: &Path, args: &[&str]) -> String {
    let out = run(cwd, args);
    let stderr = str::from_utf8(&out.stderr).unwrap();
    let stdout = str::from_utf8(&out.stdout).unwrap();
    assert!(
        out.status.success(),
        "remargin {args:?} failed\nstderr: {stderr}\nstdout: {stdout}"
    );
    String::from(stdout)
}

#[test]
fn read_subcommands_accept_manual_identity_flags() {
    let (_tmp, cwd) = setup_open_realm();
    for &(cmd, tail) in READ_SUBCOMMANDS {
        let mut args = vec![cmd, "--identity", "alice", "--type", "human"];
        args.extend_from_slice(tail);
        run_ok(&cwd, &args);
    }
}

#[test]
fn read_subcommands_accept_config_flag() {
    let (_tmp, cwd) = setup_open_realm();
    let config_path = cwd.join(".remargin.yaml");
    let config = config_path.to_string_lossy();
    for &(cmd, tail) in READ_SUBCOMMANDS {
        let mut args = vec![cmd, "--config", config.as_ref()];
        args.extend_from_slice(tail);
        run_ok(&cwd, &args);
    }
}

#[test]
fn read_subcommands_unchanged_without_identity_flags() {
    let (_tmp, cwd) = setup_open_realm();
    for &(cmd, tail) in READ_SUBCOMMANDS {
        let mut args = vec![cmd];
        args.extend_from_slice(tail);
        run_ok(&cwd, &args);
    }
}

#[test]
fn read_subcommands_resolve_passed_identity_not_walked_one() {
    // The registry gate rejecting an unregistered identity proves the flags reached resolution;
    // `--key` completes the manual declaration strict mode requires.
    let (_tmp, cwd) = setup_strict_realm();
    for &(cmd, tail) in READ_SUBCOMMANDS {
        let mut args = vec![
            cmd,
            "--identity",
            "mallory",
            "--type",
            "human",
            "--key",
            "./alice_key",
        ];
        args.extend_from_slice(tail);
        let out = run(&cwd, &args);
        assert!(
            !out.status.success(),
            "remargin {args:?} must reject an unregistered passed identity in strict mode"
        );
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("mallory"),
            "stderr must name the passed identity, got: {stderr}"
        );
    }
    for &(cmd, tail) in READ_SUBCOMMANDS {
        let mut args = vec![cmd];
        args.extend_from_slice(tail);
        run_ok(&cwd, &args);
    }
}
