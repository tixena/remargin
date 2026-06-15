use core::str;
use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use os_shim::real::RealSystem;
use remargin_core::config::ResolvedConfig;
use remargin_core::config::identity::IdentityFlags;
use remargin_core::mcp;
use serde_json::{Value, json};
use tempfile::TempDir;

/// A document whose comment's stored checksum matches "hello", so it
/// verifies clean in open mode.
const CLEAN_DOC: &str = "\
---
title: Doc
---

# Doc

```remargin
---
id: good
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

/// Same shape as [`CLEAN_DOC`] but the stored checksum does not match the
/// body, so it fails the checksum check (bad in every mode).
const TAMPERED_DOC: &str = "\
---
title: Doc
---

# Doc

```remargin
---
id: damaged
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:0000000000000000000000000000000000000000000000000000000000000000
---
hello
```
";

/// Build an open-mode workspace with a clean file and a tampered file in
/// a `notes/` subdirectory.
fn build_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join(".remargin.yaml"),
        "identity: alice\ntype: human\nmode: open\n",
    )
    .unwrap();
    let notes = tmp.path().join("notes");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("clean.md"), CLEAN_DOC).unwrap();
    fs::write(notes.join("damaged.md"), TAMPERED_DOC).unwrap();
    tmp
}

/// Run the `remargin` binary in the workspace and capture its output.
fn run_cli(cwd: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", cwd)
        .args(args)
        .output()
        .unwrap()
}

/// Call the in-process MCP `verify` tool with the given arguments and
/// return the parsed JSON payload.
fn run_mcp(base_dir: &Path, arguments: serde_json::Map<String, Value>) -> Value {
    let system = RealSystem::new();
    let config =
        ResolvedConfig::resolve(&system, base_dir, &IdentityFlags::default(), None).unwrap();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": {
            "name": "verify",
            "arguments": Value::Object(arguments),
        }
    });
    let request_str = serde_json::to_string(&request).unwrap();
    let response_str = mcp::process_request(&system, base_dir, &config, &request_str)
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(&response_str).unwrap();
    assert!(
        !response["result"]["isError"].as_bool().unwrap_or(false),
        "MCP verify failed: {response}",
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

/// Strip adapter-only volatile fields (`elapsed_ms`) recursively so the
/// CLI envelope and the MCP payload can be compared structurally.
fn strip_volatile(v: &mut Value) {
    match v {
        Value::Object(map) => {
            let _: Option<Value> = map.remove("elapsed_ms");
            for (_, child) in map.iter_mut() {
                strip_volatile(child);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                strip_volatile(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[test]
fn cli_verify_dir_json_parses_to_folder_report_with_failures() {
    let tmp = build_workspace();
    let out = run_cli(tmp.path(), &["verify", "notes", "--json"]);
    // A directory with a damaged file exits non-zero.
    assert!(!out.status.success(), "expected failure exit: {out:?}");

    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], Value::Bool(false));
    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "both .md files reported: {files:?}");
    let damaged = files
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with("damaged.md"))
        .unwrap();
    assert_eq!(damaged["ok"], Value::Bool(false));
    let clean = files
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with("clean.md"))
        .unwrap();
    assert_eq!(clean["ok"], Value::Bool(true));
}

#[test]
fn cli_verify_dir_text_lists_only_damaged_files() {
    let tmp = build_workspace();
    let out = run_cli(tmp.path(), &["verify", "notes"]);
    assert!(!out.status.success(), "expected failure exit: {out:?}");

    let stdout = str::from_utf8(&out.stdout).unwrap();
    assert!(
        stdout.contains("damaged.md"),
        "damaged file must be listed: {stdout:?}"
    );
    assert!(
        !stdout.contains("clean.md"),
        "clean file must NOT be listed in text mode: {stdout:?}"
    );
}

#[test]
fn cli_verify_single_file_unchanged() {
    let tmp = build_workspace();
    // A single clean file verifies successfully and emits the
    // per-comment summary (no `files` wrapper).
    let out = run_cli(tmp.path(), &["verify", "notes/clean.md"]);
    assert!(out.status.success(), "clean single file must pass: {out:?}");
    let stdout = str::from_utf8(&out.stdout).unwrap();
    assert!(
        stdout.contains("good"),
        "single-file text lists the comment id: {stdout:?}"
    );

    // JSON single-file shape carries `results`, not `files`.
    let out_json = run_cli(tmp.path(), &["verify", "notes/clean.md", "--json"]);
    assert!(out_json.status.success());
    let json: Value = serde_json::from_slice(&out_json.stdout).unwrap();
    assert!(json["results"].is_array(), "single-file JSON has results");
    assert!(
        json.get("files").is_none(),
        "single-file JSON has no files wrapper"
    );
}

#[test]
fn mcp_verify_dir_matches_cli_json() {
    // CLI and MCP run on independently-seeded workspaces so the
    // first-touch frontmatter self-heal does not shift one surface's
    // line numbers relative to the other.
    let cli_tmp = build_workspace();
    let cli = run_cli(cli_tmp.path(), &["verify", "notes", "--json"]);
    let mut cli_json: Value = serde_json::from_slice(&cli.stdout).unwrap();
    strip_volatile(&mut cli_json);

    let mcp_tmp = build_workspace();
    let mut mcp_json = run_mcp(
        mcp_tmp.path(),
        json!({ "path": "notes" }).as_object().unwrap().clone(),
    );
    strip_volatile(&mut mcp_json);

    assert_eq!(cli_json, mcp_json, "CLI --json and MCP must be identical");
}

#[test]
fn cli_verify_dir_honors_gitignore() {
    // A gitignored .md file is skipped by the `ignore`-crate-backed
    // walk_dir, exactly as `replace`/`search` skip it. The `ignore`
    // crate only activates `.gitignore` inside a git repo, so seed a
    // `.git` marker at the workspace root.
    let tmp = build_workspace();
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let notes = tmp.path().join("notes");
    fs::write(notes.join(".gitignore"), "ignored.md\n").unwrap();
    fs::write(notes.join("ignored.md"), TAMPERED_DOC).unwrap();

    let mcp_json = run_mcp(
        tmp.path(),
        json!({ "path": "notes" }).as_object().unwrap().clone(),
    );
    let files = mcp_json["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .all(|f| !f["path"].as_str().unwrap().ends_with("ignored.md")),
        "gitignored file must be skipped: {files:?}"
    );
    assert_eq!(
        files.len(),
        2,
        "only clean.md + damaged.md remain: {files:?}"
    );
}

#[test]
fn mcp_verify_legacy_file_alias_walks_directory() {
    let tmp = build_workspace();
    // The backward-compatible `file` alias must also accept a directory.
    let mcp_json = run_mcp(
        tmp.path(),
        json!({ "file": "notes" }).as_object().unwrap().clone(),
    );
    assert_eq!(mcp_json["ok"], Value::Bool(false));
    let files = mcp_json["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "legacy file=dir walks the directory");
}
