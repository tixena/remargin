use core::str;
use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn run_args(args: &[&str], cwd: &Path, home: &Path) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

fn run_doctor(cwd: &Path, user_settings: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["doctor", "--user-settings", user_settings.to_str().unwrap()];
    args.extend_from_slice(extra);
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .args(&args)
        .output()
        .unwrap()
}

fn assert_status(out: &Output, expected: i32) {
    assert_eq!(
        out.status.code(),
        Some(expected),
        "remargin exited with {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        str::from_utf8(&out.stdout).unwrap(),
        str::from_utf8(&out.stderr).unwrap(),
    );
}

fn finding_kinds(report: &Value) -> Vec<String> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap().to_owned())
        .collect()
}

/// Case 6: with both the `PreToolUse` hook and the `SessionStart` guard
/// installed, `doctor` exits 0 and reports no `SessionGuardMissing`.
#[test]
fn doctor_clean_when_both_hooks_installed() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let pretool = run_args(&["claude", "pretool", "install"], realm.path(), home.path());
    assert!(pretool.status.success());
    let guard = run_args(
        &["claude", "session-guard", "install"],
        realm.path(),
        home.path(),
    );
    assert!(guard.status.success());

    let user_settings = home.path().join(".claude/settings.json");
    let out = run_doctor(realm.path(), &user_settings, &["--json"]);
    assert_status(&out, 0);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["session_guard_installed"], Value::Bool(true));
    assert!(
        finding_kinds(&report).is_empty(),
        "expected no findings: {report}",
    );
}

/// Case 7: with only the `PreToolUse` hook installed (guard absent from
/// both scopes), `doctor` exits 1 and the sole finding is
/// `SessionGuardMissing`.
#[test]
fn doctor_flags_missing_guard_when_only_pretool_installed() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let pretool = run_args(&["claude", "pretool", "install"], realm.path(), home.path());
    assert!(pretool.status.success());

    let user_settings = home.path().join(".claude/settings.json");
    let out = run_doctor(realm.path(), &user_settings, &["--json"]);
    assert_status(&out, 1);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["session_guard_installed"], Value::Bool(false));
    assert_eq!(
        finding_kinds(&report),
        vec![String::from("session_guard_missing")],
    );
    assert_eq!(report["findings"][0]["severity"], Value::from("critical"));
    assert!(
        report["findings"][0]["remedy"]
            .as_str()
            .unwrap()
            .contains("session-guard install"),
        "remedy should name the install command: {report}",
    );
}

/// A fixture mirroring the real stale `Bash(remargin *)` entry in a
/// project settings file (with the enforcement hooks wired so doctor
/// does not short-circuit) is detected as a `LeftoverProjectedRule`,
/// and `--prompt-mode` emits an agent-executable removal instruction
/// naming the rule and its file.
#[test]
fn doctor_detects_stale_remargin_cli_deny_and_prompt_mode_repairs_it() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let pretool = run_args(&["claude", "pretool", "install"], realm.path(), home.path());
    assert!(pretool.status.success());
    let guard = run_args(
        &["claude", "session-guard", "install"],
        realm.path(),
        home.path(),
    );
    assert!(guard.status.success());

    let claude_dir = realm.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("settings.local.json"),
        r#"{ "permissions": { "deny": ["Bash(remargin *)"] } }"#,
    )
    .unwrap();

    let user_settings = home.path().join(".claude/settings.json");

    // --json: the stale rule is detected as a leftover.
    let out = run_doctor(realm.path(), &user_settings, &["--json"]);
    assert_status(&out, 1);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        finding_kinds(&report)
            .iter()
            .any(|k| k == "leftover_projected_rule"),
        "expected a leftover_projected_rule finding: {report}",
    );
    let leftover = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "leftover_projected_rule")
        .unwrap();
    assert_eq!(leftover["severity"], Value::from("warning"));
    assert!(
        leftover["message"]
            .as_str()
            .unwrap()
            .contains("Bash(remargin *)"),
        "message should name the stale rule: {report}",
    );

    // --prompt-mode: emits a removal instruction for the same rule.
    let prompt_out = run_doctor(realm.path(), &user_settings, &["--prompt-mode"]);
    let prompt = str::from_utf8(&prompt_out.stdout).unwrap();
    assert!(
        prompt.contains("Remove the deny rule `Bash(remargin *)`"),
        "prompt should instruct removal of the rule: {prompt}",
    );
    assert!(
        prompt.contains("settings.local.json"),
        "prompt should name the settings file: {prompt}",
    );
}

/// The human (non-JSON) findings render names the guard and its remedy.
#[test]
fn doctor_text_output_names_the_guard_remedy() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    run_args(&["claude", "pretool", "install"], realm.path(), home.path());

    let user_settings = home.path().join(".claude/settings.json");
    let out = run_doctor(realm.path(), &user_settings, &[]);
    assert_status(&out, 1);
    let stdout = str::from_utf8(&out.stdout).unwrap();
    assert!(stdout.contains("[CRITICAL]"), "expected critical: {stdout}");
    assert!(
        stdout.contains("remargin claude session-guard install"),
        "expected remedy command in text output: {stdout}",
    );
}
