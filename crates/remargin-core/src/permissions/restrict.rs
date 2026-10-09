//! `remargin claude restrict` core.
//!
//! Edits the YAML as `serde_yaml::Value` (mutating only
//! `permissions.trusted_roots`) so unknown top-level keys round-trip
//! verbatim — at the cost of dropping inline comments, acceptable
//! for a config file. Sanctioned `.remargin.yaml` write: bypasses
//! the agent-side write guard via the scoped [`write_remargin_yaml`]
//! helper so the audit boundary stays obvious.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use os_shim::System;
use serde_yaml::{Mapping, Value};

use crate::parser::rfc3339_z;
use crate::permissions::claude_sync::{RuleSet, apply_rules, residual_rules};

const RESTRICT_WILDCARD: &str = "*";

/// Caller-supplied parameters for [`restrict`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestrictArgs {
    /// Extra Bash commands to deny on the restricted path, recorded on the on-disk entry.
    pub also_deny_bash: Vec<String>,
    /// Allow the `remargin` CLI on the path; defaults to `false`.
    pub cli_allowed: bool,
    /// A subpath relative to the anchor, canonicalised before it is stored, or `"*"` for the whole
    /// realm.
    pub path: String,
}

impl RestrictArgs {
    /// Build a [`RestrictArgs`] from outside the crate: the struct is `#[non_exhaustive]`, so
    /// struct literals are unavailable there.
    #[must_use]
    pub const fn new(path: String, also_deny_bash: Vec<String>, cli_allowed: bool) -> Self {
        Self {
            also_deny_bash,
            cli_allowed,
            path,
        }
    }
}

/// What [`simulate_upsert_remargin_yaml`] would do to the YAML file's
/// `permissions.trusted_roots` entry list.
///
/// The "simulate" half of the upsert path: pure analysis, no writes.
/// The "commit" half is [`commit_upsert_remargin_yaml`], which simply
/// persists the projected body via [`write_remargin_yaml`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemarginYamlSim {
    /// The on-disk entry that would be replaced; `None` when there is none or it already matches.
    pub previous_entry: Option<RestrictEntryProjection>,
    pub projected_body: String,
    pub will_be_created: bool,
    pub would_be_noop: bool,
}

/// Snapshot of one `permissions.trusted_roots` entry, parsed back from the
/// on-disk YAML.
///
/// Used by [`simulate_upsert_remargin_yaml`] (and the `plan restrict`
/// projection in [`crate::operations::projections`]) to describe an
/// existing entry that would be overwritten.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct RestrictEntryProjection {
    /// Empty when absent.
    pub also_deny_bash: Vec<String>,
    /// `false` when absent.
    pub cli_allowed: bool,
    pub path: String,
}

/// Description of what [`restrict`] mutated. Returned to the caller
/// (CLI prints a human summary; MCP returns the JSON form) so the
/// user can see exactly which files were touched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestrictOutcome {
    /// Canonical; for the wildcard form this is the anchor root.
    pub absolute_path: PathBuf,
    /// The directory holding `.claude/`, where `.remargin.yaml` lives.
    pub anchor: PathBuf,
    pub claude_files_touched: Vec<PathBuf>,
    pub rules_applied: Vec<String>,
    pub yaml_was_created: bool,
}

