//! `remargin plan restrict` integration tests (rem-puy5).
//!
//! Mirrors the `cli_restrict.rs` patterns: real-filesystem temp dirs,
//! `assert_cmd` invocations, JSON output assertions. Covers
//! scenarios 16-22 of the rem-puy5 testing plan: plan + apply parity,
//! the no-write invariant, the noop covenant, allow-vs-deny overlap
//! detection, anchor-surprise detection, MCP/CLI parity, and the
//! wildcard projection.

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

    /// Scenario 17: `plan restrict` does not write any of the four
    /// target files.
    #[test]
    fn plan_restrict_does_not_write() {
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

        let yaml_path = realm.path().join(".remargin.yaml");
        let project_settings = realm.path().join(".claude/settings.local.json");
        let sidecar = realm.path().join(".claude/.remargin-restrictions.json");
        assert!(!yaml_path.exists(), "plan must not create .remargin.yaml");
        assert!(
            !project_settings.exists(),
            "plan must not create project settings"
        );
        assert!(
            !user_settings.exists(),
            "plan must not create user settings"
        );
        assert!(!sidecar.exists(), "plan must not create sidecar");
    }

    /// Scenario 16 + 18: plan + apply parity AND noop covenant. After
    /// `restrict`, a second `plan restrict` reports noop = true and
    /// every entry as Noop / nothing-to-add.
    #[test]
    fn plan_then_apply_then_replan_reports_noop() {
        let realm = realm_with_claude();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = user_settings_arg(&realm);

        // Apply, then replan.
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

        let replan = run_in(
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
        assert_status(&replan, 0);
        let report = parse_json(&replan);
        assert_eq!(report["noop"], json!(true));
        let cd = &report["config_diff"];
        assert_eq!(cd["remargin_yaml"]["entry_action"], json!("noop"));
        assert_eq!(cd["sidecar"]["entry_action"], json!("noop"));
        for sf in cd["settings_files"].as_array().unwrap() {
            assert_eq!(sf["deny_rules_to_add"], json!([]));
            assert_eq!(sf["allow_rules_to_add"], json!([]));
        }
    }

    /// rem-egp9: the projection no longer emits per-tool path denies,
    /// so a user-scope `Read(<path>/**)` allow has nothing on the
    /// projected deny side to overlap with.
    #[test]
    fn plan_surfaces_allow_deny_overlap_in_user_settings() {
        let realm = realm_with_claude();
        let target = realm.path().join("src/secret");
        fs::create_dir_all(&target).unwrap();
        let canonical = fs::canonicalize(&target).unwrap();
        let allow_pattern = format!("{}/**", canonical.display());
        let user_settings = user_settings_arg(&realm);
        let body = json!({
            "permissions": {
                "allow": [format!("Read(//{allow_pattern})")],
                "deny": []
            }
        });
        fs::write(&user_settings, body.to_string()).unwrap();

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
        let report = parse_json(&out);
        let conflicts = report["config_diff"]["conflicts"].as_array().unwrap();
        let saw_overlap = conflicts.iter().any(|c| c["kind"] == "allow_deny_overlap");
        assert!(
            !saw_overlap,
            "rem-egp9: native-tool allow has no projected deny to overlap with: {conflicts:?}",
        );
    }

    /// rem-egp9: the format-drift overlap case is moot now that no
    /// native-tool path denies are projected. The legacy
    /// `Read(/realm/**)` allow has nothing on the projected deny side
    /// to overlap with.
    #[test]
    fn plan_overlap_seeded_with_single_slash_allow_still_fires() {
        let realm = realm_with_claude();
        let user_settings = user_settings_arg(&realm);
        let canonical = fs::canonicalize(realm.path()).unwrap();
        let body = json!({
            "permissions": {
                "allow": [format!("Read({}/**)", canonical.display())],
                "deny": []
            }
        });
        fs::write(&user_settings, body.to_string()).unwrap();

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "restrict",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        let conflicts = report["config_diff"]["conflicts"].as_array().unwrap();
        let saw_overlap = conflicts.iter().any(|c| c["kind"] == "allow_deny_overlap");
        assert!(
            !saw_overlap,
            "rem-egp9: legacy native-tool allow no longer overlaps the minimised projection: {conflicts:?}",
        );
    }

    /// rem-em33: emitted deny rules carry exactly one leading slash
    /// before the path glob (no `//`, no `///`), matching Claude's
    /// documented format and the user-scope settings file's rules.
    #[test]
    fn plan_emits_single_slash_path_globs() {
        let realm = realm_with_claude();
        let user_settings = user_settings_arg(&realm);
        let canonical = fs::canonicalize(realm.path()).unwrap();

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "restrict",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        let cd = &report["config_diff"];
        let realm_str = canonical.display().to_string();
        for sf in cd["settings_files"].as_array().unwrap() {
            for rule in sf["deny_rules_to_add"].as_array().unwrap() {
                let rule_str = rule.as_str().unwrap();
                let slashed = format!("//{realm_str}");
                assert!(
                    !rule_str.contains(&slashed),
                    "rule {rule_str} must not have multi-slash before path"
                );
                let triple = format!("///{realm_str}");
                assert!(
                    !rule_str.contains(&triple),
                    "rule {rule_str} must not have triple-slash before path"
                );
            }
        }
    }

    /// rem-egp9: subtree-shadow overlap can no longer fire — the
    /// minimised projection emits only `Bash(remargin *)` plus
    /// optional `also_deny_bash` extras. There is no broader projected
    /// deny to shadow a more-specific user-scope allow.
    #[test]
    fn plan_overlap_subtree_shadow_reports_kind() {
        let realm = realm_with_claude();
        let user_settings = user_settings_arg(&realm);
        let canonical = fs::canonicalize(realm.path()).unwrap();
        let body = json!({
            "permissions": {
                "allow": [format!("Read({}/safe)", canonical.display())],
                "deny": []
            }
        });
        fs::write(&user_settings, body.to_string()).unwrap();

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "restrict",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        let conflicts = report["config_diff"]["conflicts"].as_array().unwrap();
        let saw_shadow = conflicts.iter().any(|c| {
            c["kind"] == "allow_deny_overlap"
                && c["overlap_kind"] == "allow_shadowed_by_broader_deny"
        });
        assert!(
            !saw_shadow,
            "rem-egp9: minimised projection has no broad path deny to shadow allow: {conflicts:?}",
        );
    }

    /// Scenario 20: anchor surprise surfaces when running from a
    /// subdirectory deeper than the realm anchor.
    #[test]
    fn plan_surfaces_anchor_is_ancestor_when_run_from_subdir() {
        let realm = realm_with_claude();
        let deep = realm.path().join("sub/sub2");
        fs::create_dir_all(&deep).unwrap();
        let user_settings = user_settings_arg(&realm);

        let out = run_in(
            &deep,
            &[
                "plan",
                "restrict",
                "sub/sub2/file",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        let conflicts = report["config_diff"]["conflicts"].as_array().unwrap();
        let saw_anchor = conflicts.iter().any(|c| c["kind"] == "anchor_is_ancestor");
        assert!(
            saw_anchor,
            "expected anchor_is_ancestor in conflicts: {conflicts:?}"
        );
    }

    /// Scenario 22: wildcard form projects realm-wide rules with the
    /// anchor as `absolute_path`.
    #[test]
    fn plan_wildcard_resolves_to_anchor() {
        let realm = realm_with_claude();
        let user_settings = user_settings_arg(&realm);

        let out = run_in(
            realm.path(),
            &[
                "plan",
                "restrict",
                "*",
                "--user-settings",
                user_settings.to_str().unwrap(),
                "--json",
            ],
        );
        assert_status(&out, 0);
        let report = parse_json(&out);
        let cd = &report["config_diff"];
        let abs = cd["absolute_path"].as_str().unwrap();
        let canonical = fs::canonicalize(realm.path()).unwrap();
        assert_eq!(abs, canonical.display().to_string());
    }
}
