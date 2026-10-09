//! `plan claude unrestrict` runs, read back from `--json` and from the files left on disk.

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

/// Plan leaves the `.remargin.yaml` byte-identical and creates no settings or sidecar file.
#[test]
fn plan_unprotect_does_not_write() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);
    let apply = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&apply, 0);

    let yaml_path = realm.path().join(".remargin.yaml");
    let project_settings = realm.path().join(".claude/settings.local.json");
    let sidecar = realm.path().join(".claude/.remargin-restrictions.json");
    let before_yaml = fs::read_to_string(&yaml_path).unwrap();

    let out = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
            "unrestrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
            "--json",
        ],
    );
    assert_status(&out, 0);

    assert_eq!(fs::read_to_string(&yaml_path).unwrap(), before_yaml);
    assert!(
        !project_settings.exists(),
        "plan must not create project settings"
    );
    assert!(
        !user_settings.exists(),
        "plan must not create user settings"
    );
    assert!(!sidecar.exists(), "plan must not create the sidecar");
}

/// Plan reports `would_commit: true` and `noop: false`; after the live run the replan is a noop.
#[test]
fn plan_then_apply_then_replan_reports_noop() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);
    let apply = run_in(
        realm.path(),
        &[
            "claude",
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
            "claude",
            "unrestrict",
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
    assert_eq!(cd["sidecar"]["entry_action"], json!("absent"));

    let unprotect = run_in(
        realm.path(),
        &[
            "claude",
            "unrestrict",
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
            "claude",
            "unrestrict",
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

/// A tracked rule hand-deleted from one settings file is an advisory `rule_already_absent`.
#[test]
fn drift_detection_surfaces_rule_already_absent() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);
    let project_settings = realm.path().join(".claude/settings.local.json");

    // `restrict` projects nothing, so the sidecar-tracked state is seeded by hand.
    let canonical_realm = fs::canonicalize(realm.path()).unwrap();
    let target_key = format!("{}/src/secret", canonical_realm.display());
    let tracked_rule = format!("Edit({target_key}/**)");
    fs::write(
        realm.path().join(".remargin.yaml"),
        "permissions:\n  trusted_roots:\n    - path: src/secret\n",
    )
    .unwrap();
    fs::write(
        &project_settings,
        json!({ "permissions": { "deny": [] } }).to_string(),
    )
    .unwrap();
    fs::write(
        &user_settings,
        json!({ "permissions": { "deny": [tracked_rule] } }).to_string(),
    )
    .unwrap();
    let sidecar = json!({
        "version": 1_u32,
        "entries": {
            target_key: {
                "added_at": "legacy",
                "added_to_files": [project_settings.to_string_lossy(), user_settings.to_string_lossy()],
                "allow": [],
                "deny": [tracked_rule],
            }
        }
    });
    fs::write(
        realm.path().join(".claude/.remargin-restrictions.json"),
        sidecar.to_string(),
    )
    .unwrap();

    let out = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
            "unrestrict",
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
        c["kind"] == "rule_already_absent" && c["rule"].as_str() == Some(tracked_rule.as_str())
    });
    assert!(
        saw,
        "expected rule_already_absent for {tracked_rule}: {conflicts:?}"
    );
}

/// The wildcard plan's projection matches the post-apply state: sidecar empty, entry stripped.
#[test]
fn wildcard_plan_then_apply() {
    let realm = realm_with_claude();
    let user_settings = user_settings_arg(&realm);
    let apply = run_in(
        realm.path(),
        &[
            "claude",
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
            "claude",
            "unrestrict",
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
            "claude",
            "unrestrict",
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
            "claude",
            "unrestrict",
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

/// With A and B restricted, `plan unrestrict A` describes only A's reversal.
#[test]
fn multi_path_independence() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/a")).unwrap();
    fs::create_dir_all(realm.path().join("src/b")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let restrict_a = run_in(
        realm.path(),
        &[
            "claude",
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
            "claude",
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
            "claude",
            "unrestrict",
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

/// A never-restricted path: both entries `Absent`, both missing-entry conflicts, no commit.
#[test]
fn never_restricted_reports_noop_with_both_missing_conflicts() {
    let realm = realm_with_claude();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
            "unrestrict",
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