/// Run the restrict command end-to-end.
///
/// 1. Walk up from `cwd` to find the nearest `.claude/` ancestor
///    (the anchor).
/// 2. Canonicalise `args.path` (or accept the wildcard).
/// 3. Append-or-merge the entry into `<anchor>/.remargin.yaml`,
///    creating the file if absent.
/// 4. Project the residue the hook cannot cover via [`residual_rules`].
///    That set is empty today, so no settings files or sidecar entry
///    are written — the `.remargin.yaml` entry alone activates both the
///    per-op guard and the `PreToolUse` hook.
///
/// The function is idempotent: re-running with the same arguments
/// produces the same final state (no duplicate entries, no
/// duplicate rules, no extra sidecar records).
///
/// `settings_files` is supplied by the caller so the CLI can pass
/// the resolved project + user-scope paths and tests can pass a
/// hermetic in-mock pair. The function does NOT do `~` expansion;
/// the caller is responsible for that.
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

    // With nothing to project, skip the settings merge and the sidecar write: an existing sidecar
    // entry must survive for `unrestrict` to scrub its rules.
    let rules = residual_rules();
    let claude_files_touched = if rules.is_empty() {
        Vec::new()
    } else {
        let timestamp = rfc3339_z(&Utc::now());
        apply_rules(
            system,
            &anchor,
            &absolute_path.display().to_string(),
            &rules,
            settings_files,
            &timestamp,
        )?;
        settings_files.to_vec()
    };

    let RuleSet { allow, deny } = rules;
    let mut rules_applied: Vec<String> = Vec::with_capacity(deny.len() + allow.len());
    rules_applied.extend(deny);
    rules_applied.extend(allow);

    Ok(RestrictOutcome {
        absolute_path,
        anchor,
        claude_files_touched,
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
/// Bypasses the guard that blocks the public `write` / `edit` ops on `.remargin.yaml`, and is
/// kept to this module so the audit boundary stays explicit. Returns `true` when the file did
/// not exist before this call.
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
/// references at canonicalise time. `MemorySystem`, for example, does
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

/// Pure projection of [`upsert_remargin_yaml`]: read the YAML file,
/// produce the body the live op would write, and report what would
/// change. Does not touch disk except for the read.
///
/// Shared by the live `restrict` path (which forwards
/// `projected_body` to [`commit_upsert_remargin_yaml`]) and the
/// `plan restrict` projection (which discards the body and inspects
/// the rest of the report).
///
/// # Errors
///
/// I/O / parse failures from reading the file or interpreting the
/// existing YAML shape (root mapping, `permissions` mapping,
/// `permissions.trusted_roots` sequence).
pub fn simulate_upsert_remargin_yaml(
    system: &dyn System,
    anchor: &Path,
    path_on_disk: &str,
    args: &RestrictArgs,
) -> Result<RemarginYamlSim> {
    let yaml_path = anchor.join(".remargin.yaml");
    let existing = system.read_to_string(&yaml_path).ok();
    let will_be_created = existing.is_none();
    let existing_body = existing.clone().unwrap_or_default();
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
        .entry(Value::String(String::from("trusted_roots")))
        .or_insert(Value::Sequence(Vec::new()));
    let restrict_seq = restrict_value
        .as_sequence_mut()
        .context("`permissions.trusted_roots` must be a YAML sequence")?;

    let already = restrict_seq.iter().position(|entry| {
        entry
            .as_mapping()
            .and_then(|m| m.get(Value::String(String::from("path"))))
            .and_then(Value::as_str)
            .is_some_and(|p| p == path_on_disk)
    });

    let previous_entry = already
        .and_then(|idx| restrict_seq.get(idx))
        .map(|entry| read_restrict_entry(entry, path_on_disk));

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

    let projected_body =
        serde_yaml::to_string(&value).context("serializing updated .remargin.yaml")?;
    let would_be_noop = !will_be_created && projected_body == existing_body;

    Ok(RemarginYamlSim {
        previous_entry,
        projected_body,
        will_be_created,
        would_be_noop,
    })
}

/// Persist the YAML body produced by [`simulate_upsert_remargin_yaml`].
///
/// Sanctioned write that bypasses the agent guard. Returns
/// `true` when the file did not exist before this call.
///
/// # Errors
///
/// I/O failures from writing the file.
pub fn commit_upsert_remargin_yaml(
    system: &dyn System,
    anchor: &Path,
    sim: &RemarginYamlSim,
) -> Result<bool> {
    write_remargin_yaml(system, anchor, &sim.projected_body)
}

/// Decode one YAML mapping entry from `permissions.trusted_roots` into
/// the structured projection used by simulation outputs and conflict
/// detectors.
fn read_restrict_entry(entry: &Value, fallback_path: &str) -> RestrictEntryProjection {
    let mapping = entry.as_mapping();
    let path = mapping
        .and_then(|m| m.get(Value::String(String::from("path"))))
        .and_then(Value::as_str)
        .map_or_else(|| String::from(fallback_path), String::from);
    let also_deny_bash = mapping
        .and_then(|m| m.get(Value::String(String::from("also_deny_bash"))))
        .and_then(Value::as_sequence)
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let cli_allowed = mapping
        .and_then(|m| m.get(Value::String(String::from("cli_allowed"))))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    RestrictEntryProjection {
        also_deny_bash,
        cli_allowed,
        path,
    }
}

/// Read `<anchor>/.remargin.yaml`, append-or-merge the
/// `trusted_roots` entry, and persist via [`write_remargin_yaml`].
///
/// Implementation: thin wrapper that runs
/// [`simulate_upsert_remargin_yaml`] for the merge logic and then
/// commits the projected body via [`commit_upsert_remargin_yaml`].
/// Both the live op and the `plan restrict` projection walk through
/// the same simulator so the two paths cannot drift.
fn upsert_remargin_yaml(
    system: &dyn System,
    anchor: &Path,
    path_on_disk: &str,
    args: &RestrictArgs,
) -> Result<bool> {
    let sim = simulate_upsert_remargin_yaml(system, anchor, path_on_disk, args)?;
    commit_upsert_remargin_yaml(system, anchor, &sim)
}

/// Render a [`RestrictOutcome`] as human-readable text for `restrict`.
///
/// Output is written to stderr in the CLI; the function returns a `String`
/// so callers can route it to any sink.
#[must_use]
pub fn render_restrict_summary(outcome: &RestrictOutcome) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Restricted: {}", outcome.absolute_path.display());
    let _ = writeln!(out, "  Anchor: {}", outcome.anchor.display());
    if outcome.yaml_was_created {
        let _ = writeln!(
            out,
            "  .remargin.yaml created at {}",
            outcome.anchor.join(".remargin.yaml").display(),
        );
    } else {
        let _ = writeln!(
            out,
            "  .remargin.yaml updated at {}",
            outcome.anchor.join(".remargin.yaml").display(),
        );
    }
    if outcome.claude_files_touched.is_empty() {
        let _ = writeln!(
            out,
            "  Settings: none written -- the PreToolUse hook is the single source of truth and \
             enforces native-tool + Bash access to this path (no projected deny rules)."
        );
    } else {
        let _ = writeln!(
            out,
            "  Settings updated: {} file(s)",
            outcome.claude_files_touched.len(),
        );
        for file in &outcome.claude_files_touched {
            let _ = writeln!(out, "    {}", file.display());
        }
        let _ = writeln!(out, "  Rules written: {}", outcome.rules_applied.len());
    }
    let _ = writeln!(
        out,
        "  The per-op guard (remargin's own ops) is enforcing immediately on the next call; run \
         `remargin doctor` to confirm the hook is wired."
    );
    out
}
