use std::path::{Path, PathBuf};

use os_shim::System;
use os_shim::mock::MockSystem;
use serde_json::{Value, json};

use super::{
    HOOK_COMMAND, HOOK_MATCHER, InstallOutcome, TestOutcome, UninstallOutcome, install, test,
    uninstall,
};

fn settings_path() -> PathBuf {
    PathBuf::from("/home/u/.claude/settings.json")
}

fn read_json(system: &dyn System, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_json::from_str(&body).unwrap()
}

fn seed(system: MockSystem, path: &Path, body: &str) -> MockSystem {
    system.with_file(path, body.as_bytes()).unwrap()
}

#[test]
fn install_writes_hook_when_settings_missing() {
    let system = MockSystem::new();
    let path = settings_path();

    let outcome = install(&system, &path).unwrap();
    assert_eq!(outcome, InstallOutcome::Installed);

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["matcher"].as_str().unwrap(), HOOK_MATCHER);
    let hooks_arr = entries[0]["hooks"].as_array().unwrap();
    assert_eq!(hooks_arr[0]["type"].as_str().unwrap(), "command");
    assert_eq!(hooks_arr[0]["command"].as_str().unwrap(), HOOK_COMMAND);
}

#[test]
fn install_is_idempotent_on_already_installed_entry() {
    let system = MockSystem::new();
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
    let system = seed(MockSystem::new(), &path, &body);

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
    let system = seed(MockSystem::new(), &path, &body);

    install(&system, &path).unwrap();

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let has_other = entries
        .iter()
        .any(|e| e["hooks"][0]["command"].as_str() == Some("other-tool"));
    let has_remargin = entries
        .iter()
        .any(|e| e["hooks"][0]["command"].as_str() == Some(HOOK_COMMAND));
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
                        { "type": "command", "command": HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(MockSystem::new(), &path, &body);

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
    let system = MockSystem::new();
    let path = settings_path();
    let outcome = uninstall(&system, &path).unwrap();
    assert_eq!(outcome, UninstallOutcome::NotInstalled);
    let _read_err = system.read_to_string(&path).unwrap_err();
}

#[test]
fn uninstall_no_op_when_entry_absent() {
    let body = serde_json::to_string_pretty(&json!({ "model": "claude-opus" })).unwrap();
    let path = settings_path();
    let system = seed(MockSystem::new(), &path, &body);

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
                        { "type": "command", "command": HOOK_COMMAND },
                    ],
                },
            ],
        },
        "model": "claude-opus",
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(MockSystem::new(), &path, &body);

    uninstall(&system, &path).unwrap();

    let value = read_json(&system, &path);
    assert!(value.get("hooks").is_none());
    assert_eq!(value["model"].as_str().unwrap(), "claude-opus");
}

#[test]
fn test_reports_installed_when_entry_present() {
    let system = MockSystem::new();
    let path = settings_path();
    install(&system, &path).unwrap();
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::Installed);
}

#[test]
fn test_reports_not_installed_when_file_missing() {
    let system = MockSystem::new();
    let path = settings_path();
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::NotInstalled);
}

#[test]
fn test_reports_not_installed_when_entry_absent() {
    let body = serde_json::to_string_pretty(&json!({ "model": "claude-opus" })).unwrap();
    let path = settings_path();
    let system = seed(MockSystem::new(), &path, &body);
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::NotInstalled);
}

/// A remargin entry whose matcher has drifted from the current
/// `HOOK_MATCHER` (an older installation) is still detected as installed —
/// detection keys on `HOOK_COMMAND` — and `install` upgrades the matcher
/// in place without duplicating the entry.
#[test]
fn install_upgrades_drifted_matcher_in_place() {
    let stale_matcher = "Read|Write|Edit|Bash|NotebookEdit";
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": stale_matcher,
                    "hooks": [
                        { "type": "command", "command": HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(MockSystem::new(), &path, &body);

    // Detected as installed despite the stale matcher string.
    assert_eq!(test(&system, &path).unwrap(), TestOutcome::Installed);

    // Install rewrites the matcher in place and reports the write.
    assert_eq!(install(&system, &path).unwrap(), InstallOutcome::Installed);

    let value = read_json(&system, &path);
    let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["matcher"].as_str().unwrap(), HOOK_MATCHER);
    assert_eq!(
        entries[0]["hooks"][0]["command"].as_str().unwrap(),
        HOOK_COMMAND,
    );

    // A second install is now a no-op.
    assert_eq!(
        install(&system, &path).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );
}

/// `uninstall` removes a remargin entry even when its matcher has drifted
/// from the current `HOOK_MATCHER`.
#[test]
fn uninstall_removes_entry_with_drifted_matcher() {
    let stale_matcher = "Read|Write|Edit|Bash|NotebookEdit";
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": stale_matcher,
                    "hooks": [
                        { "type": "command", "command": HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(MockSystem::new(), &path, &body);

    assert_eq!(
        uninstall(&system, &path).unwrap(),
        UninstallOutcome::Uninstalled,
    );
    let value = read_json(&system, &path);
    assert!(value.get("hooks").is_none());
}
