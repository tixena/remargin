//! `remargin unprotect` core (rem-yj1j.6 / T27, slice 1 — `rem-3p2v`).
//!
//! [`unprotect`] is the public entry point: anchor discovery (walk
//! up to the nearest `.claude/` ancestor), sidecar lookup, removal
//! of the matching `permissions.restrict` entry from
//! `<anchor>/.remargin.yaml`, and finally a call into
//! [`crate::permissions::claude_sync::revert_rules`] to scrub the
//! settings-file rules + drop the sidecar entry.
//!
//! ## Three forks of state to clean up
//!
//! `restrict()` writes three pieces of state. `unprotect()` reverses
//! them in the order that minimises divergence:
//!
//! 1. `<anchor>/.remargin.yaml` — the source-of-truth restrict
//!    declaration. Removed first so the per-op guard stops enforcing
//!    on the next call.
//! 2. `<anchor>/.claude/.remargin-restrictions.json` (the sidecar)
//!    — gone via [`revert_rules`] (which also returns the rules it
//!    found in the settings files).
//! 3. The two Claude settings files — scrubbed of the sidecar-
//!    tracked rules (and only those rules; never user-added ones).
//!
//! ## Diagnostic warnings, not errors
//!
//! Manual edits between `restrict` and `unprotect` are common (a
//! user who hand-tweaks a Claude settings file, or a teammate who
//! removed an entry from `.remargin.yaml` without going through
//! `unprotect`). The function surfaces these via
//! [`UnprotectOutcome::warnings`] without failing — the goal is to
//! reach the cleanest possible final state, not to police prior
//! edits.
//!
//! ## rem-is4z bypass
//!
//! Same scoping as `restrict`: this module is the only place
//! besides [`crate::permissions::restrict`] that writes
//! `.remargin.yaml` through the dedicated
//! [`crate::permissions::restrict::write_remargin_yaml`] helper.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_yaml::Value;

use crate::permissions::claude_sync::{RevertReport, revert_rules};
use crate::permissions::restrict::{find_claude_anchor, write_remargin_yaml};
use crate::permissions::sidecar;

/// Wildcard literal mirroring the schema constant. Keeps the
/// public surface symmetric with `restrict`.
const RESTRICT_WILDCARD: &str = "*";

/// Caller-supplied parameters for [`unprotect`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnprotectArgs {
    /// Subpath relative to the anchor, OR the literal `"*"`. Must
    /// match the on-disk `path` field of the restrict entry being
    /// reversed (the lookup key for both the YAML editor and the
    /// sidecar).
    pub path: String,
}

impl UnprotectArgs {
    /// Build a [`UnprotectArgs`] across the crate boundary. The
    /// struct is `#[non_exhaustive]` so external callers cannot use
    /// struct literals; this constructor preserves the API stability
    /// guarantee.
    #[must_use]
    pub const fn new(path: String) -> Self {
        Self { path }
    }
}

/// Description of what [`unprotect`] mutated. Returned to the
/// caller (CLI / MCP) so the user can see exactly which rules were
/// scrubbed and which warnings surfaced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnprotectOutcome {
    /// Canonical absolute path that was unprotected. For the
    /// wildcard form, the anchor root.
    pub absolute_path: PathBuf,
    /// The directory holding `.claude/`.
    pub anchor: PathBuf,
    /// Settings files the revert touched. Empty when no sidecar
    /// entry existed.
    pub claude_files_touched: Vec<PathBuf>,
    /// Every rule string that was scrubbed from the settings files.
    pub rules_removed: Vec<String>,
    /// Diagnostics — manual-edit detections, missing files, etc.
    /// Empty on the clean-revert happy path.
    pub warnings: Vec<String>,
    /// `true` when a matching `permissions.restrict[*].path` entry
    /// existed in `.remargin.yaml` and was removed.
    pub yaml_entry_removed: bool,
}

