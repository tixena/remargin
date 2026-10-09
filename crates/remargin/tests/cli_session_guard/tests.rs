//! `remargin claude session-guard` diagnostics and the install lifecycle of its hook entry.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::cargo::{CommandCargoExt as _, cargo_bin};
use remargin_core::permissions::pretool_install::{HOOK_MATCHER, LEGACY_HOOK_COMMAND};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Directory holding the built `remargin` binary — used as a hermetic
/// `PATH` so the guard's on-PATH check resolves deterministically.
fn remargin_bin_dir() -> PathBuf {
    let bin = cargo_bin("remargin");
    bin.parent().unwrap().to_path_buf()
}

/// `HOME` is always redirected at a temp dir: the guard reads the
/// user-scope settings file, so the developer's own install would
/// otherwise decide the outcome.
fn run_guard_in(cwd: &Path, home: &Path, path_env: &str) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", home)
        .env("PATH", path_env)
        .args(["claude", "session-guard"])
        .output()
        .unwrap()
}

/// Declare the `PreToolUse` entry an install predating absolute binary
/// paths left behind — the one case whose spawnability rides on `PATH`.
fn write_path_relative_hook(realm: &Path) {
    let settings = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [ { "type": "command", "command": LEGACY_HOOK_COMMAND } ]
                }
            ]
        }
    });
    fs::create_dir_all(realm.join(".claude")).unwrap();
    fs::write(
        realm.join(".claude/settings.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
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

/// `SessionStart` cannot block: an unparseable realm config is a stdout diagnostic and exit 0.
#[test]
fn guard_with_broken_realm_config_emits_diagnostic() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    run_args(
        &["claude", "pretool", "install", "--local"],
        realm.path(),
        home.path(),
    );
    fs::write(realm.path().join(".remargin.yaml"), ": : not valid : :").unwrap();

    let out = run_guard_in(
        realm.path(),
        home.path(),
        remargin_bin_dir().to_str().unwrap(),
    );
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

/// A live hook entry and a parseable realm config: exit 0 with empty stdout.
#[test]
fn guard_with_valid_config_proceeds_clean() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    run_args(
        &["claude", "pretool", "install", "--local"],
        realm.path(),
        home.path(),
    );
    fs::write(
        realm.path().join(".remargin.yaml"),
        "identity: alice\ntype: human\n",
    )
    .unwrap();

    let out = run_guard_in(
        realm.path(),
        home.path(),
        remargin_bin_dir().to_str().unwrap(),
    );
    assert_eq!(out.status.code(), Some(0_i32));
    assert!(
        out.stdout.is_empty(),
        "a clean guard run emits nothing: {}",
        String::from_utf8_lossy(&out.stdout),
    );
}

/// A `PATH`-relative entry with `remargin` off PATH: the diagnostic explains the fail-open risk.
#[test]
fn guard_with_remargin_off_path_emits_diagnostic() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_path_relative_hook(realm.path());

    // The binary runs by absolute path; only its own lookup of a bare `remargin` finds nothing.
    let empty = TempDir::new().unwrap();
    let out = run_guard_in(realm.path(), home.path(), empty.path().to_str().unwrap());
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

/// With no `PreToolUse` entry in either scope the guard names both scopes and the install command.
#[test]
fn guard_without_any_hook_entry_emits_diagnostic() {
    let realm = TempDir::new().unwrap();
    let realm_path = fs::canonicalize(realm.path()).unwrap();
    let home = TempDir::new().unwrap();

    let out = run_guard_in(
        &realm_path,
        home.path(),
        remargin_bin_dir().to_str().unwrap(),
    );
    assert_eq!(
        out.status.code(),
        Some(0_i32),
        "SessionStart guard must exit 0 (it cannot block); stderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ctx = payload["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let user_settings = home.path().join(".claude/settings.json");
    let project_settings = realm_path.join(".claude/settings.json");
    assert!(
        ctx.contains(user_settings.to_str().unwrap())
            && ctx.contains(project_settings.to_str().unwrap())
            && ctx.contains("remargin claude pretool install"),
        "additionalContext should name both scopes and the install command: {ctx}",
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
    // An absolute binary path: a backstop that silently fails to spawn is worse than none.
    let expected = format!("{} claude session-guard", cargo_bin("remargin").display());
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        expected
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

    // Both hooks share one settings file; uninstalling the guard must leave the `PreToolUse` hook.
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
