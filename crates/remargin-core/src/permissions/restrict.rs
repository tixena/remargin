//! `remargin restrict` core (rem-yj1j.5 / T26, slice 1 — `rem-aqnn`).
//!
//! [`restrict`] is the public entry point: anchor discovery (walk up
//! to the nearest `.claude/` ancestor), path canonicalisation, an
//! in-place edit of `<anchor>/.remargin.yaml`, then a call into
//! [`crate::permissions::claude_sync::apply_rules`] to project the
//! new entry into Claude's settings + sidecar.
//!
//! ## YAML editing strategy
//!
//! Rather than round-trip through the strongly-typed
//! [`crate::config::permissions::Permissions`] struct (which would
//! drop unknown keys the user may have added under top-level YAML),
//! the editor parses the file as a `serde_yaml::Value`, mutates only
//! the `permissions.restrict` array, and writes the result back. That
//! preserves the rest of the user's `.remargin.yaml` shape verbatim
//! at the cost of dropping inline comments — acceptable per the spec
//! because `.remargin.yaml` is a config file, not a comment-managed
//! markdown document.
//!
//! ## rem-is4z bypass
//!
//! `rem-is4z` (the agent-side guard against writing `.remargin.yaml`
//! through the `write` / `edit` ops) intentionally does NOT cover this
//! module. `restrict` (and the future `unprotect`) is the explicit,
//! sanctioned write path: the user invokes it deliberately through a
//! dedicated command. The single helper [`write_remargin_yaml`] is
//! the ONLY way the bypass is exercised; keeping it scoped here makes
//! the audit boundary obvious.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use os_shim::System;
use serde_yaml::{Mapping, Value};

use crate::config::permissions::resolve::{ResolvedRestrict, RestrictPath};
use crate::permissions::claude_sync::{RuleSet, apply_rules, rules_for};

/// Wildcard literal accepted in `restrict.path`. Mirrors the schema
/// constant in [`crate::config::permissions`].
const RESTRICT_WILDCARD: &str = "*";

/// Caller-supplied parameters for [`restrict`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestrictArgs {
    /// Extra Bash commands to deny on the restricted path. The
    /// resolver records these on the on-disk entry; the rule
    /// generator emits one `Bash(<cmd> * //...)` deny per name.
    pub also_deny_bash: Vec<String>,
    /// When `true`, allow `Bash(remargin *)` on the path — useful
    /// when the caller wants the MCP locked down but still needs CLI
    /// access for ops like `permissions show`. Defaults to `false`.
    pub cli_allowed: bool,
    /// Subpath relative to the anchor, OR the literal `"*"` for
    /// realm-wide. Subpaths are realpath-canonicalised before being
    /// stored.
    pub path: String,
}

/// Description of what [`restrict`] mutated. Returned to the caller
/// (CLI prints a human summary; MCP returns the JSON form) so the
/// user can see exactly which files were touched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestrictOutcome {
    /// Canonical absolute restricted path. For the wildcard form,
    /// this is the anchor root.
    pub absolute_path: PathBuf,
    /// The directory holding `.claude/`, where `.remargin.yaml`
    /// lives.
    pub anchor: PathBuf,
    /// Settings files the rules were applied to.
    pub claude_files_touched: Vec<PathBuf>,
    /// Every rule string the synchronizer wrote to the settings
    /// files. Useful for verbose CLI output.
    pub rules_applied: Vec<String>,
    /// `true` when this call created `.remargin.yaml`. `false` when
    /// the file already existed and we appended / merged.
    pub yaml_was_created: bool,
}

