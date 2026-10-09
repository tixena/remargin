//! Tests for the `PreToolUse` hook installer: install, uninstall, drift repair and the `test`
//! verdicts.

use std::path::{Path, PathBuf};

use os_shim::System;
use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use super::{
    HOOK_MATCHER, HOOK_SUBCOMMAND, InstallOutcome, LEGACY_HOOK_COMMAND, TestOutcome,
    UninstallOutcome, install, test, uninstall,
};

const EXE: &str = "/opt/bin/remargin";

fn settings_path() -> PathBuf {
    PathBuf::from("/home/u/.claude/settings.json")
}

/// A mock whose `current_exe` is the binary the installer must embed, and
/// which has that binary on disk so the entry reads as live.
fn mock() -> MemorySystem {
    MemorySystem::new()
        .with_current_exe(Path::new(EXE))
        .unwrap()
        .with_file(Path::new(EXE), b"binary")
        .unwrap()
}

/// The command a fresh install writes.
fn hook_command() -> String {
    format!("{EXE} {HOOK_SUBCOMMAND}")
}

fn read_json(system: &dyn System, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_json::from_str(&body).unwrap()
}

fn seed(system: MemorySystem, path: &Path, body: &str) -> MemorySystem {
    system.with_file(path, body.as_bytes()).unwrap()
}

/// Destructure a `Broken` outcome without a `panic!` (denied by clippy).
/// The `matches!` assert carries the diagnostic; the else arm is
/// unreachable.
fn expect_broken(outcome: TestOutcome) -> String {
    assert!(
        matches!(outcome, TestOutcome::Broken(_)),
        "expected Broken, got {outcome:?}",
    );
    let TestOutcome::Broken(reason) = outcome else {
        return String::new();
    };
    reason
}

#[test]
fn install_writes_hook_when_settings_missing() {
    let system = mock();
    let path = settings_path();

    let outcome = install(&system, &path).unwrap();
    assert_eq!(outcome, InstallOutcome::Installed);

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["matcher"].as_str().unwrap(), HOOK_MATCHER);
    let hooks_arr = entries[0]["hooks"].as_array().unwrap();
    assert_eq!(hooks_arr[0]["type"].as_str().unwrap(), "command");
    assert_eq!(hooks_arr[0]["command"].as_str().unwrap(), hook_command());
}

#[test]
fn install_is_idempotent_on_already_installed_entry() {
    let system = mock();
    let path = settings_path();

    assert_eq!(install(&system, &path).unwrap(), InstallOutcome::Installed);
    assert_eq!(
        install(&system, &path).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
}

#[test]
fn install_preserves_unrelated_top_level_keys() {
    let body = serde_json::to_string_pretty(&json!({
        "model": "claude-opus",
        "permissions": { "deny": ["Bash(rm *)"] },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    install(&system, &path).unwrap();

    let value = read_json(&system, &path);
    assert_eq!(value["model"].as_str().unwrap(), "claude-opus");
    assert_eq!(
        value["permissions"]["deny"].as_array().unwrap(),
        &vec![Value::String(String::from("Bash(rm *)"))],
    );
    assert!(value["hooks"]["PreToolUse"].is_array());
}

#[test]
fn install_preserves_unrelated_pretool_entries() {
    let body = serde_json::to_string_pretty(&json!({
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
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    install(&system, &path).unwrap();

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let has_other = entries
        .iter()
        .any(|e| e["hooks"][0]["command"].as_str() == Some("other-tool"));
    let has_remargin = entries
        .iter()
        .any(|e| e["hooks"][0]["command"].as_str() == Some(hook_command().as_str()));
    assert!(has_other);
    assert!(has_remargin);
}

#[test]
fn uninstall_removes_only_remargin_entry() {
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [
                        { "type": "command", "command": "other-tool" },
                    ],
                },
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": LEGACY_HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    let outcome = uninstall(&system, &path).unwrap();
    assert_eq!(outcome, UninstallOutcome::Uninstalled);

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        "other-tool"
    );
}

#[test]
fn uninstall_no_op_when_settings_file_missing() {
    let system = mock();
    let path = settings_path();
    let outcome = uninstall(&system, &path).unwrap();
    assert_eq!(outcome, UninstallOutcome::NotInstalled);
    let _read_err = system.read_to_string(&path).unwrap_err();
}

#[test]
fn uninstall_no_op_when_entry_absent() {
    let body = serde_json::to_string_pretty(&json!({ "model": "claude-opus" })).unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    let outcome = uninstall(&system, &path).unwrap();
    assert_eq!(outcome, UninstallOutcome::NotInstalled);
}

#[test]
fn uninstall_removes_empty_pretool_array_and_hooks_object() {
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": LEGACY_HOOK_COMMAND },
                    ],
                },
            ],
        },
        "model": "claude-opus",
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    uninstall(&system, &path).unwrap();

    let value = read_json(&system, &path);
    assert!(value.get("hooks").is_none());
    assert_eq!(value["model"].as_str().unwrap(), "claude-opus");
}

#[test]
fn test_reports_installed_when_entry_present() {
    let system = mock();
    let path = settings_path();
    install(&system, &path).unwrap();
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::Installed);
}

