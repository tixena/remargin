//! Style notes `batch` reports per operation, in text and `--json` modes.

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
title: Batch advice
---

# Batch advice

Body.
";

/// Only the second op earns a note: it names a comment by its id.
const OPS_WITH_ONE_WARNED_BODY: &str = r#"[
  { "content": "A clean single-line body." },
  { "content": "See a5q for the field list." }
]"#;

const CLEAN_OPS: &str = r#"[
  { "content": "A clean single-line body." },
  { "content": "Another one, equally clean." }
]"#;

fn setup_vault() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    fs::write(root.join(".remargin.yaml"), ALICE_CONFIG).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/a.md"), DOC).unwrap();
    (tmp, root)
}

fn run_batch(root: &Path, ops: &str, json_mode: bool) -> Output {
    let mut cmd = Command::cargo_bin("remargin").unwrap();
    cmd.current_dir(root)
        .arg("batch")
        .arg("docs/a.md")
        .arg("--ops")
        .arg(ops);
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
fn text_mode_prints_the_op_scoped_note_on_stderr() {
    let (_guard, root) = setup_vault();

    let out = run_batch(&root, OPS_WITH_ONE_WARNED_BODY, false);

    assert!(out.status.success(), "stderr={}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "advice: op 1, line 1: \"a5q\" reads as a comment id; quote or paraphrase what \
         that comment said instead, so the reference stands on its own\n"
    );
    assert!(
        stdout(&out).starts_with("ids:"),
        "stdout keeps the ids block it has always had: {}",
        stdout(&out)
    );
    assert!(
        !stdout(&out).contains("advice"),
        "advice never contaminates stdout: {}",
        stdout(&out)
    );
}

#[test]
fn json_mode_carries_the_op_scoped_note_in_the_payload() {
    let (_guard, root) = setup_vault();

    let out = run_batch(&root, OPS_WITH_ONE_WARNED_BODY, true);

    assert!(out.status.success(), "stderr={}", stderr(&out));
    assert_eq!(stderr(&out), "", "--json keeps advice out of stderr");

    let payload: Value = serde_json::from_str(stdout(&out)).unwrap();
    assert_eq!(payload["ids"].as_array().unwrap().len(), 2);
    let warnings = payload["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["op"], 1_u64);
    assert_eq!(warnings[0]["line"], 1_u64);
    assert!(
        warnings[0]["message"].as_str().unwrap().contains("a5q"),
        "{warnings:?}"
    );
}

#[test]
fn a_clean_batch_says_nothing_on_either_stream() {
    let (_guard, root) = setup_vault();

    let out = run_batch(&root, CLEAN_OPS, true);

    assert!(out.status.success(), "stderr={}", stderr(&out));
    assert_eq!(stderr(&out), "");

    let payload: Value = serde_json::from_str(stdout(&out)).unwrap();
    assert_eq!(payload["ids"].as_array().unwrap().len(), 2);
    assert!(
        payload.get("warnings").is_none(),
        "a clean batch keeps the payload it has always had: {payload}"
    );
}
