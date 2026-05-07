//! `plan unprotect` projection (rem-6eop / T43).
//!
//! [`project_unprotect`] mirrors [`crate::permissions::unprotect::unprotect`]
//! up through the lookup / rule-discovery step, but never writes. The
//! return value — [`crate::operations::plan::UnprotectConfigDiff`] —
//! names every file the live op would touch and every drift conflict
//! it would surface, so callers can preview the full reversal before
//! committing.
//!
//! ## Read-only by construction
//!
//! Unlike the document-projection helpers in
//! [`crate::operations::projections`] (pure with respect to disk),
//! config projections must read on-disk state to compare against the
//! requested mutation: the YAML's `permissions.restrict` list, the
//! sidecar entry, and each settings file the sidecar's
//! `added_to_files` array points at. They still never write — that is
//! the invariant the integration test guards.

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_yaml::Value;

use crate::operations::plan::{
    UnprotectConfigDiff, UnprotectConflict, UnprotectEntryAction, UnprotectSettingsDiff,
    UnprotectSidecarDiff, UnprotectYamlDiff,
};
use crate::permissions::claude_sync::canonicalize_rule;
use crate::permissions::restrict::{RestrictEntryProjection, find_claude_anchor};
use crate::permissions::sidecar;
use crate::permissions::unprotect::UnprotectArgs;

/// Wildcard literal accepted in `unprotect.path`. Mirrors the schema
/// constant in [`crate::config::permissions`].
const RESTRICT_WILDCARD: &str = "*";

/// Outcome of [`project_unprotect`].
///
/// The `Reject` variant carries the diagnostic string that
/// `plan unprotect` should surface as `reject_reason` (e.g. no
/// `.claude/` ancestor). The `Diff` variant carries the full
/// preview. The diff arm is boxed so the enum tag stays thin
/// (`large_enum_variant`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum UnprotectProjection {
    /// Concrete preview the dispatcher attaches to `PlanReport`.
    Diff(Box<UnprotectConfigDiff>),
    /// Hard reject. The dispatcher sets `would_commit = false` and
    /// surfaces the carried reason verbatim.
    Reject(String),
}

/// Build an [`UnprotectConfigDiff`] describing what
/// [`crate::permissions::unprotect::unprotect`] would do, without
/// writing to disk.
///
/// Surfaces every detectable drift conflict (YAML entry missing,
/// sidecar entry missing, rule already absent) plus a fail-closed
/// reject reason returned via the [`UnprotectProjection::Reject`]
/// variant — the dispatcher consumes it to populate
/// `PlanReport::reject_reason`.
///
/// # Errors
///
/// YAML / settings-file / sidecar I/O / parse failures.
pub fn project_unprotect(
    system: &dyn System,
    cwd: &Path,
    args: &UnprotectArgs,
) -> Result<UnprotectProjection> {
    let anchor = match find_claude_anchor(system, cwd) {
        Ok(anchor) => anchor,
        Err(err) => {
            return Ok(UnprotectProjection::Reject(format!("{err:#}")));
        }
    };

    let absolute_path = if args.path == RESTRICT_WILDCARD {
        anchor.clone()
    } else {
        let candidate = anchor.join(&args.path);
        system.canonicalize(&candidate).unwrap_or(candidate)
    };
    let target_key = absolute_path.display().to_string();

    let yaml_path = anchor.join(".remargin.yaml");
    let yaml_entry = read_yaml_entry(system, &yaml_path, &args.path)?;

    let sidecar_path = sidecar::sidecar_path(&anchor);
    let sidecar_loaded = sidecar::load(system, &anchor)?;
    let sidecar_entry = sidecar_loaded.entries.get(&target_key);

    let mut conflicts: Vec<UnprotectConflict> = Vec::new();

    let yaml_entry_present = yaml_entry.is_some();
    let yaml_diff = UnprotectYamlDiff {
        entry_action: if yaml_entry_present {
            UnprotectEntryAction::WouldBeRemoved
        } else {
            UnprotectEntryAction::Absent
        },
        path: yaml_path.clone(),
        previous_entry: yaml_entry,
    };
    if !yaml_entry_present {
        conflicts.push(UnprotectConflict::YamlEntryMissing { path: yaml_path });
    }

    let (sidecar_diff, settings_files) = if let Some(entry) = sidecar_entry {
        let mut sims: Vec<UnprotectSettingsDiff> = Vec::new();
        for settings_file in &entry.added_to_files {
            let sim = simulate_settings_scrub(
                system,
                settings_file,
                &entry.allow,
                &entry.deny,
                &mut conflicts,
            )?;
            sims.push(sim);
        }
        (
            UnprotectSidecarDiff {
                entry_action: UnprotectEntryAction::WouldBeRemoved,
                path: sidecar_path,
            },
            sims,
        )
    } else {
        conflicts.push(UnprotectConflict::SidecarEntryMissing {
            path: absolute_path.clone(),
        });
        (
            UnprotectSidecarDiff {
                entry_action: UnprotectEntryAction::Absent,
                path: sidecar_path,
            },
            Vec::new(),
        )
    };

    let diff = UnprotectConfigDiff {
        absolute_path,
        anchor,
        conflicts,
        remargin_yaml: yaml_diff,
        settings_files,
        sidecar: sidecar_diff,
    };
    Ok(UnprotectProjection::Diff(Box::new(diff)))
}

