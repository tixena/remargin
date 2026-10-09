//! Lifecycle of remargin's registration as a goose MCP extension.
//!
//! The goose guard blocks native tools on managed paths and redirects the
//! agent to remargin's MCP ops. That redirect is only a redirect when the
//! session actually has those ops; registering them is what this module
//! does. Without it the guard's deny message names tools the session never
//! received — a dead end rather than a detour.
//!
//! Four properties of the generated entry are load-bearing:
//!
//! - **`name: remargin`.** goose namespaces an extension's tools as
//!   `<name>__<tool>`, taken from this field and *not* from the entry's key
//!   in the `extensions` mapping. Another name still loads the server, but
//!   every tool then arrives as `<other>__get` and the guard's `remargin__`
//!   allow-prefix stops matching — the guard would block its own redirect
//!   target.
//! - **An absolute `cmd`.** goose warns and continues when it cannot spawn
//!   an extension, so a `PATH` miss at spawn time is a session that reaches
//!   the deny message carrying none of the tools it names. The path is
//!   resolved from the running executable at install time.
//! - **Whole-file care.** goose reads its provider, its model, and every
//!   other extension out of this same file. A `config.yaml` it cannot parse
//!   costs the user their entire goose setup rather than just remargin —
//!   goose refuses to start a session at all. So a config that does not
//!   parse is an error here, never something to overwrite, and every write
//!   lands through a temp file and a rename.
//! - **Idempotence without churn.** An entry already matching the canonical
//!   shape leaves the file untouched, and a write that is needed edits only
//!   the lines of remargin's own entry, so a hand-maintained config keeps
//!   its comments and its layout either way. A layout the line editor
//!   declines re-serializes the document instead, and the outcome reports
//!   that so the caller can say the formatting went.
//!
//! Scope: goose discovers exactly one config file,
//! `$XDG_CONFIG_HOME/goose/config.yaml` (`~/.config/goose/config.yaml` by
//! default). There is no project-scoped config — a file beside a project is
//! read only when [`ADDITIONAL_CONFIG_ENV`] names it — so `--local` writes
//! a file the user must point that variable at, and the CLI says so rather
//! than implying a scope goose would discover on its own.

mod config_splice;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_yaml::{Mapping, Value};

use self::config_splice::Edit;

/// goose discovers no project-scoped config on its own, so a `--local` install reaches a
/// session only through this variable.
pub const ADDITIONAL_CONFIG_ENV: &str = "GOOSE_ADDITIONAL_CONFIG_FILES";

pub const EXTENSION_ARG: &str = "mcp";

/// Identity for install and uninstall; the tool prefix comes from [`EXTENSION_NAME`], not this.
pub const EXTENSION_KEY: &str = "remargin";

/// goose builds every tool name as `<name>__<tool>`, so the guard's `remargin__` allow-prefix
/// matches on this.
pub const EXTENSION_NAME: &str = "remargin";

const EXTENSIONS_KEY: &str = "extensions";

const CONFIG_DIR: &str = "goose";

const CONFIG_FILE: &str = "config.yaml";

const EXTENSION_DESCRIPTION: &str = "Read, write, and comment on remargin-managed markdown. Use \
                                     these instead of shell/edit/write tools for any managed .md \
                                     file.";

const EXTENSION_TIMEOUT_SECS: u64 = 300;

/// Transport discriminant telling goose to spawn `cmd` and speak stdio.
const EXTENSION_TYPE: &str = "stdio";

/// Relative to the project root.
const LOCAL_CONFIG_DIR: &str = ".goose";

const TEMP_SUFFIX: &str = ".remargin-tmp";

/// Relative to `$HOME`; used when `XDG_CONFIG_HOME` is unset.
const XDG_CONFIG_FALLBACK: &str = ".config";

const XDG_CONFIG_HOME: &str = "XDG_CONFIG_HOME";

/// The config file as one probe found it.
enum ConfigState {
    Absent,
    /// Present but not a YAML mapping, so it must not be overwritten; carries the reason.
    Unusable(String),
    /// Carries the raw text alongside the parse: an edit is applied to
    /// those bytes, not to a re-serialization of the parse.
    Usable {
        body: String,
        mapping: Mapping,
    },
}

/// What the config says about remargin's entry.
enum EntryState {
    /// The config parses but declares no remargin entry.
    Absent,
    /// Declared and shaped correctly, but the binary `cmd` names is gone.
    CommandMissing(String),
    /// The config file is not there at all.
    ConfigAbsent,
    /// The config cannot be read as a mapping. Carries the reason.
    ConfigUnusable(String),
    /// Declared, but shaped so goose loads no remargin tools from it; carries the fault.
    Unusable(String),
    /// Declared, well-shaped, and the binary it names exists.
    Wired,
}

