//! Claude-settings synchronizer rule generation.
//!
//! Pure function over a resolved root, anchor, and `allow_dot_folders`
//! list; produces the exact deny/allow strings the merger writes into
//! `.claude/settings.local.json` and `~/.claude/settings.json`. No
//! filesystem access — inputs are materialised by the caller.
//!
//! Dot-folder denies use a single `.*/**` wildcard rather than walking
//! the filesystem at generation time (which would race against folder
//! creation). When `allow_dot_folders` names specific folders, narrow
//! re-allows override the broader deny.
//!
//! The `PreToolUse` hook (pretool.rs) is the single source of truth for
//! enforcement. `remargin restrict` writes only [`residual_rules`] into the
//! settings files, which is empty: the hook covers every shape. The full shape
//! set from [`hook_covered_rules`] exists so `remargin doctor` can recognise
//! deny rules already sitting in a settings file and flag them as drift.

pub mod rule_shape;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::permissions::resolve::{ResolvedTrustedRoot, TrustedRootPath};
use crate::permissions::sidecar::{self, SidecarEntry};

/// The editor-side tools the path denies cover: the same tools the `PreToolUse` hook gates.
const EDITOR_TOOLS: &[&str] = &["Edit", "Write", "Read", "NotebookEdit", "MultiEdit"];

/// Each token expands to `Bash(<token> {glob_root}/**)`; a trailing `*`, or its absence, is part
/// of the token. Several commands appear bare and with `*` so both `cmd <path>` and
/// `cmd -f <path>` match.
pub const BASH_MUTATORS: &[&str] = &[
    "cp *",
    "mv *",
    "tee",
    "tee *",
    "sed -i *",
    "sed *",
    "truncate *",
    "touch",
    "touch *",
    // Bare and `*` forms: `rm /path/foo` has no flag token for the middle `*` to match.
    "rm",
    "rm *",
    "rmdir",
    "rmdir *",
    "unlink",
    "unlink *",
    "shred",
    "shred *",
    "install *",
    "ln *",
    "mkdir *",
    "mkfifo *",
    "mknod *",
    "chattr *",
    "chgrp *",
    "chmod *",
    "chown *",
    "setfacl *",
    "ed *",
    "emacs *",
    "micro *",
    "nano *",
    "nvim *",
    "vi *",
    "vim *",
    "awk *",
    "lua *",
    "node *",
    "perl *",
    "php *",
    "python *",
    "python3 *",
    "ruby *",
    "7z *",
    "bunzip2 *",
    "bzip2 *",
    "gunzip *",
    "gzip *",
    "tar *",
    "unxz *",
    "unzip *",
    "xz *",
    "zip *",
    "zstd *",
    "rsync *",
    "scp *",
    "sftp *",
    "patch *",
    "curl *",
    "wget *",
    // `echo /restricted/file | xargs rm` would otherwise dodge `Bash(rm *)`.
    "xargs *",
    "find *",
    "bash *",
    "dash *",
    "fish *",
    "ksh *",
    "sh *",
    "zsh *",
    "cmake *",
    "git *",
    "make *",
    "csplit *",
    "dd *",
    "script *",
    "sort *",
    "split *",
    // `cd /restricted && rm file` would otherwise dodge every Bash deny.
    "cd",
    "cd *",
    "pushd",
    "pushd *",
    // Windows CMD; the lowercased token matches the case-insensitive shells.
    "attrib",
    "attrib *",
    "copy",
    "copy *",
    "del",
    "del *",
    "erase",
    "erase *",
    "fc *",
    "move",
    "move *",
    "rd",
    "rd *",
    "ren",
    "ren *",
    "rename",
    "rename *",
    "robocopy *",
    "type *",
    "xcopy *",
    // PowerShell cmdlets, in their canonical capitalisation.
    "Add-Content",
    "Add-Content *",
    "Clear-Content",
    "Clear-Content *",
    "Copy-Item",
    "Copy-Item *",
    "Move-Item",
    "Move-Item *",
    "New-Item",
    "New-Item *",
    "Out-File",
    "Out-File *",
    "Remove-Item",
    "Remove-Item *",
    "Rename-Item",
    "Rename-Item *",
    "Set-Content",
    "Set-Content *",
];

