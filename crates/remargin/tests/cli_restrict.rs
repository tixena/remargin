//! `remargin restrict` integration tests (rem-yj1j.5 / rem-rdjy).
//!
//! Exercises the CLI subcommand and the matching MCP tool against
//! real-filesystem temp dirs. Covers scenarios 14-20 of the
//! rem-yj1j.5 testing plan: end-to-end restrict + Layer 1
//! enforcement, settings-file and sidecar updates, gitignore
//! automation, wildcard, --json output, MCP parity.

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

    /// Scenario 14: end-to-end restrict + Layer 1 enforcement. After
    /// `remargin restrict src/secret`, the next `remargin write` on
    /// a path under `src/secret` is refused by the in-process op
    /// guard. The settings sync also runs but Layer 2 takes effect
    /// only on Claude reload (out of scope for this test).
    #[test]
    fn restrict_then_write_is_refused_by_layer_1() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::write(realm.path().join("src/secret/foo.md"), "x").unwrap();
        let user_settings = user_settings_arg(&realm);

        let restrict = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&restrict, 0);

        let write = run_in(
            realm.path(),
            &[
                "write",
                "src/secret/foo.md",
                "blocked content",
                "--raw",
                "--identity",
                "alice",
                "--type",
                "human",
            ],
        );
        assert_ne!(write.status.code(), Some(0_i32), "write should be refused");
        let stderr = String::from_utf8_lossy(&write.stderr);
        assert!(
            stderr.contains("denied by `restrict`"),
            "expected restrict refusal, got: {stderr}"
        );
    }

    /// Scenarios 15-17: end-to-end restrict writes the projected
    /// rule set into both settings files, records the sidecar entry,
    /// and adds the gitignore line.
    #[test]
    fn restrict_writes_settings_sidecar_and_gitignore() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);

        let out = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&out, 0);

        // Project-scope settings file landed with the rules.
        let project_scope = realm.path().join(".claude/settings.local.json");
        let body = fs::read_to_string(&project_scope).unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        let deny = value["permissions"]["deny"].as_array().unwrap();
        assert!(deny.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| s.starts_with("Edit(") && s.contains("src/secret"))
        }));
        assert!(deny.iter().any(|v| v.as_str() == Some("Bash(remargin *)")));

        // User-scope file landed too.
        let user_body = fs::read_to_string(&user_settings).unwrap();
        let user_value: Value = serde_json::from_str(&user_body).unwrap();
        assert!(user_value["permissions"]["deny"].is_array());

        // Sidecar exists with one entry.
        let sidecar_body =
            fs::read_to_string(realm.path().join(".claude/.remargin-restrictions.json")).unwrap();
        let sidecar: Value = serde_json::from_str(&sidecar_body).unwrap();
        let entries = sidecar["entries"].as_object().unwrap();
        assert_eq!(entries.len(), 1);

        // Gitignore created with the entry.
        let gitignore = fs::read_to_string(realm.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".claude/.remargin-restrictions.json"));
    }

    /// Scenario 18: wildcard restrict refuses every mutating op
    /// against any path under the realm.
    #[test]
    fn wildcard_restrict_blocks_realm_wide_writes() {
        let realm = realm_with_claude();
        fs::write(realm.path().join("anywhere.md"), "x").unwrap();
        let user_settings = user_settings_arg(&realm);

        let restrict = run_in(
            realm.path(),
            &[
                "restrict",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&restrict, 0);

        let write = run_in(
            realm.path(),
            &[
                "write",
                "anywhere.md",
                "blocked",
                "--raw",
                "--identity",
                "alice",
                "--type",
                "human",
            ],
        );
        assert_ne!(write.status.code(), Some(0_i32));
        let stderr = String::from_utf8_lossy(&write.stderr);
        assert!(
            stderr.contains("denied by `restrict`"),
            "expected restrict refusal, got: {stderr}"
        );
    }

    /// Scenario 19: --json output parses to the documented
    /// `RestrictOutcome` shape.
    #[test]
    fn restrict_json_output_round_trips() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);

        let out = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let stdout = str::from_utf8(&out.stdout).unwrap();
        let value: Value = serde_json::from_str(stdout).unwrap();
        assert!(value.get("absolute_path").is_some());
        assert!(value.get("anchor").is_some());
        assert!(
            value
                .get("claude_files_touched")
                .and_then(Value::as_array)
                .is_some_and(|a| a.len() == 2)
        );
        assert!(
            value
                .get("rules_applied")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        );
        assert_eq!(value["yaml_was_created"], json!(true));
    }

    /// rem-888p: `restrict` is intentionally absent from the MCP
    /// surface. `tools/list` must not advertise it, and dispatching it
    /// must return a CLI-pointing tool error. Replaces the previous
    /// MCP-parity test (`mcp_restrict_matches_cli_json`).
    #[test]
    fn restrict_absent_from_mcp_surface() {
        let realm = realm_with_claude();

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();

        // tools/list does not advertise `restrict`.
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
            !names.contains(&"restrict"),
            "restrict must not appear in tools/list (rem-888p), got: {names:?}"
        );

        // tools/call with name=restrict returns a CLI-pointing tool error.
        let call_request = json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "restrict",
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
            "restrict dispatch must surface as a tool error (rem-888p)"
        );
        let text = call_response["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("not available via MCP"),
            "expected refusal pointing to CLI, got: {text}"
        );
        assert!(text.contains("remargin restrict"), "got: {text}");
    }

    /// rem-ss9s: helper that runs `restrict src/secret` with the
    /// given `--also-deny-bash` argv and returns the resulting
    /// `permissions.restrict[0].also_deny_bash` list parsed from
    /// `.remargin.yaml`.
    fn also_deny_bash_for(extra_args: &[&str]) -> Vec<String> {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);
        let mut args: Vec<&str> = vec![
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ];
        args.extend_from_slice(extra_args);
        let out = run_in(realm.path(), &args);
        assert_status(&out, 0);

        let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        value["permissions"]["restrict"][0]["also_deny_bash"]
            .as_sequence()
            .map(|s| s.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
            .unwrap_or_default()
    }

    /// rem-ss9s scenario 1: repeated `--also-deny-bash` flags emit
    /// each token (regression check).
    #[test]
    fn also_deny_bash_repeated_flags() {
        let tokens = also_deny_bash_for(&["--also-deny-bash", "curl", "--also-deny-bash", "wget"]);
        assert_eq!(tokens, vec!["curl".to_owned(), "wget".to_owned()]);
    }

    /// rem-ss9s scenario 2: comma-separated values are split
    /// equivalently to repeated flags.
    #[test]
    fn also_deny_bash_comma_separated() {
        let tokens = also_deny_bash_for(&["--also-deny-bash", "curl,wget"]);
        assert_eq!(tokens, vec!["curl".to_owned(), "wget".to_owned()]);
    }

    /// rem-ss9s scenario 3: mixing comma-separated values and
    /// repeated flags concatenates in argv order.
    #[test]
    fn also_deny_bash_mixed_csv_and_repeated() {
        let tokens =
            also_deny_bash_for(&["--also-deny-bash", "curl,wget", "--also-deny-bash", "sed"]);
        assert_eq!(
            tokens,
            vec!["curl".to_owned(), "wget".to_owned(), "sed".to_owned()],
        );
    }

    /// rem-ss9s scenario 4: when the flag is absent the yaml
    /// has no `also_deny_bash` key (or an empty list, depending on
    /// serializer; check both forms).
    #[test]
    fn also_deny_bash_absent_omits_or_empties_field() {
        let tokens = also_deny_bash_for(&[]);
        assert!(
            tokens.is_empty(),
            "expected no extra deny tokens, got: {tokens:?}"
        );
    }

    /// rem-ss9s scenario 5: a single token still parses cleanly
    /// (no delimiter triggers).
    #[test]
    fn also_deny_bash_single_value() {
        let tokens = also_deny_bash_for(&["--also-deny-bash", "curl"]);
        assert_eq!(tokens, vec!["curl".to_owned()]);
    }

    /// rem-e6yd / T42: `cd` and `pushd` denies are emitted by default
    /// so the `cd /restricted && rm file` bypass class is closed. Both
    /// the bare form (`cd /path`) and the with-flag form (`cd -P /path`)
    /// are covered by emitting both `cd <path>/**` and `cd * <path>/**`.
    #[test]
    fn cd_pushd_denies_emitted_by_default() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);

        let out = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&out, 0);

        let canonical = fs::canonicalize(realm.path().join("src/secret")).unwrap();
        let glob = format!("{}/**", canonical.display());
        let expected: [String; 4] = [
            format!("Bash(cd {glob})"),
            format!("Bash(cd * {glob})"),
            format!("Bash(pushd {glob})"),
            format!("Bash(pushd * {glob})"),
        ];

        let project_scope = realm.path().join(".claude/settings.local.json");
        let project_body = fs::read_to_string(&project_scope).unwrap();
        let project_value: Value = serde_json::from_str(&project_body).unwrap();
        let project_deny = project_value["permissions"]["deny"].as_array().unwrap();
        for needle in &expected {
            assert!(
                project_deny
                    .iter()
                    .any(|v| v.as_str() == Some(needle.as_str())),
                "project-scope settings missing {needle}, got {project_deny:?}"
            );
        }

        let user_body = fs::read_to_string(&user_settings).unwrap();
        let user_value: Value = serde_json::from_str(&user_body).unwrap();
        let user_deny = user_value["permissions"]["deny"].as_array().unwrap();
        for needle in &expected {
            assert!(
                user_deny
                    .iter()
                    .any(|v| v.as_str() == Some(needle.as_str())),
                "user-scope settings missing {needle}, got {user_deny:?}"
            );
        }
    }

    /// rem-e6yd / T42: cd / pushd denies installed by `restrict` are
    /// scrubbed cleanly by `unprotect` via the sidecar — no leftovers.
    #[test]
    fn cd_pushd_denies_round_trip_through_unprotect() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);

        let restrict = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&restrict, 0);

        let unprotect = run_in(
            realm.path(),
            &[
                "unprotect",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&unprotect, 0);

        let canonical = fs::canonicalize(realm.path().join("src/secret")).unwrap();
        let glob = format!("{}/**", canonical.display());
        let expected: [String; 4] = [
            format!("Bash(cd {glob})"),
            format!("Bash(cd * {glob})"),
            format!("Bash(pushd {glob})"),
            format!("Bash(pushd * {glob})"),
        ];

        // Project-scope settings file no longer carries any of them.
        let project_scope = realm.path().join(".claude/settings.local.json");
        let project_body = fs::read_to_string(&project_scope).unwrap();
        let project_value: Value = serde_json::from_str(&project_body).unwrap();
        let project_deny = project_value["permissions"]["deny"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for needle in &expected {
            assert!(
                !project_deny
                    .iter()
                    .any(|v| v.as_str() == Some(needle.as_str())),
                "expected {needle} to be scrubbed from project-scope, got {project_deny:?}"
            );
        }

        // User-scope settings file no longer carries any of them.
        let user_body = fs::read_to_string(&user_settings).unwrap();
        let user_value: Value = serde_json::from_str(&user_body).unwrap();
        let user_deny = user_value["permissions"]["deny"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for needle in &expected {
            assert!(
                !user_deny
                    .iter()
                    .any(|v| v.as_str() == Some(needle.as_str())),
                "expected {needle} to be scrubbed from user-scope, got {user_deny:?}"
            );
        }
    }

    /// rem-e6yd / T42: `plan restrict` reflects the cd/pushd defaults
    /// in its `deny_rules_to_add` projection. Pin both the bare and
    /// with-flag forms so the plan output stays in sync with what the
    /// live `restrict` would emit.
    #[test]
    fn plan_restrict_reflects_cd_pushd_defaults() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        let canonical = fs::canonicalize(realm.path().join("src/secret")).unwrap();
        let glob = format!("{}/**", canonical.display());
        let expected: [String; 4] = [
            format!("Bash(cd {glob})"),
            format!("Bash(cd * {glob})"),
            format!("Bash(pushd {glob})"),
            format!("Bash(pushd * {glob})"),
        ];
        for sf in report["config_diff"]["settings_files"].as_array().unwrap() {
            let to_add = sf["deny_rules_to_add"].as_array().unwrap();
            for needle in &expected {
                assert!(
                    to_add.iter().any(|v| v.as_str() == Some(needle.as_str())),
                    "plan restrict's deny_rules_to_add missing {needle} for {}, got {to_add:?}",
                    sf["path"].as_str().unwrap_or("<unknown>")
                );
            }
            // Sanity: the coarse remargin-cli deny IS also projected.
            assert!(
                to_add
                    .iter()
                    .any(|v| v.as_str() == Some("Bash(remargin *)")),
                "rem-egp9: plan restrict should still project the coarse remargin-cli deny, got {to_add:?}"
            );
        }
    }

    /// Idempotency: re-running CLI restrict produces the same final
    /// state. Sidecar still has one entry; settings-file rule count
    /// stays constant.
    #[test]
    fn cli_restrict_is_idempotent() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);

        let args = [
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ];
        let first = run_in(realm.path(), &args);
        assert_status(&first, 0);
        let project_scope = realm.path().join(".claude/settings.local.json");
        let first_body = fs::read_to_string(&project_scope).unwrap();

        let second = run_in(realm.path(), &args);
        assert_status(&second, 0);
        let second_body = fs::read_to_string(&project_scope).unwrap();

        assert_eq!(first_body, second_body, "idempotent re-run must match");
    }
}
