//! `plan restrict` projection.
//!
//! Mirrors `restrict` up through the merge / rule-generation step but
//! never writes. Live and projection paths share the same simulation
//! helpers so new behaviour lands on both automatically. Reads
//! on-disk state (YAML, settings, sidecar) so a `plan` can compare
//! against current reality; the never-write invariant is guarded by
//! integration tests.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;

use crate::operations::plan::{
    ConfigConflict, ConfigPlanDiff, EntryAction, RemarginYamlDiff, SettingsFileDiff, SidecarDiff,
};
use crate::permissions::claude_sync::rule_shape::{RuleShape, rules_overlap};
use crate::permissions::claude_sync::{self, RuleSet, residual_rules};
use crate::permissions::restrict::{
    self as permissions_restrict, RestrictArgs, RestrictEntryProjection,
};
use crate::permissions::sidecar;

const RESTRICT_WILDCARD: &str = "*";

/// Outcome of [`project_restrict`].
///
/// The `Reject` variant carries the diagnostic string that
/// `plan restrict` should surface as `reject_reason` (e.g. the path
/// resolves outside the anchor, no `.claude/` ancestor, etc.). The
/// `Diff` variant carries the full preview. The diff arm is boxed
/// so the enum tag stays thin (`large_enum_variant`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RestrictProjection {
    Diff(Box<ConfigPlanDiff>),
    Reject(String),
}

/// A restrict target resolved two ways: its absolute path, and the form written to
/// `.remargin.yaml`.
struct PathResolution {
    absolute_path: PathBuf,
    on_disk_path: String,
}

/// Build a [`ConfigPlanDiff`] describing what
/// [`permissions_restrict::restrict`] would do, without writing to
/// disk.
///
/// Surfaces every detectable conflict (allow-vs-deny overlap, YAML
/// entry would change, anchor surprise) plus a fail-closed
/// `path-outside-anchor` reject reason returned via the
/// [`RestrictProjection::Reject`] variant — the dispatcher consumes
/// it to populate `PlanReport::reject_reason`.
///
/// # Errors
///
/// Settings-file or sidecar I/O / parse failures.
pub fn project_restrict(
    system: &dyn System,
    cwd: &Path,
    args: &RestrictArgs,
    settings_files: &[PathBuf],
) -> Result<RestrictProjection> {
    let canonical_cwd = system
        .canonicalize(cwd)
        .unwrap_or_else(|_err| cwd.to_path_buf());
    let anchor = match permissions_restrict::find_claude_anchor(system, cwd) {
        Ok(anchor) => anchor,
        Err(err) => {
            return Ok(RestrictProjection::Reject(format!("{err:#}")));
        }
    };

    let resolution = match resolve_path(system, &anchor, args) {
        Ok(resolution) => resolution,
        Err(err) => {
            return Ok(RestrictProjection::Reject(format!("{err:#}")));
        }
    };

    let yaml_sim = permissions_restrict::simulate_upsert_remargin_yaml(
        system,
        &anchor,
        &resolution.on_disk_path,
        args,
    )?;

    // The live `restrict` projects only what the hook cannot cover, which is nothing today: no
    // settings file is touched and no sidecar entry is written.
    let rules = residual_rules();
    let settings_sims = if rules.is_empty() {
        Vec::new()
    } else {
        claude_sync::simulate_apply_rules(system, settings_files, &rules)?
    };

    let target_key = resolution.absolute_path.display().to_string();
    let sidecar_diff = simulate_sidecar(system, &anchor, &target_key, &rules)?;

    let mut diff = ConfigPlanDiff {
        absolute_path: resolution.absolute_path,
        anchor: anchor.clone(),
        conflicts: Vec::new(),
        remargin_yaml: build_yaml_diff(&anchor, &yaml_sim, args),
        settings_files: settings_sims.iter().map(settings_diff_from_sim).collect(),
        sidecar: sidecar_diff,
    };

    detect_conflicts(
        &mut diff,
        &settings_sims,
        &yaml_sim,
        args,
        &canonical_cwd,
        &anchor,
    );

    Ok(RestrictProjection::Diff(Box::new(diff)))
}

fn build_yaml_diff(
    anchor: &Path,
    sim: &permissions_restrict::RemarginYamlSim,
    args: &RestrictArgs,
) -> RemarginYamlDiff {
    let entry_action = if sim.would_be_noop {
        EntryAction::Noop
    } else if sim.previous_entry.is_some() {
        EntryAction::Updated
    } else {
        EntryAction::Added
    };
    let projected_entry = if matches!(entry_action, EntryAction::Noop) {
        sim.previous_entry.clone()
    } else {
        Some(RestrictEntryProjection {
            also_deny_bash: args.also_deny_bash.clone(),
            cli_allowed: args.cli_allowed,
            path: if args.path == RESTRICT_WILDCARD {
                String::from(RESTRICT_WILDCARD)
            } else {
                args.path.clone()
            },
        })
    };
    RemarginYamlDiff {
        entry_action,
        path: anchor.join(".remargin.yaml"),
        previous_entry: sim.previous_entry.clone(),
        projected_entry,
        will_be_created: sim.will_be_created,
    }
}

