//! Install / uninstall / test the `PreToolUse` hook entry that
//! dispatches to `remargin claude pretool`. Operates over a single
//! settings file (caller picks user-scope or project-scope). Idempotent.
//!
//! The entry names the remargin binary by absolute path, resolved from the
//! running executable at install time: Claude Code treats a hook command it
//! cannot spawn as non-blocking, so a `PATH` miss is a gated tool call that
//! proceeds unguarded with no signal. An entry left by an install that
//! predates that — the bare [`LEGACY_HOOK_COMMAND`] — is still recognized
//! and still enforces while `PATH` resolves it; `test` reports it so the
//! user can reinstall. Rewriting it is `install`'s job alone: nothing else
//! touches a user's settings.

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::Result;
use os_shim::System;
use serde_json::{Map, Value, json};

use crate::permissions::hook_settings::{self, CommandState};

/// Every tool the dispatcher inspects must be listed, or Claude Code will not fan the hook in.
pub const HOOK_MATCHER: &str = "Read|Write|Edit|MultiEdit|NotebookEdit|Grep|Glob|Bash";

/// Appended to the absolute binary path in the generated command; also the entry's identity in
/// a settings file.
pub const HOOK_SUBCOMMAND: &str = "claude pretool";

/// The `PATH`-relative command installs wrote before they embedded the
/// binary path. Recognized and reported, never written.
pub const LEGACY_HOOK_COMMAND: &str = "remargin claude pretool";

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

/// Whether the `PreToolUse` hook entry is live.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TestOutcome {
    /// The entry is there but its command cannot spawn, so nothing is gated; carries the fault.
    Broken(String),
    Installed,
    NotInstalled,
    /// The entry enforces but names the binary by bare name ([`LEGACY_HOOK_COMMAND`]); carries the
    /// command as found.
    PathRelative(String),
}

/// # Errors
///
/// Returns an error if the running executable cannot be resolved (the hook
/// command would otherwise be written without an absolute path), if the
/// settings file is unreadable or contains invalid JSON, or if writing the
/// updated settings fails.
pub fn install(system: &dyn System, settings_file: &Path) -> Result<InstallOutcome> {
    let command = hook_settings::hook_command(system, HOOK_SUBCOMMAND)?;
    let mut value = hook_settings::load_or_default(system, settings_file)?;
    match upgrade_existing_entry(&mut value, &command) {
        // A remargin entry already carries the current matcher and command.
        Some(false) => Ok(InstallOutcome::AlreadyInstalled),
        // A remargin entry had drifted, now rewritten in place.
        Some(true) => {
            hook_settings::write_settings(system, settings_file, &value)?;
            Ok(InstallOutcome::Installed)
        }
        None => {
            insert_hook(&mut value, &command);
            hook_settings::write_settings(system, settings_file, &value)?;
            Ok(InstallOutcome::Installed)
        }
    }
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

/// Report whether the hook is wired: the entry is declared and the binary
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
    let state = hook_settings::command_state(system, declared_command(&value), HOOK_SUBCOMMAND)?;
    Ok(match state {
        CommandState::Absent => TestOutcome::NotInstalled,
        CommandState::Live => TestOutcome::Installed,
        CommandState::PathRelative(command) => TestOutcome::PathRelative(command),
        CommandState::StaleBinary(binary) => TestOutcome::Broken(format!(
            "the PreToolUse hook command in {} points at {binary}, which does not exist",
            settings_file.display()
        )),
    })
}

/// The command of the remargin entry, if the settings file declares one.
fn declared_command(value: &Value) -> Option<&str> {
    pretool_entries(value)?
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
    hook_settings::owns(command, HOOK_SUBCOMMAND)
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
    let pretool = hooks_obj
        .entry(String::from("PreToolUse"))
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(pretool_arr) = pretool.as_array_mut() else {
        return;
    };
    pretool_arr.push(json!({
        "matcher": HOOK_MATCHER,
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
    let Some(pretool) = hooks.get_mut("PreToolUse").and_then(Value::as_array_mut) else {
        return false;
    };
    let before = pretool.len();
    pretool.retain(|entry| !matches_remargin_entry(entry));
    let removed = pretool.len() < before;
    if pretool.is_empty() {
        let _removed_pretool: Option<Value> = hooks.remove("PreToolUse");
    }
    if hooks.is_empty() {
        let _removed_hooks: Option<Value> = root.remove("hooks");
    }
    removed
}

fn pretool_entries(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(Value::as_array)
}

/// Locate the remargin entry (identified by its [`HOOK_SUBCOMMAND`], not
/// its matcher) and reconcile both its matcher with [`HOOK_MATCHER`] and
/// its command with `command`. Returns `None` when no remargin entry is
/// present, `Some(false)` when both already match, and `Some(true)` after
/// rewriting drifted content in place — so a widened `HOOK_MATCHER` or a
/// binary that moved upgrades an older installation rather than
/// duplicating the entry.
fn upgrade_existing_entry(value: &mut Value, command: &str) -> Option<bool> {
    let entries = value
        .get_mut("hooks")
        .and_then(Value::as_object_mut)?
        .get_mut("PreToolUse")
        .and_then(Value::as_array_mut)?;
    let entry = entries
        .iter_mut()
        .find(|entry| matches_remargin_entry(entry))?;
    let obj = entry.as_object_mut()?;
    let matcher_current = obj
        .get("matcher")
        .and_then(Value::as_str)
        .is_some_and(|m| m == HOOK_MATCHER);
    if !matcher_current {
        let _prev: Option<Value> = obj.insert(
            String::from("matcher"),
            Value::String(String::from(HOOK_MATCHER)),
        );
    }
    let command_rewritten = rewrite_command(obj, command);
    Some(!matcher_current || command_rewritten)
}

/// Point the entry's remargin command at `command`, reporting whether that
/// changed anything. Only the command this module owns is touched: an
/// entry a user has extended with a second command of their own keeps it.
fn rewrite_command(entry: &mut Map<String, Value>, command: &str) -> bool {
    let Some(hooks_arr) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut rewritten = false;
    for hook in hooks_arr {
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

/// A remargin hook entry is identified by its inner [`HOOK_SUBCOMMAND`];
/// the matcher string is informational and the binary path ahead of the
/// subcommand varies with where remargin was installed from, so both a
/// drifted matcher and an entry written under another path are still
/// recognized as ours.
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