/// Run the unprotect command end-to-end.
///
/// 1. Walk up from `cwd` to the nearest `.claude/` ancestor.
/// 2. Resolve `args.path` (or accept the wildcard).
/// 3. Remove the matching entry from `<anchor>/.remargin.yaml`.
/// 4. Call [`revert_rules`] to scrub the settings files + drop the
///    sidecar entry.
/// 5. Surface the rules that were removed and any warnings the
///    revert produced.
///
/// Idempotent: a path that was never restricted produces an
/// [`UnprotectOutcome`] with `yaml_entry_removed = false`,
/// `rules_removed = []`, and a warning naming the situation.
///
/// # Errors
///
/// - No `.claude/` ancestor found.
/// - I/O / parse failures from the YAML editor or
///   [`revert_rules`].
pub fn unprotect(
    system: &dyn System,
    cwd: &Path,
    args: &UnprotectArgs,
) -> Result<UnprotectOutcome> {
    let anchor = find_claude_anchor(system, cwd)
        .with_context(|| format!("looking for `.claude/` ancestor of {}", cwd.display()))?;

    let absolute_path = if args.path == RESTRICT_WILDCARD {
        anchor.clone()
    } else {
        let candidate = anchor.join(&args.path);
        system.canonicalize(&candidate).unwrap_or(candidate)
    };

    let yaml_entry_removed = remove_yaml_entry(system, &anchor, &args.path)?;

    let target_key = absolute_path.display().to_string();
    let sidecar_present = sidecar::load(system, &anchor)?
        .entries
        .contains_key(&target_key);

    let mut outcome = UnprotectOutcome {
        absolute_path,
        anchor: anchor.clone(),
        claude_files_touched: Vec::new(),
        rules_removed: Vec::new(),
        warnings: Vec::new(),
        yaml_entry_removed,
    };

    if sidecar_present {
        let report: RevertReport = revert_rules(system, &anchor, &target_key)?;
        outcome.claude_files_touched = report.touched_files;
        outcome.warnings.extend(report.warnings);
        if !yaml_entry_removed {
            outcome.warnings.push(format!(
                "{} had no entry in {}/.remargin.yaml; sidecar reversal proceeded anyway",
                args.path,
                anchor.display()
            ));
        }
    } else if yaml_entry_removed {
        outcome.warnings.push(format!(
            "{} had no sidecar entry; .remargin.yaml entry removed but Claude settings were left untouched",
            args.path
        ));
    } else {
        outcome
            .warnings
            .push(format!("{} was not currently restricted; no-op", args.path));
    }

    Ok(outcome)
}

/// Remove the `permissions.restrict[*]` entry whose `path` field
/// matches `path_on_disk`. Returns `true` when an entry was found
/// and removed. Leaves an empty `restrict: []` array in place to
/// keep the schema stable for the next `restrict` call.
fn remove_yaml_entry(system: &dyn System, anchor: &Path, path_on_disk: &str) -> Result<bool> {
    let yaml_path = anchor.join(".remargin.yaml");
    let body = match system.read_to_string(&yaml_path) {
        Ok(body) => body,
        Err(_err) => return Ok(false),
    };
    if body.trim().is_empty() {
        return Ok(false);
    }
    let mut value: Value =
        serde_yaml::from_str(&body).with_context(|| format!("parsing {}", yaml_path.display()))?;

    let Some(root) = value.as_mapping_mut() else {
        return Ok(false);
    };
    let Some(permissions) = root
        .get_mut(Value::String(String::from("permissions")))
        .and_then(Value::as_mapping_mut)
    else {
        return Ok(false);
    };
    let Some(restrict_seq) = permissions
        .get_mut(Value::String(String::from("restrict")))
        .and_then(Value::as_sequence_mut)
    else {
        return Ok(false);
    };

    let prior_len = restrict_seq.len();
    restrict_seq.retain(|entry| {
        entry
            .as_mapping()
            .and_then(|m| m.get(Value::String(String::from("path"))))
            .and_then(Value::as_str)
            .is_none_or(|p| p != path_on_disk)
    });

    if restrict_seq.len() == prior_len {
        return Ok(false);
    }

    let updated = serde_yaml::to_string(&value).context("serializing updated .remargin.yaml")?;
    write_remargin_yaml(system, anchor, &updated)?;
    Ok(true)
}
