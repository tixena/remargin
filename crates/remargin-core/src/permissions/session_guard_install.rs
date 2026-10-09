//! Install / uninstall / test the `SessionStart` guard hook entry.
//!
//! Dispatches to `remargin claude session-guard`. Operates over a single
//! settings file (caller picks user-scope or project-scope). Idempotent.
//!
//! The entry carries no `matcher`, so it fires for every `SessionStart`
//! source (startup, resume, clear, compact). The remargin entry is
//! identified by its inner subcommand — not by any matcher, and not by the
//! binary path ahead of it — so an installation a user has annotated with a
//! matcher, or one written under another binary path, is still recognized
//! (drift-tolerant, mirroring `pretool_install`).
//!
//! The entry names the remargin binary by absolute path for the same reason
//! the `PreToolUse` entry does: a command Claude Code cannot spawn exits 127
//! and is treated as non-blocking, and a backstop that silently never runs
//! is worse than none. An entry left by an install that predates that — the
//! bare [`LEGACY_SESSION_HOOK_COMMAND`] — is still recognized and still
//! runs while `PATH` resolves it.

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::Result;
use os_shim::System;
use serde_json::{Map, Value, json};

use crate::permissions::hook_settings::{self, CommandState};

/// Appended to the absolute binary path in the generated command; also the entry's identity in
/// a settings file.
pub const SESSION_HOOK_SUBCOMMAND: &str = "claude session-guard";

/// The `PATH`-relative command installs wrote before they embedded the
/// binary path. Recognized and reported, never written.
pub const LEGACY_SESSION_HOOK_COMMAND: &str = "remargin claude session-guard";

/// Whether an install changed the settings file.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallOutcome {
    AlreadyInstalled,
    Installed,
}

/// Whether an uninstall found an entry to remove.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UninstallOutcome {
    NotInstalled,
    Uninstalled,
}

/// Whether the `SessionStart` guard entry is live.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TestOutcome {
    /// The entry is there but its command cannot spawn, so no backstop runs; carries the fault.
    Broken(String),
    Installed,
    NotInstalled,
    /// The entry runs but names the binary by bare name ([`LEGACY_SESSION_HOOK_COMMAND`]); carries
    /// the command as found.
    PathRelative(String),
}

/// # Errors
///
/// Returns an error if the running executable cannot be resolved (the hook
/// command would otherwise be written without an absolute path), if the
/// settings file is unreadable or contains invalid JSON, or if writing the
/// updated settings fails.
pub fn install(system: &dyn System, settings_file: &Path) -> Result<InstallOutcome> {
    let command = hook_settings::hook_command(system, SESSION_HOOK_SUBCOMMAND)?;
    let mut value = hook_settings::load_or_default(system, settings_file)?;
    // A drifted command is rewritten in place, not duplicated: install is the one sanctioned path
    // for rewriting a user's settings.
    if rewrite_existing_entry(&mut value, &command) {
        hook_settings::write_settings(system, settings_file, &value)?;
        return Ok(InstallOutcome::Installed);
    }
    if hook_present(&value) {
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    insert_hook(&mut value, &command);
    hook_settings::write_settings(system, settings_file, &value)?;
    Ok(InstallOutcome::Installed)
}

/// # Errors
///
/// Returns an error if the settings file exists but contains invalid
/// JSON, or if writing the updated settings fails.
pub fn uninstall(system: &dyn System, settings_file: &Path) -> Result<UninstallOutcome> {
    if !hook_settings::path_exists(system, settings_file) {
        return Ok(UninstallOutcome::NotInstalled);
    }
    let mut value = hook_settings::load_or_default(system, settings_file)?;
    if !remove_hook(&mut value) {
        return Ok(UninstallOutcome::NotInstalled);
    }
    hook_settings::write_settings(system, settings_file, &value)?;
    Ok(UninstallOutcome::Uninstalled)
}

/// Report whether the guard is wired: the entry is declared and the binary
/// its command names can be spawned.
///
/// # Errors
///
/// Returns an error if the settings file exists but contains invalid JSON,
/// or if the binary existence probe fails.
pub fn test(system: &dyn System, settings_file: &Path) -> Result<TestOutcome> {
    if !hook_settings::path_exists(system, settings_file) {
        return Ok(TestOutcome::NotInstalled);
    }
    let value = hook_settings::load_or_default(system, settings_file)?;
    let state =
        hook_settings::command_state(system, declared_command(&value), SESSION_HOOK_SUBCOMMAND)?;
    Ok(match state {
        CommandState::Absent => TestOutcome::NotInstalled,
        CommandState::Live => TestOutcome::Installed,
        CommandState::PathRelative(command) => TestOutcome::PathRelative(command),
        CommandState::StaleBinary(binary) => TestOutcome::Broken(format!(
            "the SessionStart guard command in {} points at {binary}, which does not exist",
            settings_file.display()
        )),
    })
}

/// The command of the remargin entry, if the settings file declares one.
fn declared_command(value: &Value) -> Option<&str> {
    session_entries(value)?
        .iter()
        .find_map(|entry| entry_commands(entry).find(|command| owns(command)))
}

/// Every command an entry's inner `hooks` array declares.
fn entry_commands(entry: &Value) -> impl Iterator<Item = &str> {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|hook| hook.get("type").and_then(Value::as_str) == Some("command"))
        .filter_map(|hook| hook.get("command").and_then(Value::as_str))
}

