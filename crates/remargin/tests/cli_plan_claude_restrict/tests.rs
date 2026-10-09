//! `plan claude restrict` runs, read back from `--json` and from the files left on disk.

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

/// `plan restrict` writes none of the four target files.
#[test]
fn plan_restrict_does_not_write() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
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

/// After `restrict`, a second `plan restrict` reports `noop = true` and nothing to add.
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

    let replan = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
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

/// Nothing is projected, so an existing user-scope allow raises no `allow_deny_overlap` conflict.
#[test]
fn plan_surfaces_no_allow_deny_overlap_now_projection_retired() {
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
            "claude",
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
    assert!(
        !conflicts.iter().any(|c| c["kind"] == "allow_deny_overlap"),
        "projection is empty, so no allow_deny_overlap can surface: {conflicts:?}"
    );
}

/// The hook is the single source of truth, so `settings_files` is empty.
#[test]
fn plan_wildcard_projects_no_settings_rules() {
    let realm = realm_with_claude();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
            "restrict",
            "*",
            "--user-settings",
            user_settings.to_str().unwrap(),
            "--json",
        ],
    );
    assert_status(&out, 0);
    let report = parse_json(&out);
    let settings_files = report["config_diff"]["settings_files"].as_array().unwrap();
    assert!(
        settings_files.is_empty(),
        "plan should project no settings files, got {settings_files:?}"
    );
}

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
            "claude",
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

/// The wildcard entry carries the anchor as its `absolute_path`.
#[test]
fn plan_wildcard_resolves_to_anchor() {
    let realm = realm_with_claude();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
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
