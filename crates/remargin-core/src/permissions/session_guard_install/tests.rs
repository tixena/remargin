use std::path::{Path, PathBuf};

use os_shim::System;
use os_shim::mock::MockSystem;
use serde_json::{Value, json};

use super::{
    InstallOutcome, SESSION_HOOK_COMMAND, TestOutcome, UninstallOutcome, install, test, uninstall,
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
fn install_writes_matcherless_hook_when_settings_missing() {
    let system = MockSystem::new();
    let path = settings_path();

    let outcome = install(&system, &path).unwrap();
    assert_eq!(outcome, InstallOutcome::Installed);

    let value = read_json(&system, &path);
    let entries = value["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    // No matcher — the guard fires for every SessionStart source.
    assert!(entries[0].get("matcher").is_none());
    let hooks_arr = entries[0]["hooks"].as_array().unwrap();
    assert_eq!(hooks_arr[0]["type"].as_str().unwrap(), "command");
    assert_eq!(
        hooks_arr[0]["command"].as_str().unwrap(),
        SESSION_HOOK_COMMAND
    );
}

/// Case 5: a second install over an already-present guard is a no-op.
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
    let entries = value["hooks"]["SessionStart"].as_array().unwrap();
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
    assert!(value["hooks"]["SessionStart"].is_array());
}

#[test]
fn install_preserves_unrelated_session_entries() {
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "SessionStart": [
                {
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
    let entries = value["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let has_other = entries
        .iter()
        .any(|e| e["hooks"][0]["command"].as_str() == Some("other-tool"));
    let has_remargin = entries
        .iter()
        .any(|e| e["hooks"][0]["command"].as_str() == Some(SESSION_HOOK_COMMAND));
    assert!(has_other);
    assert!(has_remargin);
}

#[test]
fn uninstall_removes_only_remargin_entry() {
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "SessionStart": [
                {
                    "hooks": [
                        { "type": "command", "command": "other-tool" },
                    ],
                },
                {
                    "hooks": [
                        { "type": "command", "command": SESSION_HOOK_COMMAND },
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
    let entries = value["hooks"]["SessionStart"].as_array().unwrap();
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
}

#[test]
fn uninstall_removes_empty_session_array_and_hooks_object() {
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "SessionStart": [
                {
                    "hooks": [
                        { "type": "command", "command": SESSION_HOOK_COMMAND },
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

/// A user-annotated guard entry (a `matcher` was added) is still
/// identified by its inner command — detection keys on the command, not
/// the matcher — so install is a no-op and uninstall removes it.
#[test]
fn entry_with_added_matcher_is_identified_by_command() {
    let body = serde_json::to_string_pretty(&json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "startup",
                    "hooks": [
                        { "type": "command", "command": SESSION_HOOK_COMMAND },
                    ],
                },
            ],
        },
    }))
    .unwrap();
    let path = settings_path();
    let system = seed(MockSystem::new(), &path, &body);

    assert_eq!(test(&system, &path).unwrap(), TestOutcome::Installed);
    assert_eq!(
        install(&system, &path).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );
    assert_eq!(
        uninstall(&system, &path).unwrap(),
        UninstallOutcome::Uninstalled,
    );
    let value = read_json(&system, &path);
    assert!(value.get("hooks").is_none());
}
