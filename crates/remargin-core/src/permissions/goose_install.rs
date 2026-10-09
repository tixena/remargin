//! Lifecycle of the goose hook plugin that dispatches to `remargin goose
//! pretool` and `remargin goose session-guard`.
//!
//! Operates over a plugin *directory* (caller picks user-scope or
//! project-scope) rather than a settings file: goose discovers plugins by
//! their presence on disk, with no registration step. Idempotent.
//!
//! Both hook entries live in one `hooks.json`, so every write merges into
//! the manifest already on disk instead of rewriting it: installing either
//! entry must never take the other one down with it.
//!
//! Two properties of the generated `hooks.json` are load-bearing:
//!
//! - **No `matcher` key.** goose reads `matcher` as a regex; a bare `*` is
//!   invalid and the whole entry is dropped with only a log warning — a
//!   guard silently absent. Omitting the key matches every event of the
//!   type, which is what the guard wants.
//! - **An absolute binary path.** goose fails open when the hook command
//!   cannot be spawned, so a `PATH` miss at spawn time is an unguarded
//!   session with no signal. The path is resolved from the running
//!   executable at install time.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_json::{Map, Value, json};

/// Plugin directory name under `<root>/.agents/plugins/`.
pub const PLUGIN_NAME: &str = "remargin-guard";

/// The only goose hook event that can block a tool call.
pub const HOOK_EVENT: &str = "PreToolUse";

pub const HOOK_SUBCOMMAND: &str = "goose pretool";

/// The event the fail-open backstop fires on. It cannot block; it reports.
pub const SESSION_HOOK_EVENT: &str = "SessionStart";

pub const SESSION_HOOK_SUBCOMMAND: &str = "goose session-guard";

/// Generous, because goose abandoning the guard is a fail-open pass for the tool call.
const HOOK_TIMEOUT_SECS: u32 = 30;

const PLUGIN_MANIFEST: &str = "plugin.json";

const HOOKS_MANIFEST: &str = "hooks/hooks.json";

const PRETOOL_HOOK: HookSpec = HookSpec {
    event: HOOK_EVENT,
    subcommand: HOOK_SUBCOMMAND,
};

const SESSION_HOOK: HookSpec = HookSpec {
    event: SESSION_HOOK_EVENT,
    subcommand: SESSION_HOOK_SUBCOMMAND,
};

/// A plugin directory declaring none of these has no reason to exist.
const MANAGED_HOOKS: [HookSpec; 2] = [PRETOOL_HOOK, SESSION_HOOK];

/// What the plugin manifest says about one managed hook entry.
enum EntryState {
    /// The manifest parses but declares no entry for this hook.
    Absent,
    /// The entry is declared, but the binary its command names is gone.
    BinaryMissing(String),
    /// The plugin directory itself is not there.
    DirAbsent,
    /// The manifest cannot be read or parsed; carries the reason.
    ManifestUnusable(String),
    /// Declared, and the binary it names exists.
    Wired,
}

/// One managed hook entry: the goose event it fires on and the remargin
/// subcommand its generated command dispatches to. The subcommand doubles
/// as the entry's identity in a manifest — it is what distinguishes a
/// remargin entry from a user's own, and one remargin entry from the other.
#[derive(Clone, Copy)]
struct HookSpec {
    event: &'static str,
    subcommand: &'static str,
}

/// Whether an install changed anything on disk.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallOutcome {
    AlreadyInstalled,
    Installed,
}

/// The on-disk hook manifest, or the reason it says nothing.
enum ManifestState {
    Unusable(String),
    Usable(Value),
}

/// Whether a managed hook entry is live.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TestOutcome {
    /// The plugin directory is present but describes no live guard; carries the fault.
    Broken(String),
    Installed,
    NotInstalled,
}

/// Whether an uninstall found anything to remove.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UninstallOutcome {
    NotInstalled,
    Uninstalled,
}

/// The directory whose presence marks `root` as a goose plugin scope.
#[must_use]
pub fn agents_dir(root: &Path) -> PathBuf {
    root.join(".agents")
}

