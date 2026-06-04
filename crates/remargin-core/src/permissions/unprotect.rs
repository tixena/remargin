//! `remargin claude unrestrict` core.
//!
//! Reverses the three forks `restrict` writes — `.remargin.yaml`
//! first (so the per-op guard stops enforcing immediately), then the
//! sidecar, then the two Claude settings files (only the sidecar-
//! tracked rules, never user-added ones). Manual mid-state edits
//! surface as [`UnprotectOutcome::warnings`] rather than errors — the
//! goal is the cleanest possible final state, not policing prior
//! edits.

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
    /// match the on-disk `path` field of the `trusted_roots` entry
    /// being reversed (the lookup key for both the YAML editor and
    /// the sidecar).
    pub path: String,
    /// When `true`, [`unprotect`] returns an error instead of a
    /// warning when `path` is not currently restricted (no YAML
    /// entry and no sidecar entry). For scripted callers that want
    /// hard-fail-on-miss semantics. Default `false` preserves the
    /// human-friendly warn-and-no-op behaviour.
    pub strict: bool,
}

impl UnprotectArgs {
    /// Build a [`UnprotectArgs`] across the crate boundary. The
    /// struct is `#[non_exhaustive]` so external callers cannot use
    /// struct literals; this constructor preserves the API stability
    /// guarantee.
    #[must_use]
    pub const fn new(path: String) -> Self {
        Self {
            path,
            strict: false,
        }
    }

    /// Builder-style setter for [`UnprotectArgs::strict`].
    #[must_use]
    pub const fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
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
    /// `true` when a matching `permissions.trusted_roots[*].path` entry
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
    } else if args.strict {
        anyhow::bail!("{} is not currently restricted (--strict)", args.path);
    } else {
        outcome
            .warnings
            .push(format!("{} was not currently restricted; no-op", args.path));
    }

    Ok(outcome)
}

/// Remove the `permissions.trusted_roots[*]` entry whose `path` field
/// matches `path_on_disk`. Returns `true` when an entry was found
/// and removed.
///
/// After the (possibly-no-op) removal, [`compact_permissions`] prunes
/// any empty `permissions:` sub-arrays and removes the wrapping
/// mapping if it ends up empty. The YAML is rewritten when EITHER the
/// removal OR the compaction produced a change, so hand-edited
/// `trusted_roots: []` / `deny_ops: []` placeholders left around from
/// earlier sessions are scrubbed on the next unprotect call.
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

    let removed = remove_restrict_path(root, path_on_disk);
    let compacted = compact_permissions(root);

    if !removed && !compacted {
        return Ok(false);
    }

    let updated = serde_yaml::to_string(&value).context("serializing updated .remargin.yaml")?;
    write_remargin_yaml(system, anchor, &updated)?;
    Ok(removed)
}

/// Remove the `permissions.trusted_roots[*]` entry whose `path` field
/// matches `path_on_disk`. Returns `true` when an entry was actually
/// removed. Missing `permissions:` or `trusted_roots:` keys are treated
/// as "nothing to remove".
fn remove_restrict_path(root: &mut serde_yaml::Mapping, path_on_disk: &str) -> bool {
    let permissions_key = Value::String(String::from("permissions"));
    let Some(permissions) = root
        .get_mut(&permissions_key)
        .and_then(Value::as_mapping_mut)
    else {
        return false;
    };
    let restrict_key = Value::String(String::from("trusted_roots"));
    let Some(restrict_seq) = permissions
        .get_mut(&restrict_key)
        .and_then(Value::as_sequence_mut)
    else {
        return false;
    };

    let prior_len = restrict_seq.len();
    restrict_seq.retain(|entry| {
        entry
            .as_mapping()
            .and_then(|m| m.get(Value::String(String::from("path"))))
            .and_then(Value::as_str)
            .is_none_or(|p| p != path_on_disk)
    });
    restrict_seq.len() != prior_len
}

/// Prune empty `permissions:` sub-arrays. If the whole `permissions:`
/// mapping ends up empty, remove it from the document root.
///
/// "Empty sub-array" means `Value::Sequence` of length 0. Non-array
/// values (none today, but defensive) are left alone.
///
/// Returns `true` when any change was made (a sub-key was removed
/// or the wrapping `permissions:` mapping was deleted).
fn compact_permissions(root: &mut serde_yaml::Mapping) -> bool {
    let permissions_key = Value::String(String::from("permissions"));
    let Some(permissions) = root
        .get_mut(&permissions_key)
        .and_then(Value::as_mapping_mut)
    else {
        return false;
    };
    let prior_len = permissions.len();
    permissions.retain(|_key, val| !matches!(val, Value::Sequence(seq) if seq.is_empty()));
    let pruned_subkey = permissions.len() != prior_len;
    if permissions.is_empty() {
        root.remove(&permissions_key);
        return true;
    }
    pruned_subkey
}

/// Render an [`UnprotectOutcome`] as human-readable text for `unprotect`.
///
/// Output is written to stderr in the CLI; the function returns a `String`
/// so callers can route it to any sink.
#[must_use]
pub fn render_unprotect_summary(outcome: &UnprotectOutcome) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Unprotected: {}", outcome.absolute_path.display());
    let _ = writeln!(out, "  Anchor: {}", outcome.anchor.display());
    if outcome.yaml_entry_removed {
        let _ = writeln!(
            out,
            "  .remargin.yaml updated at {}",
            outcome.anchor.join(".remargin.yaml").display(),
        );
    } else {
        let _ = writeln!(out, "  .remargin.yaml: no matching entry");
    }
    if outcome.claude_files_touched.is_empty() {
        let _ = writeln!(out, "  Settings: none touched (no sidecar entry)");
    } else {
        let _ = writeln!(
            out,
            "  Settings updated: {} file(s)",
            outcome.claude_files_touched.len(),
        );
        for file in &outcome.claude_files_touched {
            let _ = writeln!(out, "    {}", file.display());
        }
    }
    if !outcome.warnings.is_empty() {
        let _ = writeln!(out, "  Warnings:");
        for warning in &outcome.warnings {
            let _ = writeln!(out, "    - {warning}");
        }
    }
    let _ = writeln!(
        out,
        "  Note: Claude must reload its settings for Layer 2 (NATIVE tool denials) to take effect."
    );
    let _ = writeln!(
        out,
        "  Layer 1 (remargin's own ops) stops enforcing immediately on the next call."
    );
    out
}