#[test]
fn test_reports_not_installed_when_file_missing() {
    let system = mock();
    let path = settings_path();
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::NotInstalled);
}

#[test]
fn test_reports_not_installed_when_entry_absent() {
    let body = serde_json::to_string_pretty(&json!({ "model": "claude-opus" })).unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::NotInstalled);
}

/// Detection keys on `HOOK_SUBCOMMAND`, so an entry with a stale matcher is upgraded in place.
#[test]
fn install_upgrades_drifted_matcher_in_place() {
    let stale_matcher = "Read|Write|Edit|Bash|NotebookEdit";
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": stale_matcher,
                    "hooks": [
                        { "type": "command", "command": LEGACY_HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    assert_eq!(
        test(&system, &path).unwrap(),
        TestOutcome::PathRelative(String::from(LEGACY_HOOK_COMMAND)),
    );

    assert_eq!(install(&system, &path).unwrap(), InstallOutcome::Installed);

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["matcher"].as_str().unwrap(), HOOK_MATCHER);
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        hook_command(),
    );

    assert_eq!(
        install(&system, &path).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );
}

/// A hook that cannot spawn is non-blocking to Claude Code, so it is reported as broken.
#[test]
fn test_reports_broken_when_binary_vanished() {
    // The binary `current_exe` names is never put on disk here.
    let system = MemorySystem::new()
        .with_current_exe(Path::new(EXE))
        .unwrap();
    let path = settings_path();
    install(&system, &path).unwrap();

    let reason = expect_broken(test(&system, &path).unwrap());
    assert!(
        reason.contains(EXE) && reason.contains("does not exist"),
        "fault should name the vanished binary: {reason}",
    );
}

#[test]
fn test_reports_path_relative_legacy_entry_without_rewriting_it() {
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": LEGACY_HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    assert_eq!(
        test(&system, &path).unwrap(),
        TestOutcome::PathRelative(String::from(LEGACY_HOOK_COMMAND)),
    );
    assert_eq!(system.read_to_string(&path).unwrap(), body);
}

/// A bare-name command and a stale absolute path are both rewritten in place.
#[test]
fn install_rewrites_drifted_command_in_place() {
    let stale = "/gone/remargin claude pretool";
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": stale },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    assert_eq!(install(&system, &path).unwrap(), InstallOutcome::Installed);

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        hook_command(),
    );
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::Installed);
}

#[test]
fn uninstall_removes_entry_with_drifted_matcher() {
    let stale_matcher = "Read|Write|Edit|Bash|NotebookEdit";
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": stale_matcher,
                    "hooks": [
                        { "type": "command", "command": LEGACY_HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(mock(), &path, &body);

    assert_eq!(
        uninstall(&system, &path).unwrap(),
        UninstallOutcome::Uninstalled,
    );
    let value = read_json(&system, &path);
    assert!(value.get("hooks").is_none());
}