/// The guard's plugin directory under `root` — a home directory for
/// user scope, a project directory for `--local`.
#[must_use]
pub fn plugin_dir(root: &Path) -> PathBuf {
    agents_dir(root).join("plugins").join(PLUGIN_NAME)
}

/// Write the `PreToolUse` entry, rewriting drifted content in place.
///
/// # Errors
///
/// Returns an error when the running executable cannot be resolved (the
/// hook command would otherwise be written without an absolute path) or
/// when the plugin files cannot be written.
pub fn install(system: &dyn System, dir: &Path) -> Result<InstallOutcome> {
    install_hook(system, dir, PRETOOL_HOOK)
}

/// Write the `SessionStart` entry into the same plugin, creating the plugin
/// when the `PreToolUse` installer has not run.
///
/// # Errors
///
/// Returns an error when the running executable cannot be resolved (the
/// hook command would otherwise be written without an absolute path) or
/// when the plugin files cannot be written.
pub fn install_session_guard(system: &dyn System, dir: &Path) -> Result<InstallOutcome> {
    install_hook(system, dir, SESSION_HOOK)
}

/// Report whether the `PreToolUse` guard is wired: the directory is
/// present, the hook manifest parses, it declares the entry, and that
/// entry's binary exists on disk.
///
/// # Errors
///
/// Returns an error when a path existence probe fails.
pub fn test(system: &dyn System, dir: &Path) -> Result<TestOutcome> {
    let manifest_path = hooks_file(dir);
    Ok(match entry_state(system, dir, PRETOOL_HOOK)? {
        // The directory's presence says the guard was installed, so a missing `PreToolUse` entry is
        // drift, not a plain absence.
        EntryState::Absent => broken(&format!(
            "{} declares no {HOOK_EVENT} command entry",
            manifest_path.display()
        )),
        EntryState::BinaryMissing(binary) => broken(&format!(
            "the hook command in {} points at {binary}, which does not exist",
            manifest_path.display()
        )),
        EntryState::DirAbsent => TestOutcome::NotInstalled,
        EntryState::ManifestUnusable(reason) => broken(&reason),
        EntryState::Wired => TestOutcome::Installed,
    })
}

/// Report whether the `SessionStart` entry is wired.
///
/// # Errors
///
/// Returns an error when a path existence probe fails.
pub fn test_session_guard(system: &dyn System, dir: &Path) -> Result<TestOutcome> {
    Ok(match entry_state(system, dir, SESSION_HOOK)? {
        // The directory is shared with the `PreToolUse` guard, so one carrying only that entry is an
        // ordinary pretool-only install: an absence, not drift.
        EntryState::Absent | EntryState::DirAbsent => TestOutcome::NotInstalled,
        EntryState::BinaryMissing(binary) => broken(&format!(
            "the hook command in {} points at {binary}, which does not exist",
            hooks_file(dir).display()
        )),
        EntryState::ManifestUnusable(reason) => broken(&reason),
        EntryState::Wired => TestOutcome::Installed,
    })
}

/// Remove the `PreToolUse` entry, leaving sibling plugins alone.
///
/// # Errors
///
/// Returns an error when the plugin files cannot be rewritten or removed.
pub fn uninstall(system: &dyn System, dir: &Path) -> Result<UninstallOutcome> {
    uninstall_hook(system, dir, PRETOOL_HOOK)
}

/// Remove the `SessionStart` entry, leaving the `PreToolUse` entry and
/// sibling plugins alone.
///
/// # Errors
///
/// Returns an error when the plugin files cannot be rewritten or removed.
pub fn uninstall_session_guard(system: &dyn System, dir: &Path) -> Result<UninstallOutcome> {
    uninstall_hook(system, dir, SESSION_HOOK)
}

fn broken(reason: &str) -> TestOutcome {
    TestOutcome::Broken(String::from(reason))
}

fn child_array(parent: &Map<String, Value>, key: &str) -> Vec<Value> {
    parent
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn child_object(parent: &Map<String, Value>, key: &str) -> Map<String, Value> {
    parent
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The binary path out of a generated hook command — everything ahead of
/// the subcommand the installer appends. Splitting on the suffix rather
/// than on whitespace keeps a binary path containing spaces intact.
fn command_binary<'command>(command: &'command str, subcommand: &str) -> &'command str {
    command
        .strip_suffix(subcommand)
        .map_or(command, str::trim_end)
}

