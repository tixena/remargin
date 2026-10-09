//! `remargin goose pretool` dispatch verdicts, install lifecycle, `test` and `doctor`.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use assert_cmd::cargo::CommandCargoExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;

/// A realm whose `secret/` subtree is remargin-managed.
fn managed_realm() -> TempDir {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    fs::write(
        realm.path().join(".remargin.yaml"),
        "permissions:\n  trusted_roots:\n    - path: secret\n",
    )
    .unwrap();
    fs::write(realm.path().join("secret/idea.md"), "hi\n").unwrap();
    realm
}

fn envelope(tool_name: &str, working_dir: &Path, tool_input: &Value) -> Vec<u8> {
    let event = json!({
        "event": "PreToolUse",
        "session_id": "test",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "working_dir": working_dir.to_string_lossy(),
    });
    serde_json::to_vec(&event).unwrap()
}

fn run_dispatch(stdin_bytes: &[u8]) -> Output {
    use std::io::Write as _;
    let mut child = Command::cargo_bin("remargin")
        .unwrap()
        .args(["goose", "pretool"])
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

/// Run a lifecycle subcommand with `$HOME` pinned so the user scope lands
/// in the temp home rather than the developer's.
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

fn guard_dir(root: &Path) -> PathBuf {
    root.join(".agents/plugins/remargin-guard")
}

fn report_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn status_of(out: &Output) -> String {
    report_of(out)["status"].as_str().unwrap().to_owned()
}

/// A block fires on stdout, on stderr and through exit code 2: one channel alone can be ignored.
#[test]
fn block_fires_on_stdout_stderr_and_exit_two() {
    let realm = managed_realm();
    let stdin = envelope(
        "developer__text_editor",
        realm.path(),
        &json!({
            "command": "write",
            "path": realm.path().join("secret/idea.md").to_string_lossy(),
        }),
    );
    let out = run_dispatch(&stdin);

    assert_status(&out, 2);
    let decision: Value = serde_json::from_str(stdout_of(&out).trim()).unwrap();
    assert_eq!(decision["decision"], Value::from("block"));
    let reason = decision["reason"].as_str().unwrap();
    assert!(
        reason.contains("remargin__write"),
        "reason should name the remargin op: {reason}",
    );
    // goose names the op `remargin__write`; `mcp__remargin__*` is nothing this session can call.
    assert!(
        !reason.contains("mcp__remargin__"),
        "reason should carry goose's tool namespacing: {reason}",
    );
    assert!(
        stderr_of(&out).contains(reason),
        "stderr must carry the same reason:\nstderr: {}\nreason: {reason}",
        stderr_of(&out),
    );
}

/// goose reads any output as a decision, so an allow is silent on both channels and exits 0.
#[test]
fn allow_is_silent_and_exits_zero() {
    let realm = managed_realm();
    let stdin = envelope(
        "developer__text_editor",
        realm.path(),
        &json!({
            "command": "write",
            "path": realm.path().join("public/notes.md").to_string_lossy(),
        }),
    );
    let out = run_dispatch(&stdin);

    assert_status(&out, 0);
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "");
}

/// A shell command reaching into the managed subtree blocks the same way.
#[test]
fn shell_reaching_a_managed_path_blocks() {
    let realm = managed_realm();
    let outside = TempDir::new().unwrap();
    let stdin = envelope(
        "developer__shell",
        outside.path(),
        &json!({
            "command": format!("cat {}", realm.path().join("secret/idea.md").display()),
        }),
    );
    let out = run_dispatch(&stdin);
    assert_status(&out, 2);
    assert!(
        stdout_of(&out).contains("\"decision\":\"block\""),
        "stdout: {}",
        stdout_of(&out),
    );
}

/// goose treats a silent hook as permission to proceed, so an unreadable payload is a block.
#[test]
fn malformed_payload_blocks_on_both_channels() {
    let out = run_dispatch(b"{\"tool_name\": ");
    assert_status(&out, 2);
    assert!(
        stdout_of(&out).contains("\"decision\":\"block\""),
        "stdout: {}",
        stdout_of(&out),
    );
    assert!(!stderr_of(&out).is_empty(), "stderr must carry the reason");
}