/// Run the restrict command end-to-end.
///
/// 1. Walk up from `cwd` to find the nearest `.claude/` ancestor
///    (the anchor).
/// 2. Canonicalise `args.path` (or accept the wildcard).
/// 3. Append-or-merge the entry into `<anchor>/.remargin.yaml`,
///    creating the file if absent.
/// 4. Compute the rule set via [`rules_for`] and call [`apply_rules`]
///    to update both Claude settings files + the sidecar.
///
/// The function is idempotent: re-running with the same arguments
/// produces the same final state (no duplicate entries, no
/// duplicate rules, no extra sidecar records).
///
/// `settings_files` is supplied by the caller so the CLI can pass
/// the resolved project + user-scope paths and tests can pass a
/// hermetic in-mock pair. The function does NOT do `~` expansion;
/// the caller (rem-rdjy) is responsible for that.
///
/// # Errors
///
/// - No `.claude/` ancestor found.
/// - `args.path` resolves outside the anchor.
/// - I/O / parse failures from the YAML editor or the
///   Claude-settings synchronizer.
pub fn restrict(
    system: &dyn System,
    cwd: &Path,
    args: &RestrictArgs,
    settings_files: &[PathBuf],
) -> Result<RestrictOutcome> {
    let anchor = find_claude_anchor(system, cwd)
        .with_context(|| format!("looking for `.claude/` ancestor of {}", cwd.display()))?;

    let (absolute_path, on_disk_path) = if args.path == RESTRICT_WILDCARD {
        (anchor.clone(), String::from(RESTRICT_WILDCARD))
    } else {
        let candidate = anchor.join(&args.path);
        let lexically_normalised = lexical_normalise(&candidate);
        let absolute = system
            .canonicalize(&lexically_normalised)
            .unwrap_or(lexically_normalised);
        if !absolute.starts_with(&anchor) {
            bail!(
                "restrict path {:?} resolves to {} which is outside the anchor {}",
                args.path,
                absolute.display(),
                anchor.display()
            );
        }
        (absolute, args.path.clone())
    };

    let yaml_was_created = upsert_remargin_yaml(system, &anchor, &on_disk_path, args)?;

    let resolved = ResolvedRestrict {
        also_deny_bash: args.also_deny_bash.clone(),
        cli_allowed: args.cli_allowed,
        path: if args.path == RESTRICT_WILDCARD {
            RestrictPath::Wildcard {
                realm_root: anchor.clone(),
            }
        } else {
            RestrictPath::Absolute(absolute_path.clone())
        },
        source_file: anchor.join(".remargin.yaml"),
    };

    let allow_dot_folders = read_allow_dot_folders(system, &anchor)?;
    let rules = rules_for(&resolved, &anchor, &allow_dot_folders);

    let timestamp = Utc::now().to_rfc3339();
    apply_rules(
        system,
        &anchor,
        &absolute_path.display().to_string(),
        &rules,
        settings_files,
        &timestamp,
    )?;

    let RuleSet { allow, deny } = rules;
    let mut rules_applied: Vec<String> = Vec::with_capacity(deny.len() + allow.len());
    rules_applied.extend(deny);
    rules_applied.extend(allow);

    Ok(RestrictOutcome {
        absolute_path,
        anchor,
        claude_files_touched: settings_files.to_vec(),
        rules_applied,
        yaml_was_created,
    })
}

/// Walk up from `cwd` looking for the first ancestor containing a
/// `.claude/` directory. Returns the canonical anchor path.
///
/// # Errors
///
/// Returns an error when no `.claude/` ancestor exists.
pub fn find_claude_anchor(system: &dyn System, cwd: &Path) -> Result<PathBuf> {
    let canonical_cwd = system
        .canonicalize(cwd)
        .unwrap_or_else(|_err| cwd.to_path_buf());
    let mut cursor = canonical_cwd.as_path();
    loop {
        let candidate = cursor.join(".claude");
        if system.is_dir(&candidate).unwrap_or(false) {
            return Ok(cursor.to_path_buf());
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent,
            _ => break,
        }
    }
    bail!(
        "no `.claude/` ancestor found at or above {}; create one (e.g. `mkdir -p .claude`) or run from a Claude-enabled project root",
        canonical_cwd.display()
    );
}