fn detect_conflicts(
    diff: &mut ConfigPlanDiff,
    settings_sims: &[claude_sync::SettingsFileSim],
    yaml_sim: &permissions_restrict::RemarginYamlSim,
    args: &RestrictArgs,
    cwd: &Path,
    anchor: &Path,
) {
    // Structural `(tool, path-glob)` comparison, so format-equivalent rules match: a `//` prefix,
    // a trailing slash, `.` and `..`. Different tools and `/foo` vs `/foobar` stay distinct.
    detect_allow_deny_overlap(diff, settings_sims);

    // An overwrite with identical arguments is skipped: `would_be_noop` catches it.
    if !yaml_sim.would_be_noop
        && let Some(previous) = &yaml_sim.previous_entry
    {
        let projected = RestrictEntryProjection {
            also_deny_bash: args.also_deny_bash.clone(),
            cli_allowed: args.cli_allowed,
            path: previous.path.clone(),
        };
        if previous != &projected {
            diff.conflicts.push(ConfigConflict::YamlEntryWouldChange {
                path: previous.path.clone(),
                previous: previous.clone(),
                projected,
            });
        }
    }

    // Reported even when cwd is a descendant of the anchor: the realm root may sit further up
    // than the caller thinks.
    if cwd != anchor {
        diff.conflicts.push(ConfigConflict::AnchorIsAncestor {
            anchor: anchor.to_path_buf(),
            cwd: cwd.to_path_buf(),
        });
    }
}

fn lexical_normalise(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut stack: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    let _: Option<Component<'_>> = stack.pop();
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

fn resolve_path(system: &dyn System, anchor: &Path, args: &RestrictArgs) -> Result<PathResolution> {
    if args.path == RESTRICT_WILDCARD {
        return Ok(PathResolution {
            absolute_path: anchor.to_path_buf(),
            on_disk_path: String::from(RESTRICT_WILDCARD),
        });
    }

    let candidate = anchor.join(&args.path);
    let lexically_normalised = lexical_normalise(&candidate);
    let absolute = system
        .canonicalize(&lexically_normalised)
        .unwrap_or(lexically_normalised);
    if !absolute.starts_with(anchor) {
        anyhow::bail!(
            "restrict path {:?} resolves to {} which is outside the anchor {}",
            args.path,
            absolute.display(),
            anchor.display()
        );
    }
    Ok(PathResolution {
        absolute_path: absolute,
        on_disk_path: args.path.clone(),
    })
}

/// Path-aware allow/deny overlap detector.
///
/// For every projected deny rule, parses the rule into a [`RuleShape`]
/// and walks every existing allow rule on the same settings file
/// looking for a structural overlap. Pushes one
/// [`ConfigConflict::AllowDenyOverlap`] per `(allow, deny)` overlap
/// pair, tagged with the [`claude_sync::OverlapKind`] that describes
/// the relationship.
///
/// Naive O(deny × allow). Both lists are small (tens of rules per
/// file) and the work is per-file, so indexing is unnecessary.
fn detect_allow_deny_overlap(
    diff: &mut ConfigPlanDiff,
    settings_sims: &[claude_sync::SettingsFileSim],
) {
    for sim in settings_sims {
        for projected_deny in &sim.deny_rules_to_add {
            let deny_shape = RuleShape::parse(projected_deny);
            for existing_allow in &sim.existing_allow_rules {
                let allow_shape = RuleShape::parse(existing_allow);
                // The classifier is written from the allow side's perspective, so the order is
                // `(allow, deny)`.
                let Some(kind) = rules_overlap(&allow_shape, &deny_shape) else {
                    continue;
                };
                diff.conflicts.push(ConfigConflict::AllowDenyOverlap {
                    allow_rule: existing_allow.clone(),
                    overlap_kind: kind,
                    projected_deny_rule: projected_deny.clone(),
                    settings_file: sim.path.clone(),
                });
            }
        }
    }
}

fn settings_diff_from_sim(sim: &claude_sync::SettingsFileSim) -> SettingsFileDiff {
    SettingsFileDiff {
        allow_rules_already_present: sim.allow_rules_already_present.clone(),
        allow_rules_to_add: sim.allow_rules_to_add.clone(),
        deny_rules_already_present: sim.deny_rules_already_present.clone(),
        deny_rules_to_add: sim.deny_rules_to_add.clone(),
        path: sim.path.clone(),
        will_be_created: sim.will_be_created,
    }
}

fn simulate_sidecar(
    system: &dyn System,
    anchor: &Path,
    target_key: &str,
    rules: &RuleSet,
) -> Result<SidecarDiff> {
    let sidecar_path = sidecar::sidecar_path(anchor);
    // An empty projection writes no sidecar entry, so the plan shows no sidecar change.
    if rules.is_empty() {
        return Ok(SidecarDiff {
            entry_action: EntryAction::Noop,
            path: sidecar_path,
            will_be_created: false,
        });
    }
    let exists_on_disk = system.read_to_string(&sidecar_path).is_ok();
    let will_be_created = !exists_on_disk;

    let loaded = sidecar::load(system, anchor)?;
    let entry_action = loaded
        .entries
        .get(target_key)
        .map_or(EntryAction::Added, |existing| {
            if existing.allow == rules.allow && existing.deny == rules.deny {
                EntryAction::Noop
            } else {
                EntryAction::Updated
            }
        });
    Ok(SidecarDiff {
        entry_action,
        path: sidecar_path,
        will_be_created,
    })
}
