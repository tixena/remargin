//! `remargin unprotect` integration tests (rem-yj1j.6 / rem-hsg4).
//!
//! Exercises the CLI subcommand and the matching MCP tool against
//! real-filesystem temp dirs. Covers scenarios 12-16 of the
//! rem-yj1j.6 plan: end-to-end restrict + unprotect round-trip,
//! Layer 1 enforcement transitions, wildcard cycle, --json output,
//! MCP parity.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Output;

    use assert_cmd::Command;
    use os_shim::System as _;
    use os_shim::real::RealSystem;
    use remargin_core::config::ResolvedConfig;
    use remargin_core::config::identity::IdentityFlags;
    use remargin_core::mcp;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    fn realm_with_claude() -> TempDir {
        let realm = TempDir::new().unwrap();
        fs::create_dir_all(realm.path().join(".claude")).unwrap();
        realm
    }

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
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

    fn user_settings_arg(realm: &TempDir) -> PathBuf {
        realm.path().join("hermetic-user-settings.json")
    }

    fn run_restrict(realm: &TempDir, path: &str) {
        let user_settings = user_settings_arg(realm);
        let out = run_in(
            realm.path(),
            &[
                "restrict",
                path,
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&out, 0);
    }

    /// Scenario 12: end-to-end restrict + unprotect leaves the
    /// realm in a state that no longer carries the rule.
    #[test]
    fn restrict_then_unprotect_clears_state() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        run_restrict(&realm, "src/secret");
        let project_scope = realm.path().join(".claude/settings.local.json");
        let before = fs::read_to_string(&project_scope).unwrap();
        assert!(before.contains("Edit("));

        let out = run_in(realm.path(), &["unprotect", "src/secret"]);
        assert_status(&out, 0);

        let after = fs::read_to_string(&project_scope).unwrap();
        assert!(
            !after.contains("Edit(") || !after.contains("src/secret"),
            "settings still references the removed rule:\n{after}"
        );

        let sidecar_body =
            fs::read_to_string(realm.path().join(".claude/.remargin-restrictions.json")).unwrap();
        let sidecar: Value = serde_json::from_str(&sidecar_body).unwrap();
        assert!(sidecar["entries"].as_object().unwrap().is_empty());
    }

    /// Scenario 13: Layer 1 stops enforcing after unprotect. The
    /// post-unprotect write succeeds because the per-op guard
    /// re-resolves on each call.
    #[test]
    fn layer_1_stops_enforcing_after_unprotect() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        // Use a markdown doc so the comment-preserving write path
        // applies; raw mode is not allowed for .md files.
        fs::write(
            realm.path().join("src/secret/foo.md"),
            "---\ntitle: test\n---\n\n# Hi\n",
        )
        .unwrap();
        run_restrict(&realm, "src/secret");

        let blocked = run_in(
            realm.path(),
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--",
                "src/secret/foo.md",
                "---\ntitle: test\n---\n\n# Updated\n",
            ],
        );
        assert_ne!(blocked.status.code(), Some(0_i32));
        let blocked_stderr = String::from_utf8_lossy(&blocked.stderr);
        assert!(
            blocked_stderr.contains("denied by `restrict`"),
            "expected restrict refusal, got: {blocked_stderr}"
        );

        let unprotect = run_in(realm.path(), &["unprotect", "src/secret"]);
        assert_status(&unprotect, 0);

        let allowed = run_in(
            realm.path(),
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--",
                "src/secret/foo.md",
                "---\ntitle: test\n---\n\n# Updated\n",
            ],
        );
        assert_status(&allowed, 0);
        let body = fs::read_to_string(realm.path().join("src/secret/foo.md")).unwrap();
        assert!(body.contains("# Updated"));
    }

    /// Scenario 14: wildcard restrict + wildcard unprotect cycle.
    #[test]
    fn wildcard_restrict_and_unprotect_cycle() {
        let realm = realm_with_claude();
        fs::write(realm.path().join("anywhere.md"), "x").unwrap();
        run_restrict(&realm, "*");

        let unprotect = run_in(realm.path(), &["unprotect", "*"]);
        assert_status(&unprotect, 0);

        let body = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&body).unwrap();
        let restricts = value["permissions"]["restrict"].as_sequence().unwrap();
        assert!(restricts.is_empty());
    }

    /// Scenario 15: --json output parses to the documented
    /// `UnprotectOutcome` shape.
    #[test]
    fn unprotect_json_output_round_trips() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        run_restrict(&realm, "src/secret");

        let out = run_in(realm.path(), &["unprotect", "src/secret", "--json"]);
        assert_status(&out, 0);
        let stdout = str::from_utf8(&out.stdout).unwrap();
        let value: Value = serde_json::from_str(stdout).unwrap();
        assert!(value.get("absolute_path").is_some());
        assert!(value.get("anchor").is_some());
        assert_eq!(value["yaml_entry_removed"], json!(true));
        assert!(value.get("warnings").and_then(Value::as_array).is_some());
    }

    /// Scenario 16: MCP parity — `mcp__remargin__unprotect` returns
    /// the same shape as the CLI `--json` output.
    #[test]
    fn mcp_unprotect_matches_cli_json() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        run_restrict(&realm, "src/secret");

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "unprotect",
                "arguments": { "path": "src/secret" }
            }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        let response_str = mcp::process_request(&system, &base, &config, &request_str)
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&response_str).unwrap();
        let result = response.get("result").unwrap();
        let content = result.get("content").and_then(Value::as_array).unwrap();
        let text = content[0].get("text").and_then(Value::as_str).unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert!(payload.get("absolute_path").is_some());
        assert_eq!(payload["yaml_entry_removed"], json!(true));
    }

    /// Idempotency on the CLI surface: a second `unprotect` is a
    /// warn + no-op (exit 0).
    #[test]
    fn cli_unprotect_is_idempotent() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        run_restrict(&realm, "src/secret");
        run_in(realm.path(), &["unprotect", "src/secret"]);
        let second = run_in(realm.path(), &["unprotect", "src/secret"]);
        assert_status(&second, 0);
        let stderr = str::from_utf8(&second.stderr).unwrap();
        assert!(
            stderr.contains("not currently restricted"),
            "expected idempotent warn, got: {stderr}"
        );
    }
}