/// Diagnostic surface returned by [`revert_rules`].
///
/// Manual-edit detection lives here: when the caller deletes a rule
/// from a settings file by hand between `apply_rules` and
/// `revert_rules`, the revert path skips the missing rule and records
/// the omission here so the CLI can surface it without failing the
/// whole reverse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RevertReport {
    pub touched_files: Vec<PathBuf>,
    /// Missing rules and missing files; empty on a clean revert.
    pub warnings: Vec<String>,
}

/// Generated rule strings for one [`ResolvedTrustedRoot`] entry.
///
/// `deny` and `allow` map 1:1 to Claude's `permissions.deny` /
/// `permissions.allow` arrays. Both sides of the sync (apply +
/// reverse) work off this exact set so the round-trip is exact.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RuleSet {
    /// Empty unless the caller asked for dot-folder re-allows through `allow_dot_folders`.
    pub allow: Vec<String>,
    /// In emit order.
    pub deny: Vec<String>,
}

impl RuleSet {
    /// `true` when the set carries neither allow nor deny rules, in which case the restrict path
    /// skips the settings and sidecar write.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }
}

/// Per-settings-file projection of [`apply_rules`].
///
/// Reports the rules that would be appended vs. the rules already
/// present, plus whether the file itself would be created. Pure
/// analysis: no writes. Built by [`simulate_apply_rules`] and
/// consumed by both the live apply path (which uses the
/// `to_add` / `already_present` split for diagnostics) and the
/// `plan restrict` projection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SettingsFileSim {
    pub allow_rules_already_present: Vec<String>,
    pub allow_rules_to_add: Vec<String>,
    pub deny_rules_already_present: Vec<String>,
    pub deny_rules_to_add: Vec<String>,
    /// Every allow rule in the file, whether or not the projection touches it; the conflict
    /// detector reads it.
    pub existing_allow_rules: Vec<String>,
    /// Every deny rule in the file, whether or not the projection touches it.
    pub existing_deny_rules: Vec<String>,
    pub path: PathBuf,
    pub will_be_created: bool,
}

