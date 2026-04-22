//! Cross-surface parity harness for CLI + MCP `plan` ops (rem-9ey).
//!
//! For each mutating `plan` op we invoke the CLI binary via `assert_cmd`
//! AND the in-process MCP handler via `mcp::process_request` against a
//! byte-identical fixture, then assert the resulting [`PlanReport`]
//! JSON payloads are structurally equivalent.
//!
//! `plan` is a pure projection (no disk mutation), so we can compare
//! both surfaces without worrying about mutation-order effects. The
//! only fields that legitimately differ across adapter invocations are
//! wall-clock dependent (`ts`, `elapsed_ms`) — [`normalize`] strips
//! them before the `assert_eq!`. Any *other* divergence indicates
//! adapter drift and is the regression this harness is designed to
//! catch (rem-3a2).
//!
//! Covers the deterministic ops `delete`, `edit`, `migrate` (no legacy
//! input), `purge`, and `write` (markdown + raw). `ack`, `react`,
//! `sandbox-add`, `sandbox-remove`, `comment`, and `batch` are excluded
//! because their projections stamp `Utc::now()` into the `after`
//! document; byte-level parity would require freezing the clock, which
//! would need a shim both surfaces wire through — out of scope here
//! and easier to expand once the harness gains value.
// (Previously expected `clippy::print_stderr`; removed along with eprintln usage.)

#[cfg(test)]
mod tests {

    use std::fs;
    use std::path::Path;

    use assert_cmd::Command;
    use os_shim::real::RealSystem;
    use remargin_core::config::ResolvedConfig;
    use remargin_core::config::identity::IdentityFlags;
    use remargin_core::mcp;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    /// A fixture document containing one real comment, seeded via the
    /// low-level writer so both CLI and MCP see byte-identical bytes. The
    /// comment's `ts` is pinned so subsequent plan reports do not capture
    /// wall-clock skew.
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
        // Also drop a `.remargin.yaml` with open mode so the CLI's config
        // walk does not find anything unexpected on the host.
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
    /// Post-rem-zlx3 `--json` is per-subcommand, not a top-level flag, so
    /// append it at the end where every subcommand accepts trailing
    /// options.
    #[expect(clippy::panic, reason = "integration test assertion helper")]
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

        serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "CLI stdout was not valid JSON for args {:?}: {}; raw={}",
                args,
                err,
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    /// Call the in-process MCP handler with a `plan` tools-call request.
    ///
    /// `arguments` must be a JSON object; this adds the `op` field and
    /// wraps it inside the JSON-RPC `tools/call` envelope.
    #[expect(clippy::panic, reason = "integration test assertion helper")]
    fn run_mcp(base_dir: &Path, op: &str, arguments: Value) -> Value {
        let system = RealSystem::new();
        let config = parity_config(system, base_dir);

        let Value::Object(mut args_map) = arguments else {
            panic!("run_mcp expects an object for `arguments`, got non-object")
        };
        args_map.insert(String::from("op"), Value::String(String::from(op)));

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": Value::Object(args_map),
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
                // `identity.would_sign` can flip based on adapter-resolved
                // key_path, which is identical here but belt-and-braces.
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
    fn assert_parity(cli_args: &[&str], mcp_op: &str, mcp_args: Value, tmp: &TempDir) -> Value {
        assert_parity_with_stdin(cli_args, "", mcp_op, mcp_args, tmp)
    }

    /// Variant of [`assert_parity`] that pipes `stdin` into the CLI. Use
    /// when the CLI `content` positional would clash with clap flag
    /// parsing (e.g. a body starting with `---`).
    fn assert_parity_with_stdin(
        cli_args: &[&str],
        cli_stdin: &str,
        mcp_op: &str,
        mcp_args: Value,
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

    #[test]
    fn plan_delete_parity() {
        let tmp = TempDir::new().unwrap();
        seed(&tmp, "doc.md");
        let report = assert_parity(
            &["plan", "delete", "doc.md", "aaa"],
            "delete",
            json!({ "file": "doc.md", "ids": ["aaa"] }),
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
            json!({ "file": "doc.md", "id": "aaa", "content": "Edited content." }),
            &tmp,
        );
        assert_eq!(report["op"], "edit");
        assert_eq!(report["comments"]["modified"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn plan_migrate_parity_on_clean_doc() {
        let tmp = TempDir::new().unwrap();
        seed(&tmp, "doc.md");
        let report = assert_parity(
            &["plan", "migrate", "doc.md"],
            "migrate",
            json!({ "file": "doc.md" }),
            &tmp,
        );
        assert_eq!(report["op"], "migrate");
        // No legacy comments in the fixture; the projection is a pure
        // frontmatter touch-up. Both adapters agree on whether that's a
        // noop, whatever the verdict turns out to be — parity is what
        // we're asserting here, not the specific verdict.
    }

    #[test]
    fn plan_purge_parity() {
        let tmp = TempDir::new().unwrap();
        seed(&tmp, "doc.md");
        let report = assert_parity(
            &["plan", "purge", "doc.md"],
            "purge",
            json!({ "file": "doc.md" }),
            &tmp,
        );
        assert_eq!(report["op"], "purge");
        assert_eq!(report["comments"]["destroyed"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn plan_write_markdown_create_parity() {
        let tmp = TempDir::new().unwrap();
        seed(&tmp, "doc.md");
        // `plan write --create` on a fresh markdown path so the
        // preservation check does not trip. Pre-populate `created:` in the
        // frontmatter so `ensure_frontmatter` does not stamp a wall-clock
        // timestamp (which would diverge between CLI and MCP invocations).
        // Content starts with `---` so pipe it via stdin to avoid clap's
        // flag parser.
        let new_body = "---\ntitle: Fresh doc\ndescription: ''\nauthor: parity-bot\ncreated: 2026-04-06T12:00:00+00:00\n---\n\n# Fresh doc\n\nBody paragraph.\n";
        let report = assert_parity_with_stdin(
            &["plan", "write", "fresh.md", "--create"],
            new_body,
            "write",
            json!({ "file": "fresh.md", "content": new_body, "create": true }),
            &tmp,
        );
        assert_eq!(report["op"], "write");
    }

    #[test]
    fn plan_write_raw_returns_reject_reason_parity() {
        let tmp = TempDir::new().unwrap();
        seed(&tmp, "doc.md");
        // Pre-create the raw-target file so both adapters hit the
        // `raw -> Unsupported` branch together (rather than one erroring
        // on missing file before reaching raw handling).
        fs::write(tmp.path().join("out.txt"), "preexisting\n").unwrap();
        let report = assert_parity(
            &["plan", "write", "out.txt", "hello", "--raw"],
            "write",
            json!({ "file": "out.txt", "content": "hello", "raw": true }),
            &tmp,
        );
        assert_eq!(report["op"], "write");
        // Both adapters should report the same `reject_reason` for raw
        // writes (no structured plan representation).
        assert!(report["reject_reason"].is_string());
        assert_eq!(report["would_commit"], false);
    }
} // mod tests
