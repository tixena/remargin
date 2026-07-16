//! Install / uninstall / test the `SessionStart` guard hook entry.
//!
//! Dispatches to `remargin claude session-guard`. Operates over a single
//! settings file (caller picks user-scope or project-scope). Idempotent.
//!
//! The entry carries no `matcher`, so it fires for every `SessionStart`
//! source (startup, resume, clear, compact). The remargin entry is
//! identified by its inner command — not by any matcher — so an
//! installation a user has annotated with a matcher is still recognized
//! (drift-tolerant, mirroring `pretool_install`).

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::Result;
use os_shim::System;
use serde_json::{Map, Value, json};

use crate::permissions::hook_settings;

/// Hook command Claude Code invokes at session start. The guard reads no
/// stdin; it re-verifies enforcement will be live and writes its
/// diagnostic JSON to stdout.
pub const SESSION_HOOK_COMMAND: &str = "remargin claude session-guard";

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
    let mut value = hook_settings::load_or_default(system, settings_file)?;
    if hook_present(&value) {
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    insert_hook(&mut value);
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

/// # Errors
///
/// Returns an error if the settings file exists but contains invalid
/// JSON.
pub fn test(system: &dyn System, settings_file: &Path) -> Result<TestOutcome> {
    if !hook_settings::path_exists(system, settings_file) {
        return Ok(TestOutcome::NotInstalled);
    }
    let value = hook_settings::load_or_default(system, settings_file)?;
    Ok(if hook_present(&value) {
        TestOutcome::Installed
    } else {
        TestOutcome::NotInstalled
    })
}

fn hook_present(value: &Value) -> bool {
    session_entries(value).is_some_and(|entries| entries.iter().any(matches_remargin_entry))
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
    let session = hooks_obj
        .entry(String::from("SessionStart"))
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(session_arr) = session.as_array_mut() else {
        return;
    };
    session_arr.push(json!({
        "hooks": [
            { "type": "command", "command": SESSION_HOOK_COMMAND },
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
/// [`SESSION_HOOK_COMMAND`]; any `matcher` a user has added is
/// informational, so an annotated installation is still recognized.
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
            .is_some_and(|c| c == SESSION_HOOK_COMMAND);
        type_ok && command_ok
    })
}
