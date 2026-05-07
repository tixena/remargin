//! Claude-settings synchronizer rule generation (rem-yj1j.4 / T25,
//! slice 1 — `rem-wv71`; minimised by `rem-egp9` slice A; native-tool
//! fences restored by `rem-qjqu`).
//!
//! [`rules_for`] is a pure function over a [`ResolvedTrustedRoot`] +
//! anchor + `allow_dot_folders` list. Given those inputs it produces
//! the exact `permissions.deny` / `permissions.allow` rule strings
//! that the Claude-settings merger (slice 3, `rem-7m4u`) will write
//! into `.claude/settings.local.json` and `~/.claude/settings.json`.
//!
//! ## Output shape
//!
//! `<path>` below is a single-leading-slash absolute path glob —
//! Claude's documented form. Earlier revisions emitted `//<path>` /
//! `///<path>` (rem-em33); legacy on-disk rules in either of those
//! forms are still recognised for membership / overlap purposes via
//! [`canonicalize_rule`].
//!
//! ```text
//! deny:
//!   Edit(<path>/**)                              ← editor-tool fence
//!   Write(<path>/**)
//!   Read(<path>/**)
//!   NotebookEdit(<path>/**)
//!   Edit(<path>/.*/**)                           ← dot-folder default-deny
//!   Write(<path>/.*/**)                            (one wildcard rule per
//!   Read(<path>/.*/**)                             editor tool; suppressed
//!   NotebookEdit(<path>/.*/**)                     for dot-folders named
//!                                                  in allow_dot_folders)
//!   Bash(<cmd> [*] <path>/**)                    ← every BASH_MUTATORS
//!                                                  entry: file-mutating
//!                                                  surface (delete /
//!                                                  create / link / metadata
//!                                                  / editors / scriptable
//!                                                  interpreters / archives
//!                                                  / sync / patch /
//!                                                  downloads / shells /
//!                                                  VCS / build / disk /
//!                                                  navigation cd-pushd)
//!   Bash(mv <path>/**)                           ← mv source-side coverage
//!   Bash(mv <path>/** *)                           (rem-0j2x / T44)
//!   Bash(mv <path>/** <path>/**)
//!   <per also_deny_bash entry, Bash(<cmd> * <path>/**)>
//!   Bash(remargin *)                             ← only when cli_allowed=false
//!
//! allow:
//!   <per allow_dot_folders entry, RE-allow rules>   ← only emitted
//!                                                     when the user
//!                                                     explicitly names
//!                                                     a dot-folder
//! ```
//!
//! ## Why a single wildcard for dot-folder denies
//!
//! The spec proposed two options: enumerate every `.<name>/` under the
//! path, or emit one wildcard `.*` rule. Walking the filesystem at
//! rule-generation time is expensive AND races against folder
//! creation. A single `.*/**` wildcard rule covers all current and
//! future dot-folders without filesystem access. When
//! `allow_dot_folders` lists specific names that should remain
//! reachable (e.g. `.github`), we add narrow re-allows that override
//! the broader deny — Claude's permission resolution gives the more-
//! specific allow precedence.
//!
//! ## remargin CLI deny (`Bash(remargin *)`)
//!
//! The `remargin` CLI deny is emitted as a coarse global rule with
//! no path tail (rem-egp9 slice A keeper). Path-shape-independent —
//! tilde, `$HOME`, relative paths, and implicit-cwd subcommands
//! cannot evade it because there is no path on the command line for
//! the matcher to compare against. `cli_allowed: true` skips this
//! single rule; the editor-tool / Bash-mutator fences above still
//! emit so native-tool routes remain blocked even when the human is
//! authorised to invoke remargin from a shell.
//!
//! ## No automatic `mcp__remargin__*` allow (rem-si27)
//!
//! Earlier revisions prepended `mcp__remargin__*` to every projected
//! `allow` list so that a blanket `restrict` could not lock the user out
//! of the very tools needed to call `unprotect`. That carve-out is
//! gone: under `restrict` the user wants Claude to prompt before each
//! MCP call to remargin. Users who want silent forwarding can opt in
//! by adding `mcp__remargin__*` to `.claude/settings.local.json`
//! themselves — `restrict` no longer does it for them.
//!
//! ## No filesystem access
//!
//! Every input is materialised by the caller; this module produces
//! `Vec<String>` only. That keeps it trivially testable with
//! `MockSystem` not even needed — pure data in, pure data out.

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

/// Editor-side Claude tools touched by the base path-deny and the
/// dot-folder default-deny. Order matches the spec's example output
/// (Edit / Write / Read / `NotebookEdit`) so settings-file diffs read
/// the way users expect.
const EDITOR_TOOLS: &[&str] = &["Edit", "Write", "Read", "NotebookEdit"];

