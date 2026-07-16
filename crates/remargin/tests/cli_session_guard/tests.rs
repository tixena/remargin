use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::cargo::{CommandCargoExt as _, cargo_bin};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Directory holding the built `remargin` binary — used as a hermetic
/// `PATH` so the guard's on-PATH check resolves deterministically.
fn remargin_bin_dir() -> PathBuf {
    let bin = cargo_bin("remargin");
    bin.parent().unwrap().to_path_buf()
}

fn run_guard_in(cwd: &Path, path_env: &str) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("PATH", path_env)
        .args(["claude", "session-guard"])
        .output()
        .unwrap()
}

fn run_args(args: &[&str], cwd: &Path, home: &Path) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

/// Case 8: the guard runs in a realm whose `.remargin.yaml` does not
/// parse. `SessionStart` cannot block, so the process exits 0 and
/// surfaces the failure as diagnostic JSON on stdout — `additionalContext`
/// names the config and `systemMessage` points at `remargin doctor`.
#[test]
fn guard_with_broken_realm_config_emits_diagnostic() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join(".remargin.yaml"), ": : not valid : :").unwrap();

    // remargin resolves on PATH, so the broken config is the only failure.
    let out = run_guard_in(realm.path(), remargin_bin_dir().to_str().unwrap());
    assert_eq!(
        out.status.code(),
        Some(0_i32),
        "SessionStart guard must exit 0 (it cannot block); stderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["hookEventName"],
        json!("SessionStart")
    );
    let ctx = payload["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        ctx.contains(".remargin.yaml"),
        "additionalContext should name the config: {ctx}",
    );
    assert!(
        payload["systemMessage"]
            .as_str()
            .unwrap()
            .contains("remargin doctor"),
        "systemMessage should point at doctor: {payload}",
    );
}

/// Binary on PATH + a parseable realm config → the session proceeds clean
/// (exit 0, empty stdout).
#[test]
fn guard_with_valid_config_proceeds_clean() {
    let realm = TempDir::new().unwrap();
    fs::write(
        realm.path().join(".remargin.yaml"),
        "identity: alice\ntype: human\n",
    )
    .unwrap();

    let out = run_guard_in(realm.path(), remargin_bin_dir().to_str().unwrap());
    assert_eq!(out.status.code(), Some(0_i32));
    assert!(
        out.stdout.is_empty(),
        "a clean guard run emits nothing: {}",
        String::from_utf8_lossy(&out.stdout),
    );
}

/// `remargin` absent from PATH → the guard exits 0 but emits a diagnostic
/// whose `additionalContext` explains the fail-open (exit-127) risk.
#[test]
fn guard_with_remargin_off_path_emits_diagnostic() {
    let realm = TempDir::new().unwrap();

    // The binary still runs (assert_cmd invokes it by absolute path), but
    // its PATH lookup for a bare `remargin` finds nothing.
    let empty = TempDir::new().unwrap();
    let out = run_guard_in(realm.path(), empty.path().to_str().unwrap());
    assert_eq!(out.status.code(), Some(0_i32));
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ctx = payload["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        ctx.contains("PATH"),
        "additionalContext should mention PATH: {ctx}",
    );
}

#[test]
fn session_guard_install_local_writes_hook_to_project_settings() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let out = run_args(
        &["claude", "session-guard", "install", "--local"],
        realm.path(),
        home.path(),
    );
    assert!(
        out.status.success(),
        "install --local failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let settings_path = realm.path().join(".claude/settings.json");
    let body = fs::read_to_string(&settings_path).unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();
    let entries = value["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        "remargin claude session-guard",
    );
}

#[test]
fn session_guard_install_is_idempotent() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    run_args(
        &["claude", "session-guard", "install", "--local"],
        realm.path(),
        home.path(),
    );
    let second = run_args(
        &["claude", "session-guard", "--json", "install", "--local"],
        realm.path(),
        home.path(),
    );
    assert!(second.status.success());
    let payload: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(payload["status"].as_str().unwrap(), "already_installed");

    let body = fs::read_to_string(realm.path().join(".claude/settings.json")).unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
}

#[test]
fn session_guard_test_reports_install_state() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let before = run_args(
        &["claude", "session-guard", "--json", "test", "--local"],
        realm.path(),
        home.path(),
    );
    let before_json: Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before_json["status"].as_str().unwrap(), "not_installed");

    run_args(
        &["claude", "session-guard", "install", "--local"],
        realm.path(),
        home.path(),
    );

    let after = run_args(
        &["claude", "session-guard", "--json", "test", "--local"],
        realm.path(),
        home.path(),
    );
    let after_json: Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_eq!(after_json["status"].as_str().unwrap(), "installed");
}

#[test]
fn session_guard_uninstall_preserves_pretool_hook() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    // Both hooks share one settings file; uninstalling the guard must not
    // touch the PreToolUse enforcement hook.
    run_args(
        &["claude", "pretool", "install", "--local"],
        realm.path(),
        home.path(),
    );
    run_args(
        &["claude", "session-guard", "install", "--local"],
        realm.path(),
        home.path(),
    );

    let out = run_args(
        &["claude", "session-guard", "--json", "uninstall", "--local"],
        realm.path(),
        home.path(),
    );
    assert!(out.status.success());
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(payload["status"].as_str().unwrap(), "uninstalled");

    let body = fs::read_to_string(realm.path().join(".claude/settings.json")).unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();
    assert!(
        value["hooks"]["PreToolUse"].is_array(),
        "PreToolUse hook must survive guard uninstall: {value}",
    );
    assert!(
        value["hooks"].get("SessionStart").is_none(),
        "SessionStart entry should be gone: {value}",
    );
}
