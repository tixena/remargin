//! Runs each deterministic `plan` op through the CLI and the MCP handler and compares the reports.

use std::fs;
use std::path::Path;

use anyhow::Context as _;
use assert_cmd::Command;
use os_shim::real::RealSystem;
use remargin_core::config::ResolvedConfig;
use remargin_core::config::identity::IdentityFlags;
use remargin_core::mcp;
use serde_json::{Value, json};
use tempfile::TempDir;

/// The comment's `ts` is pinned so plan reports capture no wall-clock skew.
const FIXTURE_DOC: &str = "---
title: Parity fixture
description: ''
author: parity-bot
created: 2026-04-06T12:00:00+00:00
---

# Heading

Body paragraph one.

```remargin
---
id: aaa
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:3e5121224e71bb75be3d2a2ac568d2117b6cd3aa10a54f7abc9b19cdb1976b2e
---
Seed comment for parity harness.
```

Body paragraph two.
";

/// Write the fixture into both CLI and MCP sides of the tempdir. Both
/// `plan` invocations read the same bytes so any difference in the
/// projected report is pure adapter drift.
fn seed(tmp: &TempDir, filename: &str) {
    fs::write(tmp.path().join(filename), FIXTURE_DOC).unwrap();
    // An open-mode `.remargin.yaml` here stops the CLI's config walk before it reaches the host's.
    fs::write(
        tmp.path().join(".remargin.yaml"),
        "mode: open\nidentity: parity-bot\ntype: agent\n",
    )
    .unwrap();
}

/// Build a `ResolvedConfig` for the MCP side by loading the same
/// `.remargin.yaml` the CLI walk discovers. This guarantees both
/// surfaces operate on byte-identical config — any difference in the
/// resulting plan report is adapter drift, not config drift.
fn parity_config(system: RealSystem, cwd: &Path) -> ResolvedConfig {
    ResolvedConfig::resolve(&system, cwd, &IdentityFlags::default(), None).unwrap()
}

/// Run the CLI binary in `--json` mode with the given subcommand
/// arguments. Returns the parsed JSON stdout. If `stdin` is non-empty
/// it is piped in as the command's stdin (used when the content would
/// look like a flag to clap).
///
/// `--json` is per-subcommand, not a top-level flag, so
/// append it at the end where every subcommand accepts trailing
/// options.
fn run_cli(cwd: &Path, args: &[&str], stdin: &str) -> Value {
    let mut cmd = Command::cargo_bin("remargin").unwrap();
    cmd.current_dir(cwd).args(args).arg("--json");
    if !stdin.is_empty() {
        cmd.write_stdin(stdin);
    }
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "CLI invocation failed for args {:?}: status={}, stdout={}, stderr={}",
        args,
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    serde_json::from_slice(&output.stdout)
        .with_context(|| {
            format!(
                "CLI stdout was not valid JSON for args {:?}; raw={}",
                args,
                String::from_utf8_lossy(&output.stdout)
            )
        })
        .unwrap()
}

