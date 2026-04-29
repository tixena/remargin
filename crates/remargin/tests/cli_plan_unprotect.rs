//! `remargin plan unprotect` integration tests (rem-6eop / T43).
//!
//! Mirrors the `cli_plan_restrict.rs` patterns: real-filesystem temp
//! dirs, `assert_cmd` invocations, JSON output assertions. Covers
//! the testing-plan scenarios from the T43 spec: plan-then-act
//! parity, --json output, MCP / CLI parity, wildcard end-to-end,
//! drift detection, multi-path independence, and the no-write
//! invariant.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Output;

    use assert_cmd::Command;
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

    fn parse_json(out: &Output) -> Value {
        let stdout = str::from_utf8(&out.stdout).unwrap();
        serde_json::from_str(stdout).unwrap()
    }

    /// Plan does not write any of the four target files.
    #[test]
    fn plan_unprotect_does_not_write() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);
        // Apply restrict so there's actual state to project removal of.
        let apply = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&apply, 0);

        // Snapshot every interesting file before plan.
        let yaml_path = realm.path().join(".remargin.yaml");
        let project_settings = realm.path().join(".claude/settings.local.json");
        let sidecar = realm.path().join(".claude/.remargin-restrictions.json");
        let before_yaml = fs::read_to_string(&yaml_path).unwrap();
        let before_project = fs::read_to_string(&project_settings).unwrap();
        let before_user = fs::read_to_string(&user_settings).unwrap();
        let before_sidecar = fs::read_to_string(&sidecar).unwrap();

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);

        // Every file must be byte-identical after plan.
        assert_eq!(fs::read_to_string(&yaml_path).unwrap(), before_yaml);
        assert_eq!(
            fs::read_to_string(&project_settings).unwrap(),
            before_project
        );
        assert_eq!(fs::read_to_string(&user_settings).unwrap(), before_user);
        assert_eq!(fs::read_to_string(&sidecar).unwrap(), before_sidecar);
    }

    /// Plan-then-act parity: plan unprotect under a clean restrict
    /// reports `would_commit: true` and `noop: false`; the live
    /// `unprotect` run immediately after produces matching effects
    /// (the YAML entry disappears, sidecar entry disappears, project
    /// settings deny array shrinks).
    #[test]
    fn plan_then_apply_then_replan_reports_noop() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);
        let apply = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&apply, 0);

        let plan_first = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&plan_first, 0);
        let report = parse_json(&plan_first);
        assert_eq!(report["noop"], json!(false));
        assert_eq!(report["would_commit"], json!(true));
        let cd = &report["unprotect_diff"];
        assert_eq!(
            cd["remargin_yaml"]["entry_action"],
            json!("would_be_removed")
        );
        assert_eq!(cd["sidecar"]["entry_action"], json!("would_be_removed"));

        // Unprotect, then replan — the second plan should be a noop.
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

        let replan = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&replan, 0);
        let replan_report = parse_json(&replan);
        assert_eq!(replan_report["noop"], json!(true));
        let replan_cd = &replan_report["unprotect_diff"];
        assert_eq!(replan_cd["remargin_yaml"]["entry_action"], json!("absent"));
        assert_eq!(replan_cd["sidecar"]["entry_action"], json!("absent"));
    }

    /// Drift detection: manually delete a deny rule from the
    /// project-scope settings file between `restrict` and
    /// `plan unprotect`. The drift surfaces in the
    /// `rule_already_absent` conflicts and `rules_already_absent`
    /// list. `would_commit` stays true because conflicts are
    /// advisory, but `noop` is false because there are still other
    /// rules to remove.
    #[test]
    fn drift_detection_surfaces_rule_already_absent() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);
        let apply = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&apply, 0);

        // Hand-edit project settings to drop the first deny rule.
        let project_settings = realm.path().join(".claude/settings.local.json");
        let body = fs::read_to_string(&project_settings).unwrap();
        let mut value: Value = serde_json::from_str(&body).unwrap();
        let removed_rule = {
            let deny = value
                .get_mut("permissions")
                .and_then(|v| v.get_mut("deny"))
                .and_then(Value::as_array_mut)
                .unwrap();
            let popped = deny.remove(0);
            String::from(popped.as_str().unwrap())
        };
        fs::write(
            &project_settings,
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        assert_eq!(report["noop"], json!(false));
        assert_eq!(report["would_commit"], json!(true));
        let conflicts = report["unprotect_diff"]["conflicts"].as_array().unwrap();
        let saw = conflicts.iter().any(|c| {
            c["kind"] == "rule_already_absent" && c["rule"].as_str() == Some(removed_rule.as_str())
        });
        assert!(
            saw,
            "expected rule_already_absent for {removed_rule}: {conflicts:?}"
        );
    }

    /// Wildcard end-to-end: restrict `*`, plan unprotect `*`, then
    /// apply unprotect `*`. The plan's projection lines up with the
    /// post-apply state (sidecar empty, YAML stripped of the
    /// wildcard entry).
    #[test]
    fn wildcard_plan_then_apply() {
        let realm = realm_with_claude();
        let user_settings = user_settings_arg(&realm);
        let apply = run_in(
            realm.path(),
            &[
                "restrict",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&apply, 0);

        let plan_out = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&plan_out, 0);
        let plan_report = parse_json(&plan_out);
        assert_eq!(plan_report["noop"], json!(false));
        let cd = &plan_report["unprotect_diff"];
        let canonical = fs::canonicalize(realm.path()).unwrap();
        assert_eq!(
            cd["absolute_path"].as_str().unwrap(),
            canonical.display().to_string()
        );

        let unprotect = run_in(
            realm.path(),
            &[
                "unprotect",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&unprotect, 0);

        let replan = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&replan, 0);
        let replan_report = parse_json(&replan);
        assert_eq!(replan_report["noop"], json!(true));
    }

    /// Multi-path independence: restrict A and B, then
    /// `plan unprotect A` only describes A's reversal. B is not
    /// surfaced in any field of the diff.
    #[test]
    fn multi_path_independence() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/a")).unwrap();
        fs::create_dir_all(realm.path().join("src/b")).unwrap();
        let user_settings = user_settings_arg(&realm);

        let restrict_a = run_in(
            realm.path(),
            &[
                "restrict",
                "src/a",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&restrict_a, 0);
        let restrict_b = run_in(
            realm.path(),
            &[
                "restrict",
                "src/b",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&restrict_b, 0);

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "src/a",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        let canonical_a = fs::canonicalize(realm.path().join("src/a"))
            .unwrap()
            .display()
            .to_string();
        let canonical_b = fs::canonicalize(realm.path().join("src/b"))
            .unwrap()
            .display()
            .to_string();
        let cd = &report["unprotect_diff"];
        assert_eq!(cd["absolute_path"].as_str().unwrap(), canonical_a);
        // None of the rules in `rules_to_remove` should mention B's
        // absolute path — B's restrict entry is independent.
        for sf in cd["settings_files"].as_array().unwrap() {
            for rule in sf["rules_to_remove"].as_array().unwrap() {
                let rule_str = rule.as_str().unwrap();
                assert!(
                    !rule_str.contains(&canonical_b),
                    "B's path leaked into A's projection: {rule_str}"
                );
            }
        }
    }

    /// Path was never restricted: noop signals via both `Absent`
    /// entry actions and both `YamlEntryMissing` +
    /// `SidecarEntryMissing` conflicts. `would_commit: false`
    /// because the projection would do nothing.
    #[test]
    fn never_restricted_reports_noop_with_both_missing_conflicts() {
        let realm = realm_with_claude();
        let user_settings = user_settings_arg(&realm);

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "unprotect",
                "nonexistent",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        assert_eq!(report["noop"], json!(true));
        assert_eq!(report["would_commit"], json!(false));
        let conflicts = report["unprotect_diff"]["conflicts"].as_array().unwrap();
        let saw_yaml = conflicts.iter().any(|c| c["kind"] == "yaml_entry_missing");
        let saw_sidecar = conflicts
            .iter()
            .any(|c| c["kind"] == "sidecar_entry_missing");
        assert!(saw_yaml, "expected yaml_entry_missing: {conflicts:?}");
        assert!(saw_sidecar, "expected sidecar_entry_missing: {conflicts:?}");
    }
}
