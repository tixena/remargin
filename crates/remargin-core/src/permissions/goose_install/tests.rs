//! Unit tests for `permissions::goose_install`: the install and uninstall lifecycle and the
//! three verdicts of `test`.

use std::path::{Path, PathBuf};

use os_shim::System;
use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use super::{
    HOOK_EVENT, HOOK_SUBCOMMAND, InstallOutcome, PLUGIN_NAME, SESSION_HOOK_EVENT,
    SESSION_HOOK_SUBCOMMAND, TestOutcome, UninstallOutcome, install, install_session_guard,
    plugin_dir, test, test_session_guard, uninstall, uninstall_session_guard,
};

const EXE: &str = "/opt/bin/remargin";

fn home() -> PathBuf {
    PathBuf::from("/home/u")
}

fn guard_dir() -> PathBuf {
    plugin_dir(&home())
}

/// A mock whose `current_exe` is the binary the installer must embed.
fn mock() -> MemorySystem {
    MemorySystem::new()
        .with_current_exe(Path::new(EXE))
        .unwrap()
        .with_file(Path::new(EXE), b"binary")
        .unwrap()
}

fn read_json(system: &dyn System, path: &Path) -> Value {
    serde_json::from_str(&system.read_to_string(path).unwrap()).unwrap()
}

fn hooks_json(system: &dyn System) -> Value {
    read_json(system, &guard_dir().join("hooks/hooks.json"))
}

fn seed(system: MemorySystem, path: &Path, body: &str) -> MemorySystem {
    system.with_file(path, body.as_bytes()).unwrap()
}

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
fn plugin_dir_lands_under_the_agents_plugins_root() {
    assert_eq!(
        guard_dir(),
        PathBuf::from("/home/u/.agents/plugins").join(PLUGIN_NAME),
    );
}

#[test]
fn install_writes_both_manifests() {
    let system = mock();
    assert_eq!(
        install(&system, &guard_dir()).unwrap(),
        InstallOutcome::Installed,
    );

    let manifest = read_json(&system, &guard_dir().join("plugin.json"));
    assert_eq!(manifest["name"].as_str().unwrap(), PLUGIN_NAME);
    assert!(manifest["version"].is_string());
    assert!(manifest["description"].is_string());

    let entries = hooks_json(&system)["hooks"][HOOK_EVENT]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(entries.len(), 1);
    let hook = entries[0]["hooks"][0].clone();
    assert_eq!(hook["type"].as_str().unwrap(), "command");
    assert!(hook["timeout"].is_number());
}

/// goose reads `matcher` as a regex and silently drops a bare `*`.
#[test]
fn generated_hook_entry_carries_no_matcher_key() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    let entry = hooks_json(&system)["hooks"][HOOK_EVENT][0].clone();
    assert!(
        entry.get("matcher").is_none(),
        "matcher key must be absent: {entry}",
    );
}

/// A `PATH` miss at spawn time fails open, so the command names the binary absolutely.
#[test]
fn generated_hook_command_is_the_absolute_binary_plus_subcommand() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    let command = hooks_json(&system)["hooks"][HOOK_EVENT][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(command, format!("{EXE} {HOOK_SUBCOMMAND}"));
    assert!(Path::new(EXE).is_absolute());
}

#[test]
fn install_is_idempotent() {
    let system = mock();
    assert_eq!(
        install(&system, &guard_dir()).unwrap(),
        InstallOutcome::Installed,
    );
    assert_eq!(
        install(&system, &guard_dir()).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );
}

