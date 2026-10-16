use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use os_shim::real::RealSystem;
use remargin_core::crypto::file_hash;
use serde_json::Value;
use tempfile::TempDir;

const STALE: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn vault() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    fs::write(
        root.join(".remargin.yaml"),
        "identity: alice\ntype: human\nmode: open\n",
    )
    .unwrap();
    fs::write(root.join("a.md"), "---\ntitle: T\n---\n\n# Hello\n").unwrap();
    (tmp, root)
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn the_cli_accepts_if_hash_on_write_commands_only() {
    let (_guard, root) = vault();
    let before = file_hash(&RealSystem::new(), &root.join("a.md")).unwrap();

    // A write command with a stale hash: refused, structured, file untouched.
    let refused = run(
        &root,
        &["write", "a.md", "# New\n", "--if-hash", STALE, "--json"],
    );
    assert!(!refused.status.success());
    let err: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(err["error_kind"], "stale_version");
    assert_eq!(err["current_hash"], before);
    assert_eq!(
        file_hash(&RealSystem::new(), &root.join("a.md")).unwrap(),
        before
    );

    // The current hash lets it through.
    let allowed = run(&root, &["write", "a.md", "# New\n", "--if-hash", &before]);
    assert!(allowed.status.success(), "{allowed:?}");

    // A read command does not know the flag.
    let read = run(&root, &["get", "a.md", "--if-hash", STALE]);
    assert!(!read.status.success());
    assert!(String::from_utf8_lossy(&read.stderr).contains("--if-hash"));

    // Without the flag nothing changes.
    let plain = run(&root, &["write", "a.md", "# Again\n"]);
    assert!(plain.status.success(), "{plain:?}");
}

#[test]
fn if_hash_cannot_guard_stdin() {
    let (_guard, root) = vault();
    fs::write(root.join("-"), "decoy\n").unwrap();
    let out = run(&root, &["ack", "x", "--file", "-", "--if-hash", STALE]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--if-hash cannot guard stdin"));
}
