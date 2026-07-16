//! Install / uninstall / test the `PreToolUse` hook entry that
//! dispatches to `remargin claude pretool`. Operates over a single
//! settings file (caller picks user-scope or project-scope). Idempotent.

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_json::{Map, Value, json};

/// Matcher string written into the `PreToolUse` hook entry. Every tool
/// the dispatcher inspects must be listed here so Claude Code fans the
/// hook in for those calls.
pub const HOOK_MATCHER: &str = "Read|Write|Edit|MultiEdit|NotebookEdit|Grep|Glob|Bash";

/// Hook command Claude Code invokes for each gated tool call. The
/// dispatcher reads stdin and writes the decision JSON to stdout.
pub const HOOK_COMMAND: &str = "remargin claude pretool";

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallOutcome {
    AlreadyInstalled,
    Installed,
}

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UninstallOutcome {
    NotInstalled,
    Uninstalled,
}

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TestOutcome {
    Installed,
    NotInstalled,
}

/// # Errors
///
/// Returns an error if the settings file is unreadable or contains
/// invalid JSON, or if writing the updated settings fails.
pub fn install(system: &dyn System, settings_file: &Path) -> Result<InstallOutcome> {
    let mut value = load_or_default(system, settings_file)?;
    match upgrade_existing_entry(&mut value) {
        // A remargin entry already carries the current matcher.
        Some(false) => Ok(InstallOutcome::AlreadyInstalled),
        // A remargin entry carried a drifted matcher, now rewritten.
        Some(true) => {
            write_settings(system, settings_file, &value)?;
            Ok(InstallOutcome::Installed)
        }
        None => {
            insert_hook(&mut value);
            write_settings(system, settings_file, &value)?;
            Ok(InstallOutcome::Installed)
        }
    }
}

/// # Errors
///
/// Returns an error if the settings file exists but contains invalid
/// JSON, or if writing the updated settings fails.
pub fn uninstall(system: &dyn System, settings_file: &Path) -> Result<UninstallOutcome> {
    if !system_path_exists(system, settings_file) {
        return Ok(UninstallOutcome::NotInstalled);
    }
    let mut value = load_or_default(system, settings_file)?;
    if !remove_hook(&mut value) {
        return Ok(UninstallOutcome::NotInstalled);
    }
    write_settings(system, settings_file, &value)?;
    Ok(UninstallOutcome::Uninstalled)
}

/// # Errors
///
/// Returns an error if the settings file exists but contains invalid
/// JSON.
pub fn test(system: &dyn System, settings_file: &Path) -> Result<TestOutcome> {
    if !system_path_exists(system, settings_file) {
        return Ok(TestOutcome::NotInstalled);
    }
    let value = load_or_default(system, settings_file)?;
    Ok(if hook_present(&value) {
        TestOutcome::Installed
    } else {
        TestOutcome::NotInstalled
    })
}

fn system_path_exists(system: &dyn System, path: &Path) -> bool {
    system.read_to_string(path).is_ok()
}

fn load_or_default(system: &dyn System, settings_file: &Path) -> Result<Value> {
    let body = system.read_to_string(settings_file).unwrap_or_default();
    if body.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(&body)
        .with_context(|| format!("parsing settings JSON at {}", settings_file.display()))
}

fn write_settings(system: &dyn System, settings_file: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = settings_file.parent() {
        system
            .create_dir_all(parent)
            .with_context(|| format!("creating settings directory {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(value).context("serializing settings JSON")?;
    let mut bytes = body.into_bytes();
    bytes.push(b'\n');
    system
        .write(settings_file, &bytes)
        .with_context(|| format!("writing settings to {}", settings_file.display()))
}

fn hook_present(value: &Value) -> bool {
    pretool_entries(value).is_some_and(|entries| entries.iter().any(matches_remargin_entry))
}

fn insert_hook(value: &mut Value) {
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
            { "type": "command", "command": HOOK_COMMAND },
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

/// Locate the remargin entry (identified by its [`HOOK_COMMAND`], not its
/// matcher) and reconcile its matcher with [`HOOK_MATCHER`]. Returns
/// `None` when no remargin entry is present, `Some(false)` when the
/// matcher already matches, and `Some(true)` after rewriting a drifted
/// matcher in place — so a widened `HOOK_MATCHER` upgrades an older
/// installation rather than duplicating the entry.
fn upgrade_existing_entry(value: &mut Value) -> Option<bool> {
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
    if matcher_current {
        return Some(false);
    }
    let _prev: Option<Value> = obj.insert(
        String::from("matcher"),
        Value::String(String::from(HOOK_MATCHER)),
    );
    Some(true)
}

/// A remargin hook entry is identified by its inner [`HOOK_COMMAND`]; the
/// matcher string is informational, so an installation whose matcher has
/// drifted from the current [`HOOK_MATCHER`] is still recognized.
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
            .is_some_and(|c| c == HOOK_COMMAND);
        type_ok && command_ok
    })
}