#[test]
fn install_rewrites_a_drifted_hook_command() {
    let system = seed(
        mock(),
        &guard_dir().join("hooks/hooks.json"),
        &serde_json::to_string_pretty(&json!({
            "hooks": { HOOK_EVENT: [{ "hooks": [
                { "type": "command", "command": "/old/remargin goose pretool" },
            ] }] },
        }))
        .unwrap(),
    );
    assert_eq!(
        install(&system, &guard_dir()).unwrap(),
        InstallOutcome::Installed,
    );
    let command = hooks_json(&system)["hooks"][HOOK_EVENT][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(command, format!("{EXE} {HOOK_SUBCOMMAND}"));
}

#[test]
fn uninstall_removes_the_guard_and_preserves_sibling_plugins() {
    let sibling = home().join(".agents/plugins/other-plugin/plugin.json");
    let system = seed(mock(), &sibling, "{}");
    install(&system, &guard_dir()).unwrap();

    assert_eq!(
        uninstall(&system, &guard_dir()).unwrap(),
        UninstallOutcome::Uninstalled,
    );
    assert!(!system.exists(&guard_dir()).unwrap());
    assert!(system.exists(&sibling).unwrap());
}

#[test]
fn uninstall_is_a_no_op_when_absent() {
    let system = mock();
    assert_eq!(
        uninstall(&system, &guard_dir()).unwrap(),
        UninstallOutcome::NotInstalled,
    );
}

#[test]
fn test_reports_installed_when_wired() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    assert_eq!(test(&system, &guard_dir()).unwrap(), TestOutcome::Installed);
}

#[test]
fn test_reports_not_installed_when_absent() {
    let system = mock();
    assert_eq!(
        test(&system, &guard_dir()).unwrap(),
        TestOutcome::NotInstalled,
    );
}

/// Unparseable JSON, a manifest with no `PreToolUse` entry, and a command whose binary is gone.
#[test]
fn test_reports_broken_for_each_corrupt_shape() {
    let hooks_file = guard_dir().join("hooks/hooks.json");

    let unparseable = seed(mock(), &hooks_file, "{ not json");
    let parse_reason = expect_broken(test(&unparseable, &guard_dir()).unwrap());
    assert!(parse_reason.contains("JSON"), "reason: {parse_reason}");

    let no_entry = seed(mock(), &hooks_file, "{\"hooks\": {}}\n");
    let entry_reason = expect_broken(test(&no_entry, &guard_dir()).unwrap());
    assert!(entry_reason.contains(HOOK_EVENT), "reason: {entry_reason}");

    let missing_binary = mock();
    install(&missing_binary, &guard_dir()).unwrap();
    missing_binary.remove_file(Path::new(EXE)).unwrap();
    let binary_reason = expect_broken(test(&missing_binary, &guard_dir()).unwrap());
    assert!(
        binary_reason.contains(EXE),
        "reason should name the binary: {binary_reason}",
    );
}

fn session_entries(system: &dyn System) -> Vec<Value> {
    hooks_json(system)["hooks"][SESSION_HOOK_EVENT]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn pretool_entries(system: &dyn System) -> Vec<Value> {
    hooks_json(system)["hooks"][HOOK_EVENT]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn session_guard_install_adds_its_entry_beside_the_pretool_one() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    assert_eq!(
        install_session_guard(&system, &guard_dir()).unwrap(),
        InstallOutcome::Installed,
    );

    assert_eq!(pretool_entries(&system).len(), 1, "PreToolUse must survive");
    let entries = session_entries(&system);
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0].get("matcher").is_none(),
        "matcher key must be absent: {}",
        entries[0],
    );
    let hook = entries[0]["hooks"][0].clone();
    assert_eq!(hook["type"].as_str().unwrap(), "command");
    assert_eq!(
        hook["command"].as_str().unwrap(),
        format!("{EXE} {SESSION_HOOK_SUBCOMMAND}"),
    );
}

#[test]
fn session_guard_install_creates_the_plugin_when_absent() {
    let system = mock();
    assert_eq!(
        install_session_guard(&system, &guard_dir()).unwrap(),
        InstallOutcome::Installed,
    );
    assert!(system.exists(&guard_dir().join("plugin.json")).unwrap());
    assert_eq!(session_entries(&system).len(), 1);
    assert_eq!(
        install_session_guard(&system, &guard_dir()).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );
}

/// Both entries live in one manifest, so an install must merge, not rewrite.
#[test]
fn pretool_install_preserves_the_session_guard_entry() {
    let system = mock();
    install_session_guard(&system, &guard_dir()).unwrap();
    install(&system, &guard_dir()).unwrap();

    assert_eq!(
        session_entries(&system).len(),
        1,
        "SessionStart must survive"
    );
    assert_eq!(pretool_entries(&system).len(), 1);
}

