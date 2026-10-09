//! `remargin goose session-guard` dispatch diagnostics, install lifecycle, `test` and `doctor`.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use assert_cmd::cargo::CommandCargoExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;

fn guard_dir(root: &Path) -> PathBuf {
    root.join(".agents/plugins/remargin-guard")
}

fn hooks_manifest(root: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(guard_dir(root).join("hooks/hooks.json")).unwrap())
        .unwrap()
}

fn entries(root: &Path, event: &str) -> Vec<Value> {
    hooks_manifest(root)["hooks"][event]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn envelope(working_dir: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "event": "SessionStart",
        "session_id": "test",
        "working_dir": working_dir.to_string_lossy(),
    }))
    .unwrap()
}

/// Dispatch with `$HOME` pinned, so the user-scope plugin lookup lands in
/// the test's temp home rather than the developer's.
fn run_dispatch(home: &Path, cwd: &Path, stdin_bytes: &[u8]) -> Output {
    use std::io::Write as _;
    let mut child = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .args(["goose", "session-guard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_bytes)
        .unwrap();
    child.wait_with_output().unwrap()
}

fn run_lifecycle(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .args(args)
        .output()
        .unwrap()
}

fn stdout_of(out: &Output) -> &str {
    str::from_utf8(&out.stdout).unwrap()
}

fn stderr_of(out: &Output) -> &str {
    str::from_utf8(&out.stderr).unwrap()
}

fn assert_status(out: &Output, expected: i32) {
    assert_eq!(
        out.status.code(),
        Some(expected),
        "remargin exited with {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        stdout_of(out),
        stderr_of(out),
    );
}

fn status_of(out: &Output) -> String {
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    report["status"].as_str().unwrap().to_owned()
}

/// goose reads hook output as signal, so a wired guard over a parseable realm says nothing.
#[test]
fn healthy_stack_is_silent_and_exits_zero() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);

    let out = run_dispatch(home.path(), realm.path(), &envelope(realm.path()));
    assert_status(&out, 0);
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "");
}

/// goose swallows a non-zero hook exit, so the diagnostic goes to stdout and the exit code stays 0.
#[test]
fn absent_plugin_prints_a_diagnostic_on_stdout_and_exits_zero() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();

    let out = run_dispatch(home.path(), realm.path(), &envelope(realm.path()));
    assert_status(&out, 0);
    let diagnostic = stdout_of(&out);
    assert!(
        diagnostic.contains("REMARGIN GOOSE SESSION GUARD FAILURE"),
        "diagnostic should be unmissable: {diagnostic}",
    );
    assert!(
        diagnostic.contains("remargin-guard") && diagnostic.contains("remargin doctor"),
        "diagnostic should name the breakage and the repair: {diagnostic}",
    );
}

/// The plugin parses but its binary is gone, so goose spawns nothing and waves every call through.
#[test]
fn plugin_pointing_at_a_missing_binary_is_reported() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);
    let manifest = guard_dir(home.path()).join("hooks/hooks.json");
    fs::write(
        &manifest,
        serde_json::to_string_pretty(&json!({
            "hooks": { "PreToolUse": [{ "hooks": [
                { "type": "command", "command": "/nonexistent/remargin goose pretool" },
            ] }] },
        }))
        .unwrap(),
    )
    .unwrap();

    let out = run_dispatch(home.path(), realm.path(), &envelope(realm.path()));
    assert_status(&out, 0);
    assert!(
        stdout_of(&out).contains("/nonexistent/remargin"),
        "diagnostic should name the missing binary: {}",
        stdout_of(&out),
    );
}

/// An unparseable realm config would fail at tool-call time; the guard names it at session start.
#[test]
fn unparseable_realm_config_is_reported() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);
    fs::write(realm.path().join(".remargin.yaml"), ": : not yaml : :").unwrap();

    let out = run_dispatch(home.path(), realm.path(), &envelope(realm.path()));
    assert_status(&out, 0);
    assert!(
        stdout_of(&out).contains(".remargin.yaml"),
        "diagnostic should name the config: {}",
        stdout_of(&out),
    );
}

/// `install` merges into the pretool installer's plugin; `uninstall` takes back only its own entry.
#[test]
fn install_and_uninstall_touch_only_the_session_entry() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    let sibling = home.path().join(".agents/plugins/other-plugin");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(sibling.join("plugin.json"), "{}").unwrap();
    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);

    let installed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "--json", "install"],
    );
    assert_eq!(status_of(&installed), "installed");
    assert_eq!(entries(home.path(), "SessionStart").len(), 1);
    assert_eq!(entries(home.path(), "PreToolUse").len(), 1);

    let again = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "--json", "install"],
    );
    assert_eq!(status_of(&again), "already_installed");

    let removed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "--json", "uninstall"],
    );
    assert_eq!(status_of(&removed), "uninstalled");
    assert_eq!(
        entries(home.path(), "SessionStart"),
        [] as [serde_json::Value; 0]
    );
    assert_eq!(
        entries(home.path(), "PreToolUse").len(),
        1,
        "the PreToolUse entry must survive",
    );
    assert!(sibling.is_dir(), "sibling plugin must survive");
}

/// goose silently drops an invalid `matcher` regex, and a `PATH` miss at spawn time fails open.
#[test]
fn generated_entry_omits_matcher_and_uses_an_absolute_binary() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "install"],
    );

    let entry = entries(home.path(), "SessionStart")[0].clone();
    assert!(entry.get("matcher").is_none(), "matcher present: {entry}");
    let command = entry["hooks"][0]["command"].as_str().unwrap();
    let binary = command.strip_suffix(" goose session-guard").unwrap();
    assert!(
        Path::new(binary).is_absolute(),
        "hook binary must be absolute: {command}",
    );
    assert!(Path::new(binary).is_file(), "hook binary must exist");
}

/// `--local` targets the project scope and leaves the user scope alone.
#[test]
fn local_install_targets_the_project_scope() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "install", "--local"],
    );
    assert_eq!(entries(realm.path(), "SessionStart").len(), 1);
    assert!(!guard_dir(home.path()).exists());
}

/// A pretool-only plugin counts as absent: the shared directory says nothing about this entry.
#[test]
fn test_subcommand_reports_wired_and_absent() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();

    let absent = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "--json", "test"],
    );
    assert_eq!(status_of(&absent), "not_installed");

    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);
    let pretool_only = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "--json", "test"],
    );
    assert_eq!(status_of(&pretool_only), "not_installed");

    run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "install"],
    );
    let wired = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "--json", "test"],
    );
    assert_eq!(status_of(&wired), "installed");
}

/// A wired blocking guard without the backstop is flagged, and installing it clears the same run.
#[test]
fn doctor_flags_a_goose_stack_without_the_backstop() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(home.path(), realm.path(), &["claude", "pretool", "install"]);
    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);

    let user_settings = home.path().join(".claude/settings.json");
    let args = [
        "doctor",
        "--user-settings",
        user_settings.to_str().unwrap(),
        "--check=goose-session-guard",
        "--json",
    ];
    let out = run_lifecycle(home.path(), realm.path(), &args);
    assert_status(&out, 1);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let kinds: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["goose_session_guard_missing"]);

    run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "install"],
    );
    let clean = run_lifecycle(home.path(), realm.path(), &args);
    assert_status(&clean, 0);
}