/// Sanctioned in-place editor for `<anchor>/.remargin.yaml`.
///
/// Bypasses the rem-is4z guard (which blocks the public `write` /
/// `edit` ops on `.remargin.yaml`). Restricted to this module so the
/// audit boundary stays explicit: only `restrict` and (later)
/// `unprotect` may use it.
///
/// Returns `true` when the file did not exist before this call.
///
/// # Errors
///
/// I/O / parse failures from reading or writing the file.
pub fn write_remargin_yaml(system: &dyn System, anchor: &Path, body: &str) -> Result<bool> {
    let path = anchor.join(".remargin.yaml");
    let was_absent = system.read_to_string(&path).is_err();
    system
        .write(&path, body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(was_absent)
}

/// Strip `.` and resolve `..` purely lexically so the
/// outside-the-anchor check rejects `../escape` regardless of whether
/// the underlying [`System`] implementation collapses parent
/// references at canonicalise time. `MockSystem`, for example, does
/// not — so a real-world pre-canonicalisation pass is needed to keep
/// the boundary tight in tests.
fn lexical_normalise(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut stack: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    stack.pop();
                } else {
                    stack.push(component);
                }
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                stack.push(component);
            }
        }
    }
    let mut out = PathBuf::new();
    for component in stack {
        out.push(component.as_os_str());
    }
    out
}

fn read_allow_dot_folders(system: &dyn System, anchor: &Path) -> Result<Vec<String>> {
    let path = anchor.join(".remargin.yaml");
    let body = match system.read_to_string(&path) {
        Ok(body) => body,
        Err(_err) => return Ok(Vec::new()),
    };
    let value: Value = serde_yaml::from_str(&body)
        .with_context(|| format!("parsing {} for allow_dot_folders", path.display()))?;
    let Some(perms) = value.get("permissions").and_then(Value::as_mapping) else {
        return Ok(Vec::new());
    };
    let Some(list) = perms.get("allow_dot_folders").and_then(Value::as_sequence) else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect())
}

/// Read `<anchor>/.remargin.yaml`, append-or-merge the restrict
/// entry, and persist via [`write_remargin_yaml`].
fn upsert_remargin_yaml(
    system: &dyn System,
    anchor: &Path,
    path_on_disk: &str,
    args: &RestrictArgs,
) -> Result<bool> {
    let yaml_path = anchor.join(".remargin.yaml");
    let existing = system.read_to_string(&yaml_path).ok();
    let mut value: Value = match existing.as_deref() {
        Some(body) if !body.trim().is_empty() => serde_yaml::from_str(body)
            .with_context(|| format!("parsing existing {}", yaml_path.display()))?,
        _ => Value::Mapping(Mapping::new()),
    };

    let root_map = value
        .as_mapping_mut()
        .context(".remargin.yaml root must be a YAML mapping")?;

    let permissions_value = root_map
        .entry(Value::String(String::from("permissions")))
        .or_insert(Value::Mapping(Mapping::new()));
    let permissions = permissions_value
        .as_mapping_mut()
        .context("`permissions` must be a YAML mapping")?;

    let restrict_value = permissions
        .entry(Value::String(String::from("restrict")))
        .or_insert(Value::Sequence(Vec::new()));
    let restrict_seq = restrict_value
        .as_sequence_mut()
        .context("`permissions.restrict` must be a YAML sequence")?;

    let already = restrict_seq.iter().position(|entry| {
        entry
            .as_mapping()
            .and_then(|m| m.get(Value::String(String::from("path"))))
            .and_then(Value::as_str)
            .is_some_and(|p| p == path_on_disk)
    });

    let mut entry_map = Mapping::new();
    entry_map.insert(
        Value::String(String::from("path")),
        Value::String(String::from(path_on_disk)),
    );
    if !args.also_deny_bash.is_empty() {
        let seq: Vec<Value> = args
            .also_deny_bash
            .iter()
            .map(|cmd| Value::String(cmd.clone()))
            .collect();
        entry_map.insert(
            Value::String(String::from("also_deny_bash")),
            Value::Sequence(seq),
        );
    }
    if args.cli_allowed {
        entry_map.insert(
            Value::String(String::from("cli_allowed")),
            Value::Bool(true),
        );
    }
    let new_entry = Value::Mapping(entry_map);

    if let Some(idx) = already {
        restrict_seq[idx] = new_entry;
    } else {
        restrict_seq.push(new_entry);
    }

    let body = serde_yaml::to_string(&value).context("serializing updated .remargin.yaml")?;
    write_remargin_yaml(system, anchor, &body)
}