/// An entry the user added under a managed event is not ours to remove.
#[test]
fn install_preserves_entries_it_does_not_own() {
    let system = seed(
        mock(),
        &guard_dir().join("hooks/hooks.json"),
        &serde_json::to_string_pretty(&json!({
            "hooks": {
                SESSION_HOOK_EVENT: [{ "hooks": [
                    { "type": "command", "command": "/usr/bin/notify-me" },
                ] }],
                "Stop": [{ "hooks": [
                    { "type": "command", "command": "/usr/bin/cleanup" },
                ] }],
            },
        }))
        .unwrap(),
    );
    install_session_guard(&system, &guard_dir()).unwrap();

    let entries = session_entries(&system);
    assert_eq!(
        entries.len(),
        2,
        "the user's entry must survive: {entries:?}"
    );
    assert!(
        hooks_json(&system)["hooks"]["Stop"].is_array(),
        "an unmanaged event must survive",
    );
}

#[test]
fn session_guard_uninstall_removes_only_its_own_entry() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    install_session_guard(&system, &guard_dir()).unwrap();

    assert_eq!(
        uninstall_session_guard(&system, &guard_dir()).unwrap(),
        UninstallOutcome::Uninstalled,
    );
    assert!(system.exists(&guard_dir()).unwrap(), "plugin must survive");
    assert_eq!(session_entries(&system), [] as [serde_json::Value; 0]);
    assert_eq!(pretool_entries(&system).len(), 1);
    assert_eq!(test(&system, &guard_dir()).unwrap(), TestOutcome::Installed);
}

#[test]
fn pretool_uninstall_leaves_the_session_guard_entry() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    install_session_guard(&system, &guard_dir()).unwrap();

    assert_eq!(
        uninstall(&system, &guard_dir()).unwrap(),
        UninstallOutcome::Uninstalled,
    );
    assert!(system.exists(&guard_dir()).unwrap(), "plugin must survive");
    assert_eq!(session_entries(&system).len(), 1);
}

#[test]
fn session_guard_uninstall_removes_the_plugin_when_it_was_the_last_entry() {
    let system = mock();
    install_session_guard(&system, &guard_dir()).unwrap();
    assert_eq!(
        uninstall_session_guard(&system, &guard_dir()).unwrap(),
        UninstallOutcome::Uninstalled,
    );
    assert!(!system.exists(&guard_dir()).unwrap());
}

#[test]
fn session_guard_uninstall_is_a_no_op_on_a_pretool_only_plugin() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    assert_eq!(
        uninstall_session_guard(&system, &guard_dir()).unwrap(),
        UninstallOutcome::NotInstalled,
    );
    assert_eq!(pretool_entries(&system).len(), 1);
}

#[test]
fn session_guard_test_reports_not_installed_for_a_pretool_only_plugin() {
    let system = mock();
    install(&system, &guard_dir()).unwrap();
    assert_eq!(
        test_session_guard(&system, &guard_dir()).unwrap(),
        TestOutcome::NotInstalled,
    );
}

#[test]
fn session_guard_test_reports_installed_when_wired() {
    let system = mock();
    install_session_guard(&system, &guard_dir()).unwrap();
    assert_eq!(
        test_session_guard(&system, &guard_dir()).unwrap(),
        TestOutcome::Installed,
    );
}

/// An unreadable manifest, and an entry whose binary is gone.
#[test]
fn session_guard_test_reports_broken_for_corrupt_shapes() {
    let unparseable = seed(mock(), &guard_dir().join("hooks/hooks.json"), "{ not json");
    let parse_reason = expect_broken(test_session_guard(&unparseable, &guard_dir()).unwrap());
    assert!(parse_reason.contains("JSON"), "reason: {parse_reason}");

    let missing_binary = mock();
    install_session_guard(&missing_binary, &guard_dir()).unwrap();
    missing_binary.remove_file(Path::new(EXE)).unwrap();
    let binary_reason = expect_broken(test_session_guard(&missing_binary, &guard_dir()).unwrap());
    assert!(
        binary_reason.contains(EXE),
        "reason should name the binary: {binary_reason}",
    );
}
