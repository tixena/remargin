//! Shared JSON settings-file helpers for the Claude Code hook installers
//! ([`crate::permissions::pretool_install`],
//! [`crate::permissions::session_guard_install`]). Load-or-default the
//! settings object, write it back pretty-printed, and probe existence —
//! event-agnostic, so each installer only owns its own entry shape.

use std::path::Path;

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_json::{Map, Value};

/// `true` when the settings file is readable. Existence is proxied through
/// a successful read so the mock and real systems agree.
pub fn path_exists(system: &dyn System, path: &Path) -> bool {
    system.read_to_string(path).is_ok()
}

/// Parse the settings file into a JSON value, treating a missing or empty
/// file as an empty object.
///
/// # Errors
///
/// Returns an error when the file is present but not valid JSON.
pub fn load_or_default(system: &dyn System, settings_file: &Path) -> Result<Value> {
    let body = system.read_to_string(settings_file).unwrap_or_default();
    if body.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(&body)
        .with_context(|| format!("parsing settings JSON at {}", settings_file.display()))
}

/// Write `value` back to the settings file, creating parent directories
/// and terminating with a trailing newline.
///
/// # Errors
///
/// Returns an error when the parent directory or the file cannot be
/// written.
pub fn write_settings(system: &dyn System, settings_file: &Path, value: &Value) -> Result<()> {
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