/// The bytes a write is about to land, and what producing them cost.
struct ConfigWrite {
    body: String,
    /// The body re-serializes the whole document, which loses the file's comments and layout.
    normalizes: bool,
}

/// Whether an install wrote the config.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallOutcome {
    AlreadyInstalled,
    /// `normalized_layout`: the write re-serialized the document, so its formatting is gone.
    Installed {
        normalized_layout: bool,
    },
}

/// Whether goose would load remargin's tools from a config.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TestOutcome {
    /// The entry is there but goose would load no remargin tools from it; carries the fault.
    Broken(String),
    Installed,
    NotInstalled,
}

/// Whether an uninstall found an entry to remove.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UninstallOutcome {
    NotInstalled,
    /// `normalized_layout`: the write re-serialized the document, so its formatting is gone.
    Uninstalled {
        normalized_layout: bool,
    },
}

/// The config file a `--local` install writes, under the project root.
/// goose reads it only when [`ADDITIONAL_CONFIG_ENV`] names it.
#[must_use]
pub fn local_config_file(root: &Path) -> PathBuf {
    root.join(LOCAL_CONFIG_DIR).join(CONFIG_FILE)
}

/// Write remargin's extension entry, rewriting a drifted one in place.
///
/// Every other extension, and every unrelated key in the file, is carried
/// through untouched: goose keeps its provider and model here too.
///
/// # Errors
///
/// Returns an error when the running executable cannot be resolved (the
/// entry would otherwise name a non-absolute command), when the config
/// exists but does not parse (overwriting it would cost the user their
/// whole goose setup), or when the file cannot be written.
pub fn install(system: &dyn System, path: &Path) -> Result<InstallOutcome> {
    let command = extension_command(system)?;
    let entry = canonical_entry(&command);
    let (original, mut config) = match load_config(system, path) {
        ConfigState::Absent => (String::new(), Mapping::new()),
        ConfigState::Unusable(reason) => {
            return Err(anyhow::anyhow!(
                "{reason}; refusing to overwrite it because goose reads its provider and every \
                 other extension from this file"
            ));
        }
        ConfigState::Usable { body, mapping } => (body, mapping),
    };

    // An entry already in canonical shape leaves the file alone entirely,
    // so reinstalling never reformats a config the user hand-maintains.
    if declared_entry(&config).is_some_and(|found| *found == entry) {
        return Ok(InstallOutcome::AlreadyInstalled);
    }

    let mut extensions = child_mapping(&config, EXTENSIONS_KEY);
    insert(&mut extensions, EXTENSION_KEY, entry.clone());
    insert(&mut config, EXTENSIONS_KEY, Value::Mapping(extensions));
    let write = config_body(&original, Edit::Set(&entry), &config)?;
    write_config(system, path, &write.body)?;
    Ok(InstallOutcome::Installed {
        normalized_layout: write.normalizes,
    })
}

/// Report whether goose would load remargin's tools from this config.
///
/// # Errors
///
/// Returns an error when a path existence probe fails.
pub fn test(system: &dyn System, path: &Path) -> Result<TestOutcome> {
    Ok(match entry_state(system, path)? {
        EntryState::Absent | EntryState::ConfigAbsent => TestOutcome::NotInstalled,
        EntryState::CommandMissing(command) => TestOutcome::Broken(format!(
            "the extension entry in {} runs {command}, which does not exist, so goose warns and \
             starts the session without any remargin tools",
            path.display()
        )),
        EntryState::ConfigUnusable(reason) | EntryState::Unusable(reason) => {
            TestOutcome::Broken(reason)
        }
        EntryState::Wired => TestOutcome::Installed,
    })
}

/// Remove remargin's entry, leaving every sibling extension and every
/// unrelated key in the file alone.
///
/// # Errors
///
/// Returns an error when the config exists but does not parse (removing an
/// entry from it would mean rewriting a file this module cannot read), or
/// when the file cannot be written.
pub fn uninstall(system: &dyn System, path: &Path) -> Result<UninstallOutcome> {
    let (original, mut config) = match load_config(system, path) {
        ConfigState::Absent => return Ok(UninstallOutcome::NotInstalled),
        ConfigState::Unusable(reason) => {
            return Err(anyhow::anyhow!(
                "{reason}; refusing to rewrite it because goose reads its provider and every \
                 other extension from this file"
            ));
        }
        ConfigState::Usable { body, mapping } => (body, mapping),
    };
    if declared_entry(&config).is_none() {
        return Ok(UninstallOutcome::NotInstalled);
    }

    let mut extensions = child_mapping(&config, EXTENSIONS_KEY);
    let _removed: Option<Value> = extensions.remove(Value::from(EXTENSION_KEY));
    // The possibly-empty `extensions` mapping stays: uninstall changes exactly remargin's entry.
    insert(&mut config, EXTENSIONS_KEY, Value::Mapping(extensions));
    let write = config_body(&original, Edit::Remove, &config)?;
    write_config(system, path, &write.body)?;
    Ok(UninstallOutcome::Uninstalled {
        normalized_layout: write.normalizes,
    })
}