fn owns(command: &str) -> bool {
    hook_settings::owns(command, SESSION_HOOK_SUBCOMMAND)
}

fn hook_present(value: &Value) -> bool {
    session_entries(value).is_some_and(|entries| entries.iter().any(matches_remargin_entry))
}

/// Point the declared remargin command at `command`, reporting whether that
/// changed anything. Only the command this module owns is touched: an entry
/// a user has extended with a second command of their own keeps it.
fn rewrite_existing_entry(value: &mut Value, command: &str) -> bool {
    let Some(entries) = value
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .and_then(|hooks| hooks.get_mut("SessionStart"))
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    let mut rewritten = false;
    for hook in entries
        .iter_mut()
        .filter_map(|entry| entry.get_mut("hooks").and_then(Value::as_array_mut))
        .flatten()
    {
        let Some(hook_obj) = hook.as_object_mut() else {
            continue;
        };
        if hook_obj.get("type").and_then(Value::as_str) != Some("command") {
            continue;
        }
        let found = hook_obj.get("command").and_then(Value::as_str);
        if found.is_some_and(owns) && found != Some(command) {
            let _prev: Option<Value> = hook_obj.insert(
                String::from("command"),
                Value::String(String::from(command)),
            );
            rewritten = true;
        }
    }
    rewritten
}

fn insert_hook(value: &mut Value, command: &str) {
    let Some(root) = value.as_object_mut() else {
        return;
    };
    let hooks = root
        .entry(String::from("hooks"))
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };
    let session = hooks_obj
        .entry(String::from("SessionStart"))
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(session_arr) = session.as_array_mut() else {
        return;
    };
    session_arr.push(json!({
        "hooks": [
            { "type": "command", "command": command },
        ],
    }));
}

fn remove_hook(value: &mut Value) -> bool {
    let Some(root) = value.as_object_mut() else {
        return false;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let Some(session) = hooks.get_mut("SessionStart").and_then(Value::as_array_mut) else {
        return false;
    };
    let before = session.len();
    session.retain(|entry| !matches_remargin_entry(entry));
    let removed = session.len() < before;
    if session.is_empty() {
        let _removed_session: Option<Value> = hooks.remove("SessionStart");
    }
    if hooks.is_empty() {
        let _removed_hooks: Option<Value> = root.remove("hooks");
    }
    removed
}

fn session_entries(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(Value::as_array)
}

/// A remargin guard entry is identified solely by its inner
/// [`SESSION_HOOK_SUBCOMMAND`]; any `matcher` a user has added is
/// informational and the binary path ahead of the subcommand varies with
/// where remargin was installed from, so both an annotated installation and
/// one written under another path are still recognized.
fn matches_remargin_entry(entry: &Value) -> bool {
    let Some(obj) = entry.as_object() else {
        return false;
    };
    let Some(hooks_arr) = obj.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    hooks_arr.iter().any(|hook| {
        let Some(hook_obj) = hook.as_object() else {
            return false;
        };
        let type_ok = hook_obj
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t == "command");
        let command_ok = hook_obj
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(owns);
        type_ok && command_ok
    })
}