/// Default-deny Bash command tokens for the restricted path
/// (rem-p74a). Every entry expands to `Bash(<token> {glob_root}/**)`,
/// so a token of `cp *` becomes `Bash(cp * /path/**)` while a bare
/// `tee` becomes `Bash(tee /path/**)`. The trailing `*` (or its
/// absence) is part of the token by design — the format string in
/// [`rules_for`] does NOT inject one.
///
/// The list is broad on purpose: most entries below can read, modify,
/// create, delete, or otherwise mutate a file on disk, which would
/// defeat the MCP-only contract `restrict` is supposed to enforce.
/// `cd` / `pushd` (rem-e6yd / T42) are non-mutating but close the
/// shell-relative bypass — `cd /restricted && rm file` would
/// otherwise route around every other rule because `rm`'s argv would
/// no longer carry the restricted path. Users can layer extra denies
/// on top via `--also-deny-bash`; the purpose of THIS list is to
/// make the defaults safe-by-default so an agent cannot trivially
/// bypass the restriction with a forgotten command.
///
/// Ordering: original write-side mutators first (preserves
/// rule-emission order with older settings files), then the new
/// categories grouped by intent. Within each category, order is
/// alphabetical-ish for human scanability, not load-bearing.
///
/// `sed` appears twice on purpose: legacy `sed -i *` is preserved so
/// repeat runs do not shuffle rule order or churn the sidecar, and
/// plain `sed *` is added alongside to cover redirection-based writes
/// (`sed ... > /restricted/file`) that escape `-i`.
///
/// `cd` / `pushd` each appear twice (`cd` and `cd *`) to match both
/// the bare form (`cd /path/notes`) and the with-flag form
/// (`cd -P /path/notes`), since the matcher needs the path to land in
/// the trailing position with no fixed-token prefix.
pub(crate) const BASH_MUTATORS: &[&str] = &[
    // Write-side mutators (original surface).
    "cp *",
    "mv *",
    "tee",
    "tee *",
    "sed -i *",
    "sed *",
    "truncate *",
    "touch",
    "touch *",
    // Delete.
    "rm *",
    "rmdir *",
    "unlink *",
    // Create / link.
    "install *",
    "ln *",
    "mkdir *",
    "mkfifo *",
    "mknod *",
    // Metadata / permissions.
    "chattr *",
    "chgrp *",
    "chmod *",
    "chown *",
    "setfacl *",
    // Interactive editors.
    "ed *",
    "emacs *",
    "micro *",
    "nano *",
    "nvim *",
    "vi *",
    "vim *",
    // Scriptable interpreters (can write any file).
    "awk *",
    "lua *",
    "node *",
    "perl *",
    "php *",
    "python *",
    "python3 *",
    "ruby *",
    // Archives.
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
    // Sync / remote copy.
    "rsync *",
    "scp *",
    "sftp *",
    // Patch.
    "patch *",
    // Network downloads.
    "curl *",
    "wget *",
    // Shells (can do anything).
    "bash *",
    "dash *",
    "fish *",
    "ksh *",
    "sh *",
    "zsh *",
    // VCS / build.
    "cmake *",
    "git *",
    "make *",
    // Disk / write.
    "csplit *",
    "dd *",
    "script *",
    "sort *",
    "split *",
    // Directory navigation (rem-e6yd / T42). Closes the
    // shell-relative bypass: `cd /restricted && rm file` would
    // otherwise dodge every Bash deny because `rm`'s argv carries
    // only `file`. Both bare and with-flag forms emitted.
    "cd",
    "cd *",
    "pushd",
    "pushd *",
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
    /// Files the revert opened. Useful for the CLI to print "removed
    /// rules from N file(s)".
    pub touched_files: Vec<PathBuf>,
    /// Human-readable diagnostics: missing rules, missing files, etc.
    /// Empty on the clean-revert happy path.
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
    /// `permissions.allow` rules. Empty by default (rem-si27);
    /// populated only with the per-dot-folder editor-tool re-allows
    /// the caller requested via `allow_dot_folders`.
    pub allow: Vec<String>,
    /// `permissions.deny` rules in emit order.
    pub deny: Vec<String>,
}

/// Per-settings-file projection of [`apply_rules`].
///
/// Reports the rules that would be appended vs. the rules already
/// present, plus whether the file itself would be created. Pure
/// analysis: no writes. Built by [`simulate_apply_rules`] and
/// consumed by both the live apply path (which uses the
/// `to_add` / `already_present` split for diagnostics) and the
/// `plan restrict` projection (rem-puy5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SettingsFileSim {
    /// Allow rules (subset of [`RuleSet::allow`]) already present in
    /// the settings file's `permissions.allow` array.
    pub allow_rules_already_present: Vec<String>,
    /// Allow rules (subset of [`RuleSet::allow`]) that would be
    /// appended.
    pub allow_rules_to_add: Vec<String>,
    /// Deny rules (subset of [`RuleSet::deny`]) already present in
    /// the settings file's `permissions.deny` array.
    pub deny_rules_already_present: Vec<String>,
    /// Deny rules (subset of [`RuleSet::deny`]) that would be
    /// appended.
    pub deny_rules_to_add: Vec<String>,
    /// Allow rules already in the settings file's `permissions.allow`
    /// array regardless of whether the projection touches them. Used
    /// by the conflict detector to surface allow-vs-deny overlap.
    pub existing_allow_rules: Vec<String>,
    /// Deny rules already in the settings file's `permissions.deny`
    /// array regardless of whether the projection touches them.
    pub existing_deny_rules: Vec<String>,
    /// Settings file path the simulation reports on.
    pub path: PathBuf,
    /// `true` when the settings file does not exist on disk.
    pub will_be_created: bool,
}

