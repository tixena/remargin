use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use assert_cmd::cargo::CommandCargoExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;

fn realm_with_claude() -> TempDir {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join(".claude")).unwrap();
    realm
}

fn run_pretool(stdin_bytes: &[u8]) -> Output {
    use std::io::Write as _;
    let mut child = Command::cargo_bin("remargin")
        .unwrap()
        .args(["claude", "pretool"])
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

fn restrict_in(realm_path: &Path, path: &str, user_settings: &Path) {
    let out = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(realm_path)
        .args([
            "claude",
            "restrict",
            path,
            "--user-settings",
            user_settings.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "restrict failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

fn envelope(tool: &str, cwd: &Path, tool_input: &Value) -> Vec<u8> {
    let event = json!({
        "session_id": "test",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": cwd.to_string_lossy(),
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": tool_input,
    });
    serde_json::to_vec(&event).unwrap()
}

/// Scenario 21: end-to-end against a real `claude restrict`-ed
/// realm. The hook denies the Read with the canonical message.
#[test]
fn end_to_end_against_real_claude_restricted_realm() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("secret/foo.md");
    let stdin = envelope("Read", realm.path(), &json!({ "file_path": target }));

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.is_empty());
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["hookEventName"],
        json!("PreToolUse")
    );
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
    let reason = payload["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("mcp__remargin__get"));
}

/// Scenario 7 (target-path scope): the session cwd sits outside the
/// realm, but the target is an absolute path inside a `claude
/// restrict`-ed realm. Scope is resolved from the target, so the hook
/// still denies — exit 0 with the deny payload on stdout.
#[test]
fn cwd_outside_realm_absolute_target_inside_denies() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let outside = TempDir::new().unwrap();
    let target = realm.path().join("secret/foo.md");
    let stdin = envelope("Read", outside.path(), &json!({ "file_path": target }));

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
    let reason = payload["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("mcp__remargin__get"));
}

/// Scenario 22: exit 0 with empty stdout when the path is
/// unrestricted.
#[test]
fn unrestricted_call_exits_zero_with_empty_stdout() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    fs::create_dir_all(realm.path().join("public")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("public/foo.md");
    let stdin = envelope("Read", realm.path(), &json!({ "file_path": target }));

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    assert!(
        out.stdout.is_empty(),
        "expected empty stdout for silent allow"
    );
}

/// Scenario 23: malformed stdin exits 2 with a non-empty stderr
/// (Claude Code feeds stderr back to the model on exit 2).
#[test]
fn malformed_stdin_exits_two_with_stderr() {
    let out = run_pretool(b"not json");
    assert_eq!(out.status.code(), Some(2_i32));
    assert!(!out.stderr.is_empty(), "expected stderr diagnostic");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("malformed PreToolUse event"));
}

/// Scenario 24: env-var prefix on a Bash command does not hide the
/// real verb from the extractor.
#[test]
fn env_var_prefix_does_not_hide_verb() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("secret/x");
    let command = format!("FOO=bar  rm {}", target.display());
    let stdin = envelope("Bash", realm.path(), &json!({ "command": command }));

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
}

/// Scenario 25: the JSON wire shape matches Claude Code's
/// `PreToolUse` hook contract verbatim — keys are `camelCase`,
/// decision is lowercase, `hookEventName` is exactly `"PreToolUse"`.
#[test]
fn decision_json_matches_claude_code_contract() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("secret/foo.md");
    let stdin = envelope(
        "Edit",
        realm.path(),
        &json!({
            "file_path": target,
            "old_string": "a",
            "new_string": "b",
        }),
    );

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    let inner = &payload["hookSpecificOutput"];
    assert_eq!(inner["hookEventName"], json!("PreToolUse"));
    assert_eq!(inner["permissionDecision"], json!("deny"));
    assert!(
        inner["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("mcp__remargin__edit")
    );
    // No extra top-level keys snuck in.
    let obj = payload.as_object().unwrap();
    assert_eq!(obj.len(), 1);
    assert!(obj.contains_key("hookSpecificOutput"));
}

/// Widened matcher: `MultiEdit` on a restricted `file_path` denies
/// end-to-end through the wired hook. `MultiEdit`'s path field is
/// `file_path`, same as `Edit`.
#[test]
fn multi_edit_against_restricted_realm_denies() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("secret/foo.md");
    let stdin = envelope(
        "MultiEdit",
        realm.path(),
        &json!({ "file_path": target, "edits": [] }),
    );

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
}

/// Widened matcher: `Grep` whose `path` is the restricted search root
/// denies end-to-end.
#[test]
fn grep_against_restricted_realm_denies() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("secret");
    let stdin = envelope(
        "Grep",
        realm.path(),
        &json!({ "pattern": "foo", "path": target }),
    );

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
}

/// Widened matcher: `Glob` whose `path` is the restricted search root
/// denies end-to-end.
#[test]
fn glob_against_restricted_realm_denies() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("secret");
    let stdin = envelope(
        "Glob",
        realm.path(),
        &json!({ "pattern": "**/*.md", "path": target }),
    );

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
}