/// The full deny/allow shape set the `PreToolUse` hook covers.
///
/// `remargin restrict` does not write these into settings files. `remargin doctor` compares
/// on-disk deny rules against this set: a rule found there is drift, redundant because the
/// hook enforces it.
///
/// Pure: no filesystem access. The caller must pass the realm anchor
/// (the directory that holds `.claude/`) so wildcard entries can
/// expand to a concrete path glob. `allow_dot_folders` controls which
/// dot-folder names get a re-allow rule on top of the default-deny.
///
/// Wildcards (`TrustedRootPath::Wildcard`) anchor at the entry's
/// `realm_root`; `_anchor` is unused for these entries because the
/// realm root already anchors them. Absolute entries use their own
/// path verbatim.
///
/// Output:
///
/// - Per-tool path denies for every entry in [`EDITOR_TOOLS`]:
///   `Edit/Write/Read/NotebookEdit/MultiEdit(<path>/**)`.
/// - Dot-folder default-deny: same tools against `<path>/.*/**`.
/// - Bash mutators: every entry in [`BASH_MUTATORS`] expands to
///   `Bash(<cmd> <path>/**)`.
/// - mv source-side coverage: `Bash(mv <path>/**)`,
///   `Bash(mv <path>/** *)`, `Bash(mv <path>/** <path>/**)`.
/// - `also_deny_bash` extras: `Bash(<cmd> * <path>/**)` for each
///   user-supplied entry.
/// - Per `allow_dot_folders` entry: per-tool re-allows that override
///   the dot-folder default-deny.
#[must_use]
pub fn hook_covered_rules(
    entry: &ResolvedTrustedRoot,
    _anchor: &Path,
    allow_dot_folders: &[String],
) -> RuleSet {
    let restricted_root = match &entry.path {
        TrustedRootPath::Absolute(path) => path.clone(),
        TrustedRootPath::Wildcard { realm_root } => realm_root.clone(),
    };
    let glob_root = restricted_root.display().to_string();

    let mut deny: Vec<String> = Vec::new();

    // `glob_root` is canonical absolute, so rules are emitted as `Tool(/path/**)`. On-disk rules
    // with a `//` or `///` prefix still match through [`canonicalize_rule`].

    for tool in EDITOR_TOOLS {
        deny.push(format!("{tool}({glob_root}/**)"));
    }

    // One wildcard rule per tool covers every dot-folder under the root, present or future.
    for tool in EDITOR_TOOLS {
        deny.push(format!("{tool}({glob_root}/.*/**)"));
    }

    for cmd in BASH_MUTATORS {
        deny.push(format!("Bash({cmd} {glob_root}/**)"));
    }

    // The `mv *` template covers only the destination side; these shapes cover the bare, the
    // source-side and the both-sides forms.
    deny.push(format!("Bash(mv {glob_root}/**)"));
    deny.push(format!("Bash(mv {glob_root}/** *)"));
    deny.push(format!("Bash(mv {glob_root}/** {glob_root}/**)"));

    // The `cp *` template covers only the destination side; these shapes cover the source side.
    deny.push(format!("Bash(cp {glob_root}/**)"));
    deny.push(format!("Bash(cp {glob_root}/** *)"));
    deny.push(format!("Bash(cp {glob_root}/** {glob_root}/**)"));

    for cmd in &entry.also_deny_bash {
        deny.push(format!("Bash({cmd} * {glob_root}/**)"));
    }

    // No implicit `mcp__remargin__*` allow and no implicit `.remargin/` carve-out: only the
    // folders named in `allow_dot_folders` are re-allowed.
    let mut allow: Vec<String> = Vec::new();
    for folder in allow_dot_folders {
        for tool in EDITOR_TOOLS {
            allow.push(format!("{tool}({glob_root}/{folder}/**)"));
        }
    }

    RuleSet { allow, deny }
}

/// The rules `remargin restrict` writes into settings files: none.
///
/// The `PreToolUse` hook gates every editor tool on any target under a trusted root, honours
/// `allow_dot_folders`, and resolves every path-shaped word of a Bash command whatever the verb,
/// so each shape [`hook_covered_rules`] emits is already enforced. This is a named function so
/// the live `restrict` path and the `plan restrict` projection share one source.
#[must_use]
pub fn residual_rules() -> RuleSet {
    RuleSet::default()
}

/// Pure projection of [`apply_rules`]. Per file in `settings_files`,
/// reports which rules in `rules` would be appended vs. left alone.
/// Does not mutate disk.
///
/// The live [`apply_rules`] path runs this same simulator so the
/// projection reflects the exact set of writes the live path would
/// produce.
///
/// # Errors
///
/// Settings-file read / parse failures (the writer's failure modes
/// are intentionally not exercised here).
pub fn simulate_apply_rules(
    system: &dyn System,
    settings_files: &[PathBuf],
    rules: &RuleSet,
) -> Result<Vec<SettingsFileSim>> {
    let mut sims: Vec<SettingsFileSim> = Vec::with_capacity(settings_files.len());
    for settings_file in settings_files {
        sims.push(simulate_settings_file(system, settings_file, rules)?);
    }
    Ok(sims)
}

fn simulate_settings_file(
    system: &dyn System,
    settings_file: &Path,
    rules: &RuleSet,
) -> Result<SettingsFileSim> {
    let body_opt = system.read_to_string(settings_file).ok();
    let will_be_created = body_opt.is_none();
    let body = body_opt.unwrap_or_default();
    let value: Value = if body.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&body)
            .with_context(|| format!("parsing settings JSON at {}", settings_file.display()))?
    };
    let existing_deny = read_permission_array(&value, "deny");
    let existing_allow = read_permission_array(&value, "allow");

    let (deny_rules_already_present, deny_rules_to_add) =
        partition_rules(&rules.deny, &existing_deny);
    let (allow_rules_already_present, allow_rules_to_add) =
        partition_rules(&rules.allow, &existing_allow);

    Ok(SettingsFileSim {
        allow_rules_already_present,
        allow_rules_to_add,
        deny_rules_already_present,
        deny_rules_to_add,
        existing_allow_rules: existing_allow,
        existing_deny_rules: existing_deny,
        path: settings_file.to_path_buf(),
        will_be_created,
    })
}

