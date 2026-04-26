//! `remargin permissions show / check` CLI + MCP integration tests
//! (rem-yj1j.7 / T28 / rem-8y1h).
//!
//! Covers spec scenarios 20-22 of T28's testing plan:
//!
//! - Scenario 20: `permissions show` and `permissions check`
//!   surface the parent-walked `.remargin.yaml` permissions correctly
//!   (text + JSON, restricted exit 0).
//! - Scenario 21: when no rules cover a path, `check` exits 1 and
//!   `show` lists the empty surface.
//! - Scenario 22: MCP `permissions_show` and `permissions_check`
//!   parity with CLI `--json` output.
//!
//! T26 (`restrict`) and T27 (`unprotect`) are not yet wired, so these
//! tests stage a `.remargin.yaml` directly. When the CLI restrict/
//! unprotect commands land, they replace the hand-written fixture with
//! a CLI invocation but the assertions stay the same.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::Path;
    use std::process::Output;

    use assert_cmd::Command;
    use os_shim::System as _;
    use os_shim::real::RealSystem;
    use remargin_core::config::ResolvedConfig;
    use remargin_core::config::identity::IdentityFlags;
    use remargin_core::mcp;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn stdout_of(out: &Output) -> &str {
        str::from_utf8(&out.stdout).unwrap()
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

    fn write_realm_yaml(realm: &Path, body: &str) {
        fs::write(realm.join(".remargin.yaml"), body).unwrap();
    }

    /// Scenario 20a: `permissions show --json` over a hand-rolled
    /// realm reports the declared `restrict` and `deny_ops` entries.
    #[test]
    fn show_json_lists_declared_entries() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n  deny_ops:\n    - path: archive\n      ops: [purge]\n",
        );
        // Materialise the targets so `restrict_covers` matches paths
        // canonicalised through the real filesystem.
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::create_dir_all(realm.path().join("archive")).unwrap();

        let out = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&out, 0);
        let body: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        let restrict = body.get("restrict").and_then(Value::as_array).unwrap();
        assert_eq!(restrict.len(), 1);
        let deny_ops = body.get("deny_ops").and_then(Value::as_array).unwrap();
        assert_eq!(deny_ops.len(), 1);
        let ops = deny_ops[0].get("ops").and_then(Value::as_array).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].as_str().unwrap(), "purge");
    }

    /// Scenario 20b: `permissions check` exits 0 when the path sits
    /// under a `restrict` entry (gitignore-style: matched = success).
    #[test]
    fn check_exits_zero_for_restricted_path() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::write(realm.path().join("src/secret/foo.md"), "x").unwrap();

        let out = run_in(realm.path(), &["permissions", "check", "src/secret/foo.md"]);
        assert_status(&out, 0);
    }

    /// Scenario 21a: with no `.remargin.yaml`, `permissions show`
    /// returns empty collections under `--json`.
    #[test]
    fn show_json_empty_when_no_config() {
        let realm = TempDir::new().unwrap();
        let out = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&out, 0);
        let body: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        assert!(
            body.get("restrict")
                .and_then(Value::as_array)
                .unwrap()
                .is_empty()
        );
        assert!(
            body.get("deny_ops")
                .and_then(Value::as_array)
                .unwrap()
                .is_empty()
        );
        assert!(
            body.get("trusted_roots")
                .and_then(Value::as_array)
                .unwrap()
                .is_empty()
        );
    }

    /// Scenario 21b: with no rules covering a path, `permissions check`
    /// exits 1 (the gitignore-style "not matched" code).
    #[test]
    fn check_exits_one_when_unrestricted() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/public")).unwrap();
        fs::write(realm.path().join("src/public/foo.md"), "x").unwrap();

        let out = run_in(realm.path(), &["permissions", "check", "src/public/foo.md"]);
        assert_status(&out, 1);
    }

    /// `--why` populates the matching-rule section in JSON output for a
    /// restricted hit (smoke test for the optional detail field).
    #[test]
    fn check_why_populates_matching_rule() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::write(realm.path().join("src/secret/foo.md"), "x").unwrap();

        let out = run_in(
            realm.path(),
            &[
                "permissions",
                "check",
                "src/secret/foo.md",
                "--why",
                "--json",
            ],
        );
        assert_status(&out, 0);
        let body: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        assert_eq!(body.get("restricted").unwrap(), &Value::Bool(true));
        let rule = body.get("matching_rule").unwrap();
        assert_eq!(rule.get("kind").unwrap().as_str().unwrap(), "restrict");
    }

    // --- Scenario 22: MCP parity ------------------------------------------

    fn mcp_call(base: &Path, config: &ResolvedConfig, tool_name: &str, arguments: &Value) -> Value {
        let system = RealSystem::new();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": { "name": tool_name, "arguments": arguments }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        let response = mcp::process_request(&system, base, config, &request_str)
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();
        parsed.get("result").unwrap().clone()
    }

    fn mcp_payload(result: &Value) -> Value {
        let content = result.get("content").and_then(Value::as_array).unwrap();
        let text = content[0].get("text").and_then(Value::as_str).unwrap();
        let mut parsed: Value = serde_json::from_str(text).unwrap();
        strip_elapsed_ms(&mut parsed);
        parsed
    }

    /// Drop the `elapsed_ms` field injected by both surfaces so the
    /// comparison focuses on the structured payload. CLI / MCP run in
    /// different processes and rarely report the same elapsed time, but
    /// every other field is identical by construction.
    fn strip_elapsed_ms(value: &mut Value) {
        if let Value::Object(map) = value {
            map.remove("elapsed_ms");
        }
    }

    fn parse_cli_json(out: &Output) -> Value {
        let mut value: Value = serde_json::from_str(stdout_of(out)).unwrap();
        strip_elapsed_ms(&mut value);
        value
    }

    /// `permissions_show` MCP tool returns the same JSON shape the CLI
    /// emits under `--json`.
    #[test]
    fn mcp_permissions_show_matches_cli_json() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n  deny_ops:\n    - path: archive\n      ops: [purge]\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::create_dir_all(realm.path().join("archive")).unwrap();

        // CLI side.
        let cli = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&cli, 0);
        let cli_body = parse_cli_json(&cli);

        // MCP side.
        let system = RealSystem::new();
        // Canonicalise the temp path so the parent-walk inside the MCP
        // resolver matches the CLI's canonicalised cwd.
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();
        let result = mcp_call(&base, &config, "permissions_show", &json!({}));
        let mcp_body = mcp_payload(&result);

        assert_eq!(cli_body, mcp_body, "CLI and MCP show output diverged");
    }

    /// `permissions_check` MCP tool agrees with CLI `--json` for both
    /// restricted (= true) and unrestricted (= false) targets.
    #[test]
    fn mcp_permissions_check_matches_cli_json() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::write(realm.path().join("src/secret/foo.md"), "x").unwrap();
        fs::create_dir_all(realm.path().join("src/public")).unwrap();
        fs::write(realm.path().join("src/public/foo.md"), "x").unwrap();

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();

        // Restricted target.
        let cli_hit = run_in(
            realm.path(),
            &[
                "permissions",
                "check",
                "src/secret/foo.md",
                "--json",
                "--why",
            ],
        );
        assert_status(&cli_hit, 0);
        let cli_hit_body = parse_cli_json(&cli_hit);
        let mcp_hit = mcp_call(
            &base,
            &config,
            "permissions_check",
            &json!({ "path": "src/secret/foo.md", "why": true }),
        );
        let mcp_hit_body = mcp_payload(&mcp_hit);
        assert_eq!(cli_hit_body, mcp_hit_body);

        // Unrestricted target.
        let cli_miss = run_in(
            realm.path(),
            &["permissions", "check", "src/public/foo.md", "--json"],
        );
        assert_status(&cli_miss, 1);
        let cli_miss_body = parse_cli_json(&cli_miss);
        let mcp_miss = mcp_call(
            &base,
            &config,
            "permissions_check",
            &json!({ "path": "src/public/foo.md" }),
        );
        let mcp_miss_body = mcp_payload(&mcp_miss);
        assert_eq!(cli_miss_body, mcp_miss_body);
    }

    /// `permissions show` text output names the realm and the
    /// restricted entry. Smoke test for the human-readable formatter.
    #[test]
    fn show_text_output_includes_restrict_entry() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();

        let out = run_in(realm.path(), &["permissions", "show"]);
        assert_status(&out, 0);
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("restrict:"),
            "expected 'restrict:' header in:\n{stderr}",
        );
        assert!(
            stderr.contains("src/secret"),
            "expected restricted path in:\n{stderr}",
        );
    }

    /// `PathBuf` coverage: also ensure relative paths beginning with `./`
    /// canonicalise correctly through the CLI surface.
    #[test]
    fn check_dot_slash_path_canonicalises() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  restrict:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::write(realm.path().join("src/secret/foo.md"), "x").unwrap();

        let out = run_in(
            realm.path(),
            &["permissions", "check", "./src/secret/foo.md"],
        );
        assert_status(&out, 0);
    }
}