/// Ancestor gap, end-to-end: a `Bash rm` of the realm root (a strict
/// ancestor of the trusted root `secret`) denies through the wired binary.
#[test]
fn bash_rm_realm_root_ancestor_denies() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let command = format!("rm -rf {}", realm.path().display());
    let stdin = envelope("Bash", realm.path(), &json!({ "command": command }));

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
}

/// Ancestor gap, end-to-end: an `ls` of the same realm root reads the
/// ancestor and stays allowed — exit 0 with empty stdout.
#[test]
fn bash_ls_realm_root_ancestor_silent_allows() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let command = format!("ls {}", realm.path().display());
    let stdin = envelope("Bash", realm.path(), &json!({ "command": command }));

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    assert!(
        out.stdout.is_empty(),
        "expected silent allow for a read of the ancestor realm root"
    );
}

/// Ancestor gap, end-to-end: a `Grep` whose search root is the realm root
/// sweeps the protected subtree and denies through the wired binary.
#[test]
fn grep_realm_root_ancestor_denies() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let stdin = envelope(
        "Grep",
        realm.path(),
        &json!({ "pattern": "foo", "path": realm.path().to_string_lossy() }),
    );

    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny")
    );
    let reason = payload["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("mcp__remargin__search"));
}

fn run_pretool_args(args: &[&str], cwd: &Path, home: &Path) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn pretool_install_local_writes_hook_to_project_settings() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let out = run_pretool_args(
        &["claude", "pretool", "install", "--local"],
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
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        "remargin claude pretool",
    );
}

#[test]
fn pretool_install_user_writes_hook_to_home_settings() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let out = run_pretool_args(&["claude", "pretool", "install"], realm.path(), home.path());
    assert!(
        out.status.success(),
        "install (user) failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let settings_path = home.path().join(".claude/settings.json");
    let body = fs::read_to_string(&settings_path).unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();
    assert!(value["hooks"]["PreToolUse"].is_array());
}

#[test]
fn pretool_install_is_idempotent() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    run_pretool_args(
        &["claude", "pretool", "install", "--local"],
        realm.path(),
        home.path(),
    );
    let second = run_pretool_args(
        &["claude", "pretool", "--json", "install", "--local"],
        realm.path(),
        home.path(),
    );
    assert!(second.status.success());
    let stdout = String::from_utf8(second.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(payload["status"].as_str().unwrap(), "already_installed");

    let body = fs::read_to_string(realm.path().join(".claude/settings.json")).unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
}

#[test]
fn pretool_test_reports_install_state() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let before = run_pretool_args(
        &["claude", "pretool", "--json", "test", "--local"],
        realm.path(),
        home.path(),
    );
    let before_json: Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before_json["status"].as_str().unwrap(), "not_installed");

    run_pretool_args(
        &["claude", "pretool", "install", "--local"],
        realm.path(),
        home.path(),
    );

    let after = run_pretool_args(
        &["claude", "pretool", "--json", "test", "--local"],
        realm.path(),
        home.path(),
    );
    let after_json: Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_eq!(after_json["status"].as_str().unwrap(), "installed");
}

#[test]
fn pretool_uninstall_removes_only_remargin_entry() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    // Pre-seed settings with a foreign PreToolUse entry alongside.
    let settings_dir = realm.path().join(".claude");
    fs::create_dir_all(&settings_dir).unwrap();
    let initial = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [
                        { "type": "command", "command": "other-tool" },
                    ],
                },
            ],
        },
    });
    fs::write(
        settings_dir.join("settings.json"),
        serde_json::to_string_pretty(&initial).unwrap(),
    )
    .unwrap();

    run_pretool_args(
        &["claude", "pretool", "install", "--local"],
        realm.path(),
        home.path(),
    );
    let out = run_pretool_args(
        &["claude", "pretool", "--json", "uninstall", "--local"],
        realm.path(),
        home.path(),
    );
    assert!(out.status.success());
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(payload["status"].as_str().unwrap(), "uninstalled");

    let body = fs::read_to_string(settings_dir.join("settings.json")).unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        "other-tool",
    );
}

#[test]
fn pretool_uninstall_is_idempotent_when_not_installed() {
    let realm = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let out = run_pretool_args(
        &["claude", "pretool", "--json", "uninstall", "--local"],
        realm.path(),
        home.path(),
    );
    assert!(out.status.success());
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(payload["status"].as_str().unwrap(), "not_installed");
}

#[test]
fn bare_pretool_still_runs_the_dispatcher() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("secret")).unwrap();
    let user_settings = realm.path().join("hermetic-user-settings.json");
    restrict_in(realm.path(), "secret", &user_settings);

    let target = realm.path().join("secret/foo.md");
    let stdin = envelope("Read", realm.path(), &json!({ "file_path": target }));

    // Same as run_pretool, but explicitly the no-subcommand form.
    let out = run_pretool(&stdin);
    assert_eq!(out.status.code(), Some(0_i32));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let payload: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        payload["hookSpecificOutput"]["permissionDecision"],
        json!("deny"),
    );
}