/// goose's config file for user scope: `$XDG_CONFIG_HOME/goose/config.yaml`,
/// falling back to `~/.config/goose/config.yaml`. This is the only path
/// goose discovers on its own.
#[must_use]
pub fn user_config_file(system: &dyn System, home: &Path) -> PathBuf {
    let config_home = system
        .env_var(XDG_CONFIG_HOME)
        .ok()
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join(XDG_CONFIG_FALLBACK), PathBuf::from);
    config_home.join(CONFIG_DIR).join(CONFIG_FILE)
}

/// The canonical entry for a remargin server started by `command`.
fn canonical_entry(command: &str) -> Value {
    let mut entry = Mapping::new();
    insert(&mut entry, "enabled", Value::Bool(true));
    insert(&mut entry, "type", Value::from(EXTENSION_TYPE));
    insert(&mut entry, "name", Value::from(EXTENSION_NAME));
    insert(
        &mut entry,
        "description",
        Value::from(EXTENSION_DESCRIPTION),
    );
    insert(&mut entry, "cmd", Value::from(command));
    insert(
        &mut entry,
        "args",
        Value::Sequence(vec![Value::from(EXTENSION_ARG)]),
    );
    insert(&mut entry, "timeout", Value::from(EXTENSION_TIMEOUT_SECS));
    insert(&mut entry, "env_keys", Value::Sequence(Vec::new()));
    insert(&mut entry, "envs", Value::Mapping(Mapping::new()));
    Value::Mapping(entry)
}

/// The bytes a write lands: the original text with only remargin's own
/// lines edited when that edit reproduces `intended` exactly, and a full
/// re-serialization otherwise. Re-parsing the edited text is what makes
/// the line editor safe to trust — an edit landing anywhere but where it
/// was meant to never reaches the file.
///
/// The fallback is the one place a write costs the file its formatting,
/// so it is also the one place that can report it.
///
/// # Errors
///
/// Returns an error when the config cannot be serialized.
fn config_body(original: &str, edit: Edit<'_>, intended: &Mapping) -> Result<ConfigWrite> {
    if let Some(edited) = config_splice::apply(original, edit) {
        let parsed = serde_yaml::from_str::<Value>(&edited).ok();
        if parsed.as_ref().and_then(Value::as_mapping) == Some(intended) {
            return Ok(ConfigWrite {
                body: edited,
                normalizes: false,
            });
        }
    }
    let body = serde_yaml::to_string(intended).context("serializing the goose config")?;
    Ok(ConfigWrite {
        body,
        normalizes: true,
    })
}

fn child_mapping(parent: &Mapping, key: &str) -> Mapping {
    parent
        .get(Value::from(key))
        .and_then(Value::as_mapping)
        .cloned()
        .unwrap_or_default()
}

/// remargin's entry in `config`, if one is declared.
fn declared_entry(config: &Mapping) -> Option<&Value> {
    config
        .get(Value::from(EXTENSIONS_KEY))?
        .as_mapping()?
        .get(Value::from(EXTENSION_KEY))
}

/// Why goose would load no remargin tools from `entry`, or `None` when its
/// shape is sound. The binary's existence is a separate probe; this is
/// shape alone.
fn entry_fault(entry: &Value, path: &Path) -> Option<String> {
    let file = path.display();
    let Some(mapping) = entry.as_mapping() else {
        return Some(format!("the extension entry in {file} is not a mapping"));
    };
    // Only an explicit `false` is a fault: goose's default for an absent
    // key is not something this module claims to know.
    if mapping.get(Value::from("enabled")) == Some(&Value::Bool(false)) {
        return Some(format!(
            "the extension entry in {file} is disabled (`enabled: false`), so goose starts the \
             session with no remargin tools"
        ));
    }
    match string_field(mapping, "type") {
        Some(found) if found == EXTENSION_TYPE => {}
        Some(found) => {
            return Some(format!(
                "the extension entry in {file} has type `{found}`, not `{EXTENSION_TYPE}`, so \
                 goose never spawns the remargin server"
            ));
        }
        None => {
            return Some(format!(
                "the extension entry in {file} declares no `type`, so goose cannot tell how to \
                 start the remargin server"
            ));
        }
    }
    match string_field(mapping, "name") {
        Some(found) if found == EXTENSION_NAME => {}
        Some(found) => {
            return Some(format!(
                "the extension entry in {file} is named `{found}`, so its tools arrive as \
                 `{found}__*` and the guard's `{EXTENSION_NAME}__` redirect target does not exist"
            ));
        }
        None => {
            return Some(format!(
                "the extension entry in {file} declares no `name`, which is what goose prefixes \
                 its tools with"
            ));
        }
    }
    if entry_command(mapping).is_none() {
        return Some(format!(
            "the extension entry in {file} declares no `cmd`, so goose drops it silently"
        ));
    }
    if !declares_arg(mapping) {
        return Some(format!(
            "the extension entry in {file} does not pass `{EXTENSION_ARG}` in `args`, so the \
             command it runs is not the remargin MCP server"
        ));
    }
    None
}