/// The command of the entry `hook` owns, if the manifest declares one.
fn declared_command(manifest: &Value, hook: HookSpec) -> Option<&str> {
    entries(manifest, hook.event)?
        .iter()
        .find_map(|entry| entry_commands(entry).find(|command| owns(command, hook.subcommand)))
}

/// `true` when the manifest declares no entry this lifecycle owns, for any
/// managed event — the plugin directory then has no reason to exist.
fn declares_no_managed_entry(manifest: &Value) -> bool {
    !MANAGED_HOOKS.iter().any(|hook| {
        entries(manifest, hook.event).is_some_and(|list| {
            list.iter()
                .any(|entry| declares_subcommand(entry, hook.subcommand))
        })
    })
}

fn declares_subcommand(entry: &Value, subcommand: &str) -> bool {
    entry_commands(entry).any(|command| owns(command, subcommand))
}

fn entries<'manifest>(manifest: &'manifest Value, event: &str) -> Option<&'manifest Vec<Value>> {
    manifest.get("hooks")?.get(event)?.as_array()
}

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

fn entry_json(command: &str) -> Value {
    json!({
        "hooks": [
            {
                "type": "command",
                "command": command,
                "timeout": HOOK_TIMEOUT_SECS,
            },
        ],
    })
}

fn entry_state(system: &dyn System, dir: &Path, hook: HookSpec) -> Result<EntryState> {
    if !system.exists(dir)? {
        return Ok(EntryState::DirAbsent);
    }
    let manifest = match load_manifest(system, dir) {
        ManifestState::Unusable(reason) => return Ok(EntryState::ManifestUnusable(reason)),
        ManifestState::Usable(value) => value,
    };
    let Some(command) = declared_command(&manifest, hook) else {
        return Ok(EntryState::Absent);
    };
    let binary = command_binary(command, hook.subcommand);
    if system.exists(Path::new(binary))? {
        Ok(EntryState::Wired)
    } else {
        Ok(EntryState::BinaryMissing(String::from(binary)))
    }
}

fn file_matches(system: &dyn System, path: &Path, body: &str) -> bool {
    system.read_to_string(path).is_ok_and(|found| found == body)
}

/// The hook command: the absolute path to the running remargin binary plus
/// the dispatch subcommand. Written unquoted so it reads the same whether
/// goose spawns it through a shell or splits it into argv.
fn hook_command(system: &dyn System, hook: HookSpec) -> Result<String> {
    let exe = system
        .current_exe()
        .context("resolving the remargin binary path for the goose hook command")?;
    Ok(format!("{} {}", exe.display(), hook.subcommand))
}

fn hooks_file(dir: &Path) -> PathBuf {
    dir.join(HOOKS_MANIFEST)
}

