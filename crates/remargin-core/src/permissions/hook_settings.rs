//! Shared settings-file and hook-command helpers for the Claude Code hook
//! installers ([`crate::permissions::pretool_install`],
//! [`crate::permissions::session_guard_install`]). Load-or-default the
//! settings object, write it back pretty-printed, probe existence, and
//! build / classify the command an entry runs — event-agnostic, so each
//! installer only owns its own entry shape.
//!
//! One property of a generated command is load-bearing: it names the
//! remargin binary by **absolute path**. Claude Code treats a hook command
//! it cannot spawn as a non-blocking failure (exit 127), so a `PATH` miss
//! at spawn time is an unguarded tool call with no signal. The path is
//! resolved from the running executable at install time, as goose's
//! installers resolve theirs.

use std::path::Path;

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_json::{Map, Value};

/// What a settings file says about one installer's hook command.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandState {
    Absent,
    /// Declared with an absolute path, and that binary is on disk.
    Live,
    /// Declared, but naming the binary by bare name, so it runs only while `PATH` resolves it.
    /// Carries the command as found.
    PathRelative(String),
    /// Declared with an absolute path that is gone from disk, so the hook cannot spawn; carries it.
    StaleBinary(String),
}

/// The binary path out of a hook command — everything ahead of the
/// subcommand the installer appends. Splitting on the suffix rather than on
/// whitespace keeps a binary path containing spaces intact.
#[must_use]
pub fn command_binary<'command>(command: &'command str, subcommand: &str) -> &'command str {
    command
        .strip_suffix(subcommand)
        .map_or(command, str::trim_end)
}

/// Classify the `command` an entry declares for `subcommand`, `None` being
/// an entry no settings file declares.
///
/// # Errors
///
/// Returns an error when the binary existence probe fails.
pub fn command_state(
    system: &dyn System,
    command: Option<&str>,
    subcommand: &str,
) -> Result<CommandState> {
    let Some(declared) = command else {
        return Ok(CommandState::Absent);
    };
    let binary = command_binary(declared, subcommand);
    if !Path::new(binary).is_absolute() {
        return Ok(CommandState::PathRelative(String::from(declared)));
    }
    if system.exists(Path::new(binary))? {
        Ok(CommandState::Live)
    } else {
        Ok(CommandState::StaleBinary(String::from(binary)))
    }
}

/// The hook command an installer writes: the absolute path to the running
/// remargin binary plus its dispatch `subcommand`. Written unquoted so it
/// reads the same whether Claude Code spawns it through a shell or splits
/// it into argv.
///
/// # Errors
///
/// Returns an error when the running executable cannot be resolved — the
/// command would otherwise be written without an absolute path.
pub fn hook_command(system: &dyn System, subcommand: &str) -> Result<String> {
    let exe = system.current_exe().with_context(|| {
        format!("resolving the remargin binary path for the `{subcommand}` hook command")
    })?;
    Ok(format!("{} {subcommand}", exe.display()))
}

/// `true` when `command` is one an installer generated for `subcommand`.
/// Matched on the suffix rather than the whole string so an entry written
/// under a different binary path — or by an install predating the absolute
/// path — is still recognized as ours and rewritten in place instead of
/// duplicated.
#[must_use]
pub fn owns(command: &str, subcommand: &str) -> bool {
    command.trim_end().ends_with(subcommand)
}

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