/// The `cmd` of an entry, when it names one that is not blank.
fn entry_command(mapping: &Mapping) -> Option<&str> {
    string_field(mapping, "cmd").filter(|command| !command.trim().is_empty())
}

fn entry_state(system: &dyn System, path: &Path) -> Result<EntryState> {
    let config = match load_config(system, path) {
        ConfigState::Absent => return Ok(EntryState::ConfigAbsent),
        ConfigState::Unusable(reason) => return Ok(EntryState::ConfigUnusable(reason)),
        ConfigState::Usable { mapping, body: _ } => mapping,
    };
    let Some(entry) = declared_entry(&config) else {
        return Ok(EntryState::Absent);
    };
    if let Some(fault) = entry_fault(entry, path) {
        return Ok(EntryState::Unusable(fault));
    }
    // `entry_fault` returning `None` established both of these.
    let command = entry
        .as_mapping()
        .and_then(entry_command)
        .unwrap_or_default();
    if system.exists(Path::new(command))? {
        Ok(EntryState::Wired)
    } else {
        Ok(EntryState::CommandMissing(String::from(command)))
    }
}

/// `true` when `args` carries the MCP subcommand.
fn declares_arg(mapping: &Mapping) -> bool {
    mapping
        .get(Value::from("args"))
        .and_then(Value::as_sequence)
        .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some(EXTENSION_ARG)))
}

/// The absolute command the entry runs. goose fails open on a spawn it
/// cannot make, so the path is resolved here rather than left to `PATH`.
fn extension_command(system: &dyn System) -> Result<String> {
    let exe = system
        .current_exe()
        .context("resolving the remargin binary path for the goose extension entry")?;
    Ok(exe.display().to_string())
}

fn insert(map: &mut Mapping, key: &str, value: Value) {
    let _replaced: Option<Value> = map.insert(Value::from(key), value);
}

fn load_config(system: &dyn System, path: &Path) -> ConfigState {
    let Ok(body) = system.read_to_string(path) else {
        return ConfigState::Absent;
    };
    match serde_yaml::from_str::<Value>(&body) {
        // goose writes an empty file before its first `configure`, and an empty YAML document is a
        // null: an absence to fill, not a fault.
        Ok(Value::Null) => ConfigState::Usable {
            body,
            mapping: Mapping::new(),
        },
        Ok(Value::Mapping(mapping)) => ConfigState::Usable { body, mapping },
        Ok(_) => ConfigState::Unusable(format!("{} is not a YAML mapping", path.display())),
        Err(err) => ConfigState::Unusable(format!("{} is not valid YAML ({err})", path.display())),
    }
}

/// A string field of an entry, when it is one.
fn string_field<'entry>(mapping: &'entry Mapping, key: &str) -> Option<&'entry str> {
    mapping.get(Value::from(key)).and_then(Value::as_str)
}

/// Replace `path` through a sibling temp file and a rename, so an
/// interrupted write can never leave goose a half-file. goose reads its
/// provider from here: a config it cannot parse costs the user every
/// session, not just remargin's tools.
fn write_config(system: &dyn System, path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        system
            .create_dir_all(parent)
            .with_context(|| format!("creating goose config directory {}", parent.display()))?;
    }
    let temp = temp_path(path);
    system
        .write(&temp, body.as_bytes())
        .with_context(|| format!("writing goose config {}", temp.display()))?;
    system
        .rename(&temp, path)
        .with_context(|| format!("replacing goose config {}", path.display()))
}

/// The sibling `path` is staged in — same directory, so the rename that
/// follows stays on one filesystem and is atomic.
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(TEMP_SUFFIX);
    path.with_file_name(name)
}
