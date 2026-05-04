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

    /// Scenario 12 (rem-egp9): end-to-end restrict + unprotect leaves
    /// the realm in a state that no longer carries the projected rule.
    /// Under the minimised projection the only deny `restrict` emits
    /// is `Bash(remargin *)`.
    #[test]
    fn restrict_then_unprotect_clears_state() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        run_restrict(&realm, "src/secret");
        let project_scope = realm.path().join(".claude/settings.local.json");
        let before = fs::read_to_string(&project_scope).unwrap();
        assert!(before.contains("Bash(remargin *)"));

        let out = run_in(realm.path(), &["unprotect", "src/secret"]);
        assert_status(&out, 0);

        let after = fs::read_to_string(&project_scope).unwrap();
        assert!(
            !after.contains("Bash(remargin *)"),
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
    /// After rem-bimq the YAML compaction prunes the empty restrict
    /// array AND the now-empty permissions block.
    #[test]
    fn wildcard_restrict_and_unprotect_cycle() {
        let realm = realm_with_claude();
        fs::write(realm.path().join("anywhere.md"), "x").unwrap();
        run_restrict(&realm, "*");

        let unprotect = run_in(realm.path(), &["unprotect", "*"]);
        assert_status(&unprotect, 0);

        let body = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
        assert!(
            !body.contains("permissions:") && !body.contains("restrict:"),
            "wildcard unprotect should compact .remargin.yaml: {body}",
        );
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

    /// rem-888p: `unprotect` is intentionally absent from the MCP
    /// surface. `tools/list` must not advertise it, and dispatching it
    /// must return a CLI-pointing tool error. Replaces the previous
    /// MCP-parity test (`mcp_unprotect_matches_cli_json`).
    #[test]
    fn unprotect_absent_from_mcp_surface() {
        let realm = realm_with_claude();

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();

        // tools/list does not advertise `unprotect`.
        let list_request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/list",
            "params": {}
        });
        let list_response_str =
            mcp::process_request(&system, &base, &config, &list_request.to_string())
                .unwrap()
                .unwrap();
        let list_response: Value = serde_json::from_str(&list_response_str).unwrap();
        let tools = list_response["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(
            !names.contains(&"unprotect"),
            "unprotect must not appear in tools/list (rem-888p), got: {names:?}"
        );

        // tools/call with name=unprotect returns a CLI-pointing tool error.
        let call_request = json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "unprotect",
                "arguments": { "path": "src/secret" }
            }
        });
        let call_response_str =
            mcp::process_request(&system, &base, &config, &call_request.to_string())
                .unwrap()
                .unwrap();
        let call_response: Value = serde_json::from_str(&call_response_str).unwrap();
        assert_eq!(
            call_response["result"]["isError"].as_bool(),
            Some(true),
            "unprotect dispatch must surface as a tool error (rem-888p)"
        );
        let text = call_response["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("not available via MCP"),
            "expected refusal pointing to CLI, got: {text}"
        );
        assert!(text.contains("remargin unprotect"), "got: {text}");
    }

    /// rem-s669 (rem-egp9 update): when the projected deny rule has
    /// been hand-deleted from BOTH the realm-local and the user-scope
    /// settings file between `restrict` and `unprotect`, the warning
    /// emitter must surface both — one warning per file — and the
    /// rest of the unprotect work (yaml + sidecar) must complete
    /// cleanly. Under the minimised projection the only deny is
    /// `Bash(remargin *)`.
    #[test]
    fn unprotect_warns_per_settings_file_when_both_have_hand_deleted_rules() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        run_restrict(&realm, "src/secret");

        let realm_local = realm.path().join(".claude/settings.local.json");
        let user_scope = user_settings_arg(&realm);

        // The minimised projection emits exactly one deny —
        // `Bash(remargin *)` — when `cli_allowed = false`. Hand-delete
        // it from both settings files, mirroring what a user would do
        // if they manually scrubbed entries.
        let target_rule = "Bash(remargin *)";
        for file in [&realm_local, &user_scope] {
            let mut value: Value =
                serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
            value["permissions"]["deny"]
                .as_array_mut()
                .unwrap()
                .retain(|v| v.as_str() != Some(target_rule));
            fs::write(
                file,
                serde_json::to_string_pretty(&value).unwrap().as_bytes(),
            )
            .unwrap();
        }

        let out = run_in(realm.path(), &["unprotect", "src/secret"]);
        assert_status(&out, 0);
        let stderr = str::from_utf8(&out.stderr).unwrap();

        // Two warnings expected: one per file, each referencing the
        // missing rule. The phrasing is owned by `revert_rules` —
        // matching on `not present in` keeps the assertion stable
        // against minor wording changes.
        let warning_count = stderr.matches("not present in").count();
        assert_eq!(
            warning_count, 2,
            "expected one not-present warning per settings file, got {warning_count}\nstderr: {stderr}"
        );
        assert!(
            stderr.contains(".claude/settings.local.json"),
            "stderr should name the realm-local file: {stderr}"
        );
        assert!(
            stderr.contains("hermetic-user-settings.json"),
            "stderr should name the user-scope file: {stderr}"
        );

        // Yaml entry was removed.
        let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
        assert!(
            !yaml.contains("src/secret"),
            "restrict entry should have been removed from yaml: {yaml}"
        );

        // Sidecar entry was removed.
        let sidecar_body =
            fs::read_to_string(realm.path().join(".claude/.remargin-restrictions.json")).unwrap();
        let sidecar: Value = serde_json::from_str(&sidecar_body).unwrap();
        assert!(
            sidecar["entries"].as_object().unwrap().is_empty(),
            "sidecar entries should be empty after unprotect: {sidecar}"
        );

        // The remargin-cli deny is gone from both files (it stays
        // hand-deleted; no lingering rule references the realm).
        for file in [&realm_local, &user_scope] {
            let value: Value = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
            let any_remargin_left = value["permissions"]["deny"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some(target_rule));
            assert!(
                !any_remargin_left,
                "{file:?} still contains the remargin-cli deny after unprotect: {value:#?}"
            );
        }
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

    /// rem-bimq: `--strict` against an unrestricted path exits
    /// non-zero with a clear error. The yaml stays untouched.
    #[test]
    fn cli_unprotect_strict_unrestricted_path_fails() {
        let realm = realm_with_claude();
        let yaml_path = realm.path().join(".remargin.yaml");
        let out = run_in(realm.path(), &["unprotect", "src/secret", "--strict"]);
        assert_ne!(out.status.code(), Some(0_i32));
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("not currently restricted") && stderr.contains("--strict"),
            "expected strict refusal with --strict in message, got: {stderr}",
        );
        assert!(!yaml_path.exists(), "no .remargin.yaml should be created");
    }

    // rem-888p: the previous `mcp_unprotect_strict_unrestricted_path_returns_error`
    // (rem-bimq MCP parity) is gone — `unprotect` is no longer exposed
    // via MCP. Strict-mode error coverage now lives in the CLI-only
    // `cli_unprotect_strict_unrestricted_path_fails` above; the surface
    // removal itself is asserted by `unprotect_absent_from_mcp_surface`.
}
