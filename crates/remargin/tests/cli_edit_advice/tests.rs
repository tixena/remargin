//! Style notes `edit` reports for a replacement body, in text and `--json` modes.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const ALICE_CONFIG: &str = "identity: alice\ntype: human\nmode: open\n";

const DOC: &str = "\
---
title: Edit advice
---

# Edit advice

Body.
";

const CLEAN_BODY: &str = "A clean single-line body.";

/// Earns a warn-tier note: it names a comment by its id.
const WARNED_BODY: &str = "See a5q for the field list.";

fn setup_vault() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    fs::write(root.join(".remargin.yaml"), ALICE_CONFIG).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/a.md"), DOC).unwrap();
    (tmp, root)
}

fn seed_comment(root: &Path) -> String {
    let out = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(root)
        .arg("comment")
        .arg("docs/a.md")
        .arg(CLEAN_BODY)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let payload: Value = serde_json::from_str(stdout(&out)).unwrap();
    payload["id"].as_str().unwrap().to_owned()
}

fn run_edit(root: &Path, id: &str, content: &str, json_mode: bool) -> Output {
    let mut cmd = Command::cargo_bin("remargin").unwrap();
    cmd.current_dir(root)
        .arg("edit")
        .arg("docs/a.md")
        .arg(id)
        .arg(content);
    if json_mode {
        cmd.arg("--json");
    }
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> &str {
    str::from_utf8(&out.stdout).unwrap()
}

fn stderr(out: &Output) -> &str {
    str::from_utf8(&out.stderr).unwrap()
}

#[test]
fn text_mode_prints_the_note_on_stderr() {
    let (_guard, root) = setup_vault();
    let id = seed_comment(&root);

    let out = run_edit(&root, &id, WARNED_BODY, false);

    assert!(out.status.success(), "stderr={}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "advice: line 1: \"a5q\" reads as a comment id; quote or paraphrase what \
         that comment said instead, so the reference stands on its own\n"
    );
    assert!(
        !stdout(&out).contains("advice"),
        "advice never contaminates stdout: {}",
        stdout(&out)
    );
}

#[test]
fn json_mode_carries_the_note_in_the_payload() {
    let (_guard, root) = setup_vault();
    let id = seed_comment(&root);

    let out = run_edit(&root, &id, WARNED_BODY, true);

    assert!(out.status.success(), "stderr={}", stderr(&out));
    assert_eq!(stderr(&out), "", "--json keeps advice out of stderr");

    let payload: Value = serde_json::from_str(stdout(&out)).unwrap();
    assert_eq!(payload["edited"], id);
    let warnings = payload["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["line"], 1_u64);
    assert!(
        warnings[0]["message"].as_str().unwrap().contains("a5q"),
        "{warnings:?}"
    );
}

#[test]
fn a_clean_edit_says_nothing_on_either_stream() {
    let (_guard, root) = setup_vault();
    let id = seed_comment(&root);

    let out = run_edit(&root, &id, "Another one, equally clean.", true);

    assert!(out.status.success(), "stderr={}", stderr(&out));
    assert_eq!(stderr(&out), "");

    let payload: Value = serde_json::from_str(stdout(&out)).unwrap();
    assert_eq!(payload["edited"], id);
    assert!(
        payload.get("warnings").is_none(),
        "a clean edit keeps the payload it has always had: {payload}"
    );
}