fn install_hook(system: &dyn System, dir: &Path, hook: HookSpec) -> Result<InstallOutcome> {
    let command = hook_command(system, hook)?;
    let plugin_body = render(&plugin_manifest())?;
    // An unreadable manifest carries nothing worth preserving, so install rewrites from scratch.
    let current = match load_manifest(system, dir) {
        ManifestState::Unusable(_) => Value::Object(Map::new()),
        ManifestState::Usable(value) => value,
    };
    let hooks_body = render(&with_entry(&current, hook, &command))?;
    let plugin_file = dir.join(PLUGIN_MANIFEST);
    let manifest_path = hooks_file(dir);

    if file_matches(system, &plugin_file, &plugin_body)
        && file_matches(system, &manifest_path, &hooks_body)
    {
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    write_file(system, &plugin_file, &plugin_body)?;
    write_file(system, &manifest_path, &hooks_body)?;
    Ok(InstallOutcome::Installed)
}

fn load_manifest(system: &dyn System, dir: &Path) -> ManifestState {
    let path = hooks_file(dir);
    let Ok(body) = system.read_to_string(&path) else {
        return ManifestState::Unusable(format!("{} is unreadable", path.display()));
    };
    match serde_json::from_str::<Value>(&body) {
        Ok(value) if value.is_object() => ManifestState::Usable(value),
        Ok(_) => ManifestState::Unusable(format!("{} is not a JSON object", path.display())),
        Err(err) => {
            ManifestState::Unusable(format!("{} is not valid JSON ({err})", path.display()))
        }
    }
}

/// `true` when `command` is one this lifecycle generated for `subcommand`.
/// Matched on the suffix rather than the whole string so an entry written
/// by an install under a different binary path is still recognized as ours
/// and rewritten in place instead of duplicated.
fn owns(command: &str, subcommand: &str) -> bool {
    command.trim_end().ends_with(subcommand)
}

fn plugin_manifest() -> Value {
    json!({
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "description":
            "Blocks goose file and shell tools on remargin-managed paths, redirects the agent to \
             the remargin MCP tools, and reports a broken guard at session start.",
    })
}

fn remove_plugin(system: &dyn System, dir: &Path) -> Result<()> {
    system
        .remove_dir_all(dir)
        .with_context(|| format!("removing goose plugin directory {}", dir.display()))
}

fn render(value: &Value) -> Result<String> {
    let mut body = serde_json::to_string_pretty(value).context("serializing goose plugin JSON")?;
    body.push('\n');
    Ok(body)
}

fn uninstall_hook(system: &dyn System, dir: &Path, hook: HookSpec) -> Result<UninstallOutcome> {
    if !system.exists(dir)? {
        return Ok(UninstallOutcome::NotInstalled);
    }
    let ManifestState::Usable(manifest) = load_manifest(system, dir) else {
        // An unreadable manifest declares no live entry, so nothing survives its removal.
        remove_plugin(system, dir)?;
        return Ok(UninstallOutcome::Uninstalled);
    };
    let (stripped, removed) = without_entry(&manifest, hook);
    if !removed {
        return Ok(UninstallOutcome::NotInstalled);
    }
    if declares_no_managed_entry(&stripped) {
        remove_plugin(system, dir)?;
    } else {
        write_file(system, &hooks_file(dir), &render(&stripped)?)?;
    }
    Ok(UninstallOutcome::Uninstalled)
}

/// `manifest` with `hook`'s canonical entry in place of any entry this
/// lifecycle already owns for that event. Every other event, and every
/// entry the user added, is carried through untouched.
fn with_entry(manifest: &Value, hook: HookSpec, command: &str) -> Value {
    let mut root = manifest.as_object().cloned().unwrap_or_default();
    let mut hooks = child_object(&root, "hooks");
    let mut list = child_array(&hooks, hook.event);
    list.retain(|entry| !declares_subcommand(entry, hook.subcommand));
    list.push(entry_json(command));
    let _replaced_event: Option<Value> = hooks.insert(String::from(hook.event), Value::Array(list));
    let _replaced_hooks: Option<Value> = root.insert(String::from("hooks"), Value::Object(hooks));
    Value::Object(root)
}

/// `manifest` without `hook`'s entry, plus whether one was there to remove.
fn without_entry(manifest: &Value, hook: HookSpec) -> (Value, bool) {
    let mut root = manifest.as_object().cloned().unwrap_or_default();
    let mut hooks = child_object(&root, "hooks");
    let mut list = child_array(&hooks, hook.event);
    let before = list.len();
    list.retain(|entry| !declares_subcommand(entry, hook.subcommand));
    let removed = list.len() < before;
    if list.is_empty() {
        let _dropped_event: Option<Value> = hooks.remove(hook.event);
    } else {
        let _replaced_event: Option<Value> =
            hooks.insert(String::from(hook.event), Value::Array(list));
    }
    if hooks.is_empty() {
        let _dropped_hooks: Option<Value> = root.remove("hooks");
    } else {
        let _replaced_hooks: Option<Value> =
            root.insert(String::from("hooks"), Value::Object(hooks));
    }
    (Value::Object(root), removed)
}

fn write_file(system: &dyn System, path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        system
            .create_dir_all(parent)
            .with_context(|| format!("creating goose plugin directory {}", parent.display()))?;
    }
    system
        .write(path, body.as_bytes())
        .with_context(|| format!("writing goose plugin file {}", path.display()))
}