/// Compute the rule set for one resolved restrict entry.
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
/// Output (per the restored projection — `rem-qjqu`):
///
/// - Per-tool path denies for every entry in [`EDITOR_TOOLS`]:
///   `Edit/Write/Read/NotebookEdit(<path>/**)`.
/// - Dot-folder default-deny: same four tools against `<path>/.*/**`.
/// - Bash mutators: every entry in [`BASH_MUTATORS`] expands to
///   `Bash(<cmd> <path>/**)`.
/// - mv source-side coverage (T44): `Bash(mv <path>/**)`,
///   `Bash(mv <path>/** *)`, `Bash(mv <path>/** <path>/**)`.
/// - `also_deny_bash` extras: `Bash(<cmd> * <path>/**)` for each
///   user-supplied entry.
/// - When `cli_allowed == false`, the global `Bash(remargin *)` deny
///   (no path tail — slice A keeper). `cli_allowed == true` skips it.
/// - Per `allow_dot_folders` entry: per-tool re-allows that override
///   the dot-folder default-deny.
///
/// `mcp__remargin__*` is NOT auto-emitted on the allow side
/// (`rem-si27`); the user opts in if they want silent MCP forwarding.
#[must_use]
pub fn rules_for(
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

    // `glob_root` is canonical absolute (leading `/`). Format strings
    // therefore emit `Tool(/path/**)` directly — no extra `//` prefix
    // (rem-em33). Legacy on-disk rules with the older `//` / `///`
    // prefix still match for membership purposes via
    // [`canonicalize_rule`].

    // 1. Base read/write tool denies — the editor-side defenses.
    for tool in EDITOR_TOOLS {
        deny.push(format!("{tool}({glob_root}/**)"));
    }

    // 2. Dot-folder default-deny. A single wildcard rule per tool
    //    covers every current and future dot-folder under the
    //    restricted root; specific allows below override.
    for tool in EDITOR_TOOLS {
        deny.push(format!("{tool}({glob_root}/.*/**)"));
    }

    // 3. Bash mutators — keep shell-out paths from dodging the
    //    editor-tool denies.
    for cmd in BASH_MUTATORS {
        deny.push(format!("Bash({cmd} {glob_root}/**)"));
    }

    // 3a. Source-side `mv` coverage (rem-0j2x / T44). The `mv *`
    //     template above only emits the destination-side pattern
    //     (`Bash(mv * /path/**)`). The remaining shapes — bare
    //     single-arg, source-side, and both-sides — close the
    //     exfiltration / accidental-source-move surface. Agents that
    //     legitimately need to move a tracked file under a restricted
    //     realm route through `mcp__remargin__mv` (which the user
    //     must opt in to allowing — rem-si27 dropped the auto-allow);
    //     humans with `cli_allowed: true` fall back to `remargin mv`.
    deny.push(format!("Bash(mv {glob_root}/**)"));
    deny.push(format!("Bash(mv {glob_root}/** *)"));
    deny.push(format!("Bash(mv {glob_root}/** {glob_root}/**)"));

    // 4. Caller-supplied bash extras, e.g. `also_deny_bash: [curl]`.
    for cmd in &entry.also_deny_bash {
        deny.push(format!("Bash({cmd} * {glob_root}/**)"));
    }

    // 5. Block remargin CLI invocations globally when `cli_allowed` is
    //    false (rem-egp9 slice A keeper). No path tail — the matcher
    //    cannot be dodged with tilde / `$HOME` / relative paths
    //    because there is no path on the command line to evade.
    //    `op_guard` still handles per-target enforcement.
    if !entry.cli_allowed {
        deny.push(String::from("Bash(remargin *)"));
    }

    // 6. Allow list. Empty by default (rem-si27 dropped the implicit
    //    `mcp__remargin__*` allow so users keep per-call oversight of
    //    remargin's MCP tools under a blanket restrict). Per-dot-folder
    //    re-allows override the default-deny ONLY for folders the user
    //    explicitly listed in `allow_dot_folders` (rem-2plr — no
    //    implicit `.remargin/` carve-out either).
    let mut allow: Vec<String> = Vec::new();
    for folder in allow_dot_folders {
        for tool in EDITOR_TOOLS {
            allow.push(format!("{tool}({glob_root}/{folder}/**)"));
        }
    }

    RuleSet { allow, deny }
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
/// purposes (rem-em33).
///
/// Pure, idempotent. `Bash(curl * //foo/**)` becomes
/// `Bash(curl * /foo/**)`; the cmd tokens themselves are not analysed
/// — `Bash(http://x.example/x  /foo/**)` would also collapse the URL,
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