/// `uninstall` removes exactly the plugin `install` wrote; a sibling plugin survives both.
#[test]
fn install_then_uninstall_round_trips_and_preserves_siblings() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    let sibling = home.path().join(".agents/plugins/other-plugin");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(sibling.join("plugin.json"), "{}").unwrap();

    let installed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "pretool", "--json", "install"],
    );
    assert_status(&installed, 0);
    let dir = guard_dir(home.path());
    assert!(dir.join("plugin.json").is_file(), "plugin.json missing");
    assert!(
        dir.join("hooks/hooks.json").is_file(),
        "hooks/hooks.json missing",
    );

    let again = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "pretool", "--json", "install"],
    );
    assert_eq!(status_of(&again), "already_installed");

    let removed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "pretool", "--json", "uninstall"],
    );
    assert_eq!(status_of(&removed), "uninstalled");
    assert!(!dir.exists(), "guard directory should be gone");
    assert!(sibling.is_dir(), "sibling plugin must survive");
}

/// goose silently drops an invalid `matcher` regex, and a `PATH` miss at spawn time fails open.
#[test]
fn generated_hook_manifest_omits_matcher_and_uses_an_absolute_binary() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);

    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(guard_dir(home.path()).join("hooks/hooks.json")).unwrap(),
    )
    .unwrap();
    let entry = &manifest["hooks"]["PreToolUse"][0];
    assert!(entry.get("matcher").is_none(), "matcher present: {entry}");

    let command = entry["hooks"][0]["command"].as_str().unwrap();
    let binary = command.strip_suffix(" goose pretool").unwrap();
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
        &["goose", "pretool", "install", "--local"],
    );
    assert!(guard_dir(realm.path()).join("plugin.json").is_file());
    assert!(!guard_dir(home.path()).exists());
}

/// The three verdicts `test` distinguishes: wired, absent, and present but corrupt.
#[test]
fn test_subcommand_reports_wired_absent_and_corrupt() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();

    let absent = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "pretool", "--json", "test"],
    );
    assert_eq!(status_of(&absent), "not_installed");

    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);
    let wired = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "pretool", "--json", "test"],
    );
    assert_eq!(status_of(&wired), "installed");

    fs::write(
        guard_dir(home.path()).join("hooks/hooks.json"),
        "{ not json",
    )
    .unwrap();
    let corrupt = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "pretool", "--json", "test"],
    );
    let corrupt_report = report_of(&corrupt);
    assert_eq!(status_of(&corrupt), "broken");
    assert!(
        corrupt_report["detail"]
            .as_str()
            .unwrap()
            .contains("hooks.json"),
        "broken detail should name the manifest: {corrupt_report}",
    );
}

/// goose without the guard plugin is a `doctor` finding that `--check=goose-guard` selects alone.
#[test]
fn doctor_flags_a_goose_install_without_the_guard() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".agents/plugins")).unwrap();
    run_lifecycle(home.path(), realm.path(), &["claude", "pretool", "install"]);
    run_lifecycle(
        home.path(),
        realm.path(),
        &["claude", "session-guard", "install"],
    );

    let user_settings = home.path().join(".claude/settings.json");
    let out = run_lifecycle(
        home.path(),
        realm.path(),
        &[
            "doctor",
            "--user-settings",
            user_settings.to_str().unwrap(),
            "--check=goose-guard",
            "--json",
        ],
    );
    assert_status(&out, 1);
    let report = report_of(&out);
    let kinds: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["goose_guard_missing"]);

    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);
    let clean = run_lifecycle(
        home.path(),
        realm.path(),
        &[
            "doctor",
            "--user-settings",
            user_settings.to_str().unwrap(),
            "--check=goose-guard",
            "--json",
        ],
    );
    assert_status(&clean, 0);
}