/// Read the matching `permissions.restrict[*]` entry from the YAML
/// file at `yaml_path`, where `path_on_disk` is the same key
/// `unprotect`'s mutating path uses. Returns `None` when the file is
/// missing, the file has no matching entry, or the YAML shape is not
/// the expected `permissions.restrict[]` mapping (defensive parity
/// with the live op which treats unknown shapes as "nothing to
/// remove").
///
/// Parse errors propagate up to the caller — the same behaviour the
/// live op exhibits via `serde_yaml::from_str`.
fn read_yaml_entry(
    system: &dyn System,
    yaml_path: &Path,
    path_on_disk: &str,
) -> Result<Option<RestrictEntryProjection>> {
    let body = match system.read_to_string(yaml_path) {
        Ok(body) => body,
        Err(_err) => return Ok(None),
    };
    if body.trim().is_empty() {
        return Ok(None);
    }
    let value: Value =
        serde_yaml::from_str(&body).with_context(|| format!("parsing {}", yaml_path.display()))?;
    let Some(root) = value.as_mapping() else {
        return Ok(None);
    };
    let Some(permissions) = root
        .get(Value::String(String::from("permissions")))
        .and_then(Value::as_mapping)
    else {
        return Ok(None);
    };
    let Some(restrict_seq) = permissions
        .get(Value::String(String::from("trusted_roots")))
        .and_then(Value::as_sequence)
    else {
        return Ok(None);
    };
    for entry in restrict_seq {
        let Some(mapping) = entry.as_mapping() else {
            continue;
        };
        let entry_path = mapping
            .get(Value::String(String::from("path")))
            .and_then(Value::as_str);
        if entry_path != Some(path_on_disk) {
            continue;
        }
        let also_deny_bash = mapping
            .get(Value::String(String::from("also_deny_bash")))
            .and_then(Value::as_sequence)
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let cli_allowed = mapping
            .get(Value::String(String::from("cli_allowed")))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        return Ok(Some(RestrictEntryProjection {
            also_deny_bash,
            cli_allowed,
            path: String::from(path_on_disk),
        }));
    }
    Ok(None)
}

/// Project the per-file scrub: split each tracked rule (sidecar's
/// recorded `allow` + `deny`) into "still present, would be removed"
/// vs. "already absent (drift)". Pushes one
/// [`UnprotectConflict::RuleAlreadyAbsent`] per drift hit.
///
/// Mirrors the live `revert_rules` membership check via
/// [`canonicalize_rule`] so legacy `//` / `///` prefix forms still
/// match (rem-em33).
fn simulate_settings_scrub(
    system: &dyn System,
    settings_file: &Path,
    allow_rules: &[String],
    deny_rules: &[String],
    conflicts: &mut Vec<UnprotectConflict>,
) -> Result<UnprotectSettingsDiff> {
    let body = system.read_to_string(settings_file).ok();
    let value: Option<serde_json::Value> = match body.as_deref() {
        Some(text) if !text.trim().is_empty() => Some(
            serde_json::from_str(text)
                .with_context(|| format!("parsing settings JSON at {}", settings_file.display()))?,
        ),
        _ => None,
    };

    let existing_deny = value
        .as_ref()
        .map(|v| read_permission_array(v, "deny"))
        .unwrap_or_default();
    let existing_allow = value
        .as_ref()
        .map(|v| read_permission_array(v, "allow"))
        .unwrap_or_default();

    let mut rules_to_remove: Vec<String> = Vec::new();
    let mut rules_already_absent: Vec<String> = Vec::new();

    classify_rules(
        deny_rules,
        &existing_deny,
        settings_file,
        &mut rules_to_remove,
        &mut rules_already_absent,
        conflicts,
    );
    classify_rules(
        allow_rules,
        &existing_allow,
        settings_file,
        &mut rules_to_remove,
        &mut rules_already_absent,
        conflicts,
    );

    Ok(UnprotectSettingsDiff {
        path: settings_file.to_path_buf(),
        rules_already_absent,
        rules_to_remove,
    })
}

/// Split `tracked` into "present in `existing`" vs. "absent". Drift
/// hits land in `conflicts` as
/// [`UnprotectConflict::RuleAlreadyAbsent`].
fn classify_rules(
    tracked: &[String],
    existing: &[String],
    settings_file: &Path,
    rules_to_remove: &mut Vec<String>,
    rules_already_absent: &mut Vec<String>,
    conflicts: &mut Vec<UnprotectConflict>,
) {
    for rule in tracked {
        let target = canonicalize_rule(rule);
        if existing.iter().any(|e| canonicalize_rule(e) == target) {
            rules_to_remove.push(rule.clone());
        } else {
            rules_already_absent.push(rule.clone());
            conflicts.push(UnprotectConflict::RuleAlreadyAbsent {
                rule: rule.clone(),
                settings_file: settings_file.to_path_buf(),
            });
        }
    }
}

fn read_permission_array(value: &serde_json::Value, key: &str) -> Vec<String> {
    let Some(permissions) = value
        .get("permissions")
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    let Some(array) = permissions.get(key).and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}