fn partition_rules(rules: &[String], existing: &[String]) -> (Vec<String>, Vec<String>) {
    let mut already: Vec<String> = Vec::new();
    let mut to_add: Vec<String> = Vec::new();
    for rule in rules {
        let target = canonicalize_rule(rule);
        if existing.iter().any(|e| canonicalize_rule(e) == target) {
            already.push(rule.clone());
        } else {
            to_add.push(rule.clone());
        }
    }
    (already, to_add)
}

/// Collapse runs of `/` inside a rule string to a single `/`.
///
/// Maps legacy on-disk forms (`Read(//foo/**)`, `Read(///foo/**)`) to
/// the canonical single-slash form (`Read(/foo/**)`) for membership
/// purposes.
///
/// Pure, idempotent. `Bash(curl * //foo/**)` becomes
/// `Bash(curl * /foo/**)`; the cmd tokens themselves are not analysed
/// — `Bash(http://x.example/x /foo/**)` would also collapse the URL,
/// but every Claude rule we emit anchors paths absolutely so the
/// happy-path round-trip is exact.
#[must_use]
pub fn canonicalize_rule(rule: &str) -> String {
    let mut out = String::with_capacity(rule.len());
    let mut prev_slash = false;
    for ch in rule.chars() {
        if ch == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        out.push(ch);
    }
    out
}

fn read_permission_array(value: &Value, key: &str) -> Vec<String> {
    let Some(permissions) = value.get("permissions").and_then(Value::as_object) else {
        return Vec::new();
    };
    let Some(array) = permissions.get(key).and_then(Value::as_array) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}

/// Apply `rules` to every settings file in `settings_files`, updating
/// the sidecar to record exactly what was added.
///
/// Idempotent: rules already present in a settings file are left
/// in place (no duplicates), and the sidecar entry is overwritten
/// with the latest deltas so a subsequent [`revert_rules`] removes
/// the right strings. `added_at` is caller-supplied so callers can
/// pin a value in tests.
///
/// # Errors
///
/// - Settings-file read / parse / write failures.
/// - Sidecar I/O failures (forwarded from [`sidecar::add_entry`]).
pub fn apply_rules(
    system: &dyn System,
    anchor: &Path,
    target_path: &str,
    rules: &RuleSet,
    settings_files: &[PathBuf],
    added_at: &str,
) -> Result<()> {
    for settings_file in settings_files {
        merge_rules_into_settings(system, settings_file, rules)?;
    }

    sidecar::add_entry(
        system,
        anchor,
        target_path,
        SidecarEntry {
            added_at: String::from(added_at),
            added_to_files: settings_files.to_vec(),
            allow: rules.allow.clone(),
            deny: rules.deny.clone(),
        },
    )
}

