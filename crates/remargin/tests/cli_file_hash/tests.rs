//! `hash` on `--json` reads and writes: the sha256 of the file's bytes.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use os_shim::real::RealSystem;
use remargin_core::crypto::file_hash;
use serde_json::Value;
use tempfile::TempDir;

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

fn run_json(root: &Path, args: &[&str]) -> Value {
    let out = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(root)
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

fn on_disk(root: &Path, name: &str) -> String {
    file_hash(&RealSystem::new(), &root.join(name)).unwrap()
}

#[test]
fn json_reads_return_the_file_hash() {
    let (_guard, root) = vault();
    let want = on_disk(&root, "a.md");
    for args in [
        &["get", "a.md"][..],
        &["get", "a.md", "-n"],
        &["metadata", "a.md"],
        &["comments", "a.md"],
    ] {
        assert_eq!(run_json(&root, args)["hash"], want, "{args:?}");
    }
}

#[test]
fn json_writes_return_the_hash_of_what_they_stored() {
    let (_guard, root) = vault();

    let wrote = run_json(&root, &["write", "a.md", "# New\n\nBody.\n"]);
    assert_eq!(wrote["hash"], on_disk(&root, "a.md"));

    let created = run_json(&root, &["comment", "a.md", "A comment."]);
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["hash"], on_disk(&root, "a.md"));

    for args in [
        vec!["react", "a.md", &id, "+1"],
        vec!["edit", "a.md", &id, "Edited."],
        vec!["ack", &id, "--file", "a.md"],
        vec!["delete", "a.md", &id],
    ] {
        assert_eq!(
            run_json(&root, &args)["hash"],
            on_disk(&root, "a.md"),
            "{args:?}"
        );
    }

    let copied = run_json(&root, &["cp", "a.md", "b.md"]);
    assert_eq!(copied["hash"], on_disk(&root, "b.md"));
    let moved = run_json(&root, &["mv", "b.md", "c.md"]);
    assert_eq!(moved["hash"], on_disk(&root, "c.md"));

    // rm deletes the file: no hash.
    assert!(run_json(&root, &["rm", "c.md"]).get("hash").is_none());
}

#[test]
fn text_output_of_metadata_and_write_has_no_hash_line() {
    let (_guard, root) = vault();
    for args in [
        &["metadata", "a.md"][..],
        &["write", "a.md", "# New\n\nBody.\n"],
    ] {
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(!text.contains("hash"), "{args:?}: {text}");
    }
}

#[test]
fn ack_by_id_hashes_only_when_every_id_is_in_one_file() {
    let (_guard, root) = vault();
    fs::write(root.join("b.md"), "# B\n").unwrap();
    let a = run_json(&root, &["comment", "a.md", "one"])["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let a2 = run_json(&root, &["comment", "a.md", "two"])["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = run_json(&root, &["comment", "b.md", "three"])["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let same = run_json(&root, &["ack", &a, &a2]);
    assert_eq!(same["hash"], on_disk(&root, "a.md"));

    let split = run_json(&root, &["ack", &a, &b, "--remove"]);
    assert!(split.get("hash").is_none(), "{split}");
}