/// Call the in-process MCP handler with a `plan` tools-call request.
///
/// `arguments` is a typed map (not a generic `Value`) so the
/// "must be an object" contract is enforced at compile time, not
/// at runtime. This adds the `op` field and wraps it inside the
/// JSON-RPC `tools/call` envelope.
fn run_mcp(base_dir: &Path, op: &str, mut arguments: serde_json::Map<String, Value>) -> Value {
    let system = RealSystem::new();
    let config = parity_config(system, base_dir);

    arguments.insert(String::from("op"), Value::String(String::from(op)));

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": {
            "name": "plan",
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
        "MCP request failed for op {op}: {response}",
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

/// Strip fields that legitimately differ across CLI and MCP
/// invocations: timestamps / elapsed counters and adapter-only
/// metadata. Applied recursively.
fn normalize(mut v: Value) -> Value {
    strip_volatile(&mut v);
    v
}

fn strip_volatile(v: &mut Value) {
    match v {
        Value::Object(map) => {
            let _: Option<Value> = map.remove("elapsed_ms");
            let _: Option<Value> = map.remove("ts");
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

/// Invoke CLI and MCP for the same plan op, normalize, and assert
/// structural equality. Returns the normalized report for op-specific
/// follow-up assertions.
fn assert_parity(
    cli_args: &[&str],
    mcp_op: &str,
    mcp_args: serde_json::Map<String, Value>,
    tmp: &TempDir,
) -> Value {
    assert_parity_with_stdin(cli_args, "", mcp_op, mcp_args, tmp)
}

/// Variant of [`assert_parity`] that pipes `stdin` into the CLI. Use
/// when the CLI `content` positional would clash with clap flag
/// parsing (e.g. a body starting with `---`).
fn assert_parity_with_stdin(
    cli_args: &[&str],
    cli_stdin: &str,
    mcp_op: &str,
    mcp_args: serde_json::Map<String, Value>,
    tmp: &TempDir,
) -> Value {
    let cli_value = normalize(run_cli(tmp.path(), cli_args, cli_stdin));
    let mcp_value = normalize(run_mcp(tmp.path(), mcp_op, mcp_args));
    assert_eq!(
        cli_value, mcp_value,
        "adapter drift for op {mcp_op:?}: CLI != MCP"
    );
    cli_value
}

/// Build the `mcp_args` Map for an `assert_parity*` call. Wraps a
/// `json!({...})` literal so test code stays compact while the
/// helper signature stays typed.
fn obj(v: &Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn plan_delete_parity() {
    let tmp = TempDir::new().unwrap();
    seed(&tmp, "doc.md");
    let report = assert_parity(
        &["plan", "delete", "doc.md", "aaa"],
        "delete",
        obj(&json!({ "file": "doc.md", "ids": ["aaa"] })),
        &tmp,
    );
    assert_eq!(report["op"], "delete");
    assert_eq!(report["comments"]["destroyed"].as_array().unwrap().len(), 1);
}

#[test]
fn plan_edit_parity() {
    let tmp = TempDir::new().unwrap();
    seed(&tmp, "doc.md");
    let report = assert_parity(
        &["plan", "edit", "doc.md", "aaa", "Edited content."],
        "edit",
        obj(&json!({ "file": "doc.md", "id": "aaa", "content": "Edited content." })),
        &tmp,
    );
    assert_eq!(report["op"], "edit");
    assert_eq!(report["comments"]["modified"].as_array().unwrap().len(), 1);
}

#[test]
fn plan_purge_parity() {
    let tmp = TempDir::new().unwrap();
    seed(&tmp, "doc.md");
    let report = assert_parity(
        &["plan", "purge", "doc.md"],
        "purge",
        obj(&json!({ "file": "doc.md" })),
        &tmp,
    );
    assert_eq!(report["op"], "purge");
    assert_eq!(report["comments"]["destroyed"].as_array().unwrap().len(), 1);
}

#[test]
fn plan_write_markdown_create_parity() {
    let tmp = TempDir::new().unwrap();
    seed(&tmp, "doc.md");
    // `created:` is pre-populated so `ensure_frontmatter` stamps no wall-clock timestamp, and the
    // body goes through stdin because it starts with `---`.
    let new_body = "---\ntitle: Fresh doc\ndescription: ''\nauthor: parity-bot\ncreated: 2026-04-06T12:00:00+00:00\n---\n\n# Fresh doc\n\nBody paragraph.\n";
    let report = assert_parity_with_stdin(
        &["plan", "write", "fresh.md", "--create"],
        new_body,
        "write",
        obj(&json!({ "file": "fresh.md", "content": new_body, "create": true })),
        &tmp,
    );
    assert_eq!(report["op"], "write");
}

#[test]
fn plan_write_raw_returns_reject_reason_parity() {
    let tmp = TempDir::new().unwrap();
    seed(&tmp, "doc.md");
    // The raw target exists up front so both adapters reach the raw-unsupported branch together.
    fs::write(tmp.path().join("out.txt"), "preexisting\n").unwrap();
    let report = assert_parity(
        &["plan", "write", "out.txt", "hello", "--raw"],
        "write",
        obj(&json!({ "file": "out.txt", "content": "hello", "raw": true })),
        &tmp,
    );
    assert_eq!(report["op"], "write");
    assert!(report["reject_reason"].is_string());
    assert_eq!(report["would_commit"], false);
}