/// Reverse [`apply_rules`] for `target_path`.
///
/// Looks up the sidecar entry; for each rule string the entry
/// recorded, scrubs that string from each `added_to_files` settings
/// file (skipping silently when the file or the rule is missing —
/// that's the manual-edit case the [`RevertReport`] documents).
/// Removes the sidecar entry on success.
///
/// Returns an empty [`RevertReport`] (no warnings) when the sidecar
/// has no entry for `target_path`. The caller decides whether to
/// surface that as an error or as a soft "nothing to do".
///
/// # Errors
///
/// Sidecar / settings-file I/O failures (read / parse / write).
pub fn revert_rules(system: &dyn System, anchor: &Path, target_path: &str) -> Result<RevertReport> {
    let mut report = RevertReport::default();
    let Some(entry) = sidecar::remove_entry(system, anchor, target_path)? else {
        return Ok(report);
    };

    for settings_file in &entry.added_to_files {
        report.touched_files.push(settings_file.clone());
        let body = match system.read_to_string(settings_file) {
            Ok(body) => body,
            Err(_err) => {
                report.warnings.push(format!(
                    "settings file {} disappeared between apply and revert; skipping",
                    settings_file.display()
                ));
                continue;
            }
        };
        let mut value: Value = match serde_json::from_str(&body) {
            Ok(value) => value,
            Err(err) => {
                report.warnings.push(format!(
                    "settings file {} no longer parses ({err}); skipping",
                    settings_file.display()
                ));
                continue;
            }
        };
        let removed_deny = scrub_permission_array(&mut value, "deny", &entry.deny);
        let removed_allow = scrub_permission_array(&mut value, "allow", &entry.allow);
        for rule in &entry.deny {
            if !removed_deny.contains(rule) {
                report.warnings.push(format!(
                    "deny rule {rule:?} not present in {} (manually removed?)",
                    settings_file.display()
                ));
            }
        }
        for rule in &entry.allow {
            if !removed_allow.contains(rule) {
                report.warnings.push(format!(
                    "allow rule {rule:?} not present in {} (manually removed?)",
                    settings_file.display()
                ));
            }
        }
        write_settings(system, settings_file, &value)?;
    }

    Ok(report)
}

/// Read a settings file (creating an empty `{}` shape when absent),
/// merge `rules` into its `permissions.{deny,allow}` arrays without
/// duplicating, and write the result back. Other top-level keys are
/// preserved verbatim.
fn merge_rules_into_settings(
    system: &dyn System,
    settings_file: &Path,
    rules: &RuleSet,
) -> Result<()> {
    if let Some(parent) = settings_file.parent() {
        system
            .create_dir_all(parent)
            .with_context(|| format!("creating settings directory {}", parent.display()))?;
    }
    let body = system.read_to_string(settings_file).unwrap_or_default();
    let mut value: Value = if body.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&body)
            .with_context(|| format!("parsing settings JSON at {}", settings_file.display()))?
    };

    append_unique_to_permission_array(&mut value, "deny", &rules.deny);
    append_unique_to_permission_array(&mut value, "allow", &rules.allow);

    write_settings(system, settings_file, &value)
}

/// Append every entry in `rules` to `value.permissions.<key>` that is
/// not already present. Creates the `permissions` and array slots if
/// they do not exist. No-op when `value` is not a JSON object.
fn append_unique_to_permission_array(value: &mut Value, key: &str, rules: &[String]) {
    let Some(root) = value.as_object_mut() else {
        return;
    };
    let permissions_value = root
        .entry(String::from("permissions"))
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(permissions) = permissions_value.as_object_mut() else {
        return;
    };
    let key_value = permissions
        .entry(String::from(key))
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(array) = key_value.as_array_mut() else {
        return;
    };
    for rule in rules {
        let target = canonicalize_rule(rule);
        if !array.iter().any(|existing| {
            existing
                .as_str()
                .is_some_and(|e| canonicalize_rule(e) == target)
        }) {
            array.push(Value::String(rule.clone()));
        }
    }
}

/// Remove every entry in `rules` from `value.permissions.<key>`,
/// returning the rules that were actually removed (so the caller can
/// detect manual deletions).
fn scrub_permission_array(value: &mut Value, key: &str, rules: &[String]) -> Vec<String> {
    let mut removed: Vec<String> = Vec::new();
    let Some(permissions) = value.get_mut("permissions").and_then(Value::as_object_mut) else {
        return removed;
    };
    let Some(array) = permissions.get_mut(key).and_then(Value::as_array_mut) else {
        return removed;
    };
    for rule in rules {
        let target = canonicalize_rule(rule);
        if let Some(idx) = array.iter().position(|existing| {
            existing
                .as_str()
                .is_some_and(|e| canonicalize_rule(e) == target)
        }) {
            let _: Value = array.remove(idx);
            removed.push(rule.clone());
        }
    }
    removed
}

fn write_settings(system: &dyn System, settings_file: &Path, value: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(value).context("serializing settings JSON")?;
    let mut bytes = body.into_bytes();
    bytes.push(b'\n');
    system
        .write(settings_file, &bytes)
        .with_context(|| format!("writing settings to {}", settings_file.display()))
}
