//! `permissions show` / `permissions check`. `check` routes through the
//! same `target_is_sanctioned` predicate the per-op guard uses so the
//! two layers cannot drift.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use serde::Serialize;

use crate::config::permissions::resolve::{
    ResolvedDenyOpsItem, ResolvedPermissions, TrustedRootPath, resolve_permissions,
};
use crate::permissions::op_guard::target_is_sanctioned;

/// Serialised view of a single `allow_dot_folders` declaration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct AllowDotFoldersView {
    pub names: Vec<String>,
    pub source_file: PathBuf,
}

/// Serialised view of a `check()` result.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckOutput {
    /// Set when the caller asked `--why` AND the path is restricted.
    pub matching_rule: Option<MatchingRule>,
    /// Canonical absolute path that was evaluated.
    pub path: PathBuf,
    /// `true` when the path is outside the `trusted_roots` allow-list
    /// or covered by a `deny_ops` rule.
    pub restricted: bool,
}

/// Serialised view of a single `deny_ops` declaration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct DenyOpsView {
    pub ops: Vec<DenyOpsItemView>,
    pub path: PathBuf,
    pub source_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct DenyOpsItemView {
    pub exceptions: Vec<String>,
    pub name: String,
}

/// Description of the rule that caused a `check()` to report
/// `restricted = true`. Present only when `--why` was passed and a
/// match was found.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct MatchingRule {
    /// Either `"restrict"` or `"deny_ops"`.
    pub kind: &'static str,
    /// Human-readable form of the matching rule.
    pub rule_text: String,
    /// `.remargin.yaml` that declared the rule.
    pub source_file: PathBuf,
}

/// Serialised view of a single `trusted_roots` declaration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrustedRootView {
    /// Canonical absolute path; `None` for wildcard entries.
    pub absolute_path: Option<PathBuf>,
    pub also_deny_bash: Vec<String>,
    pub cli_allowed: bool,
    /// On-disk text — `"src/secret"`, `"*"`, etc.
    pub path_text: String,
    /// Anchor directory for wildcard entries; `None` otherwise.
    pub realm_root: Option<PathBuf>,
    pub source_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShowOutput {
    pub allow_dot_folders: Vec<AllowDotFoldersView>,
    pub deny_ops: Vec<DenyOpsView>,
    pub trusted_roots: Vec<TrustedRootView>,
}

/// `permissions check`: would `op_guard` refuse a mutating op on `path`?
///
/// Returns `restricted=true` when the path is outside the allow-list
/// declared by `trusted_roots`, or covered by a `deny_ops` entry.
/// Routes through the same predicates the per-op guard uses so the
/// two layers cannot drift.
///
/// # Errors
///
/// Forwards I/O / parse failures from `resolve_permissions`.
pub fn check(system: &dyn System, cwd: &Path, path: &Path, why: bool) -> Result<CheckOutput> {
    let canonical = system
        .canonicalize(path)
        .unwrap_or_else(|_err| absolutise(cwd, path));
    let resolved = resolve_permissions(system, cwd)?;
    let matching_rule = first_matching_rule(&resolved, &canonical);
    Ok(CheckOutput {
        matching_rule: if why { matching_rule.clone() } else { None },
        path: canonical,
        restricted: matching_rule.is_some(),
    })
}

/// `permissions show`: dump the resolved permissions for `cwd`.
///
/// # Errors
///
/// Forwards I/O / parse failures from `resolve_permissions`.
pub fn show(system: &dyn System, cwd: &Path) -> Result<ShowOutput> {
    let resolved = resolve_permissions(system, cwd)?;
    Ok(ShowOutput {
        allow_dot_folders: group_allow_dot_folders(&resolved),
        deny_ops: group_deny_ops(&resolved),
        trusted_roots: group_trusted_roots(&resolved),
    })
}

fn absolutise(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn first_matching_rule(resolved: &ResolvedPermissions, canonical: &Path) -> Option<MatchingRule> {
    if let Some(entry) = resolved
        .deny_ops
        .iter()
        .find(|entry| canonical == entry.path || canonical.starts_with(&entry.path))
    {
        let rendered: Vec<String> = entry.ops.iter().map(render_deny_ops_item).collect();
        return Some(MatchingRule {
            kind: "deny_ops",
            rule_text: format!(
                "deny_ops {{ path: {}, ops: [{}] }}",
                entry.path.display(),
                rendered.join(", "),
            ),
            source_file: entry.source_file.clone(),
        });
    }

    // Routes through the same predicate as the per-op guard so the
    // two layers can't drift on the covers-an-entry rule.
    if !resolved.trusted_roots_unconstrained() {
        let inside = !resolved.trusted_roots.is_empty()
            && target_is_sanctioned(canonical, &resolved.trusted_roots);
        if !inside
            && let Some(source) = resolved
                .trusted_roots
                .first()
                .map(|entry| entry.source_file.clone())
                .or_else(|| resolved.trusted_roots_lock.clone())
        {
            return Some(MatchingRule {
                kind: "trusted_roots",
                rule_text: format!(
                    "outside trusted_roots (target {} is not inside any entry)",
                    canonical.display(),
                ),
                source_file: source,
            });
        }
    }

    None
}

fn group_allow_dot_folders(resolved: &ResolvedPermissions) -> Vec<AllowDotFoldersView> {
    // the resolver now preserves one entry per declaring
    // `.remargin.yaml` so each view's `source_file` mirrors the
    // provenance already carried by `trusted_roots` and `deny_ops`.
    resolved
        .allow_dot_folders
        .iter()
        .map(|entry| AllowDotFoldersView {
            names: entry.names.clone(),
            source_file: entry.source_file.clone(),
        })
        .collect()
}

fn group_deny_ops(resolved: &ResolvedPermissions) -> Vec<DenyOpsView> {
    resolved
        .deny_ops
        .iter()
        .map(|entry| DenyOpsView {
            ops: entry
                .ops
                .iter()
                .map(|item| DenyOpsItemView {
                    exceptions: item.exceptions.clone(),
                    name: String::from(item.name.as_str()),
                })
                .collect(),
            path: entry.path.clone(),
            source_file: entry.source_file.clone(),
        })
        .collect()
}

fn render_deny_ops_item(item: &ResolvedDenyOpsItem) -> String {
    if item.exceptions.is_empty() {
        String::from(item.name.as_str())
    } else {
        format!(
            "{} (except {})",
            item.name.as_str(),
            item.exceptions.join(", "),
        )
    }
}

fn group_trusted_roots(resolved: &ResolvedPermissions) -> Vec<TrustedRootView> {
    resolved
        .trusted_roots
        .iter()
        .map(|entry| match &entry.path {
            TrustedRootPath::Absolute(path) => TrustedRootView {
                absolute_path: Some(path.clone()),
                also_deny_bash: entry.also_deny_bash.clone(),
                cli_allowed: entry.cli_allowed,
                path_text: path.display().to_string(),
                realm_root: None,
                source_file: entry.source_file.clone(),
            },
            TrustedRootPath::Wildcard { realm_root } => TrustedRootView {
                absolute_path: None,
                also_deny_bash: entry.also_deny_bash.clone(),
                cli_allowed: entry.cli_allowed,
                path_text: String::from("*"),
                realm_root: Some(realm_root.clone()),
                source_file: entry.source_file.clone(),
            },
        })
        .collect()
}

/// Render a [`ShowOutput`] as human-readable text for `permissions show`.
///
/// Output is written to stderr in the CLI; the function returns a `String`
/// so callers can route it to any sink.
#[must_use]
pub fn render_show_text(cwd: &Path, report: &ShowOutput) -> String {
    use crate::display::format_string_list;
    use core::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Permissions resolved at {}:", cwd.display());
    let _ = writeln!(out);
    let _ = writeln!(out, "  trusted_roots:");
    if report.trusted_roots.is_empty() {
        let _ = writeln!(out, "    (none)");
    } else {
        for entry in &report.trusted_roots {
            let _ = writeln!(
                out,
                "    {}  (source: {})",
                entry.path_text,
                entry.source_file.display(),
            );
            if let Some(realm) = entry.realm_root.as_deref() {
                let _ = writeln!(out, "      realm_root: {}", realm.display());
            }
            if !entry.also_deny_bash.is_empty() {
                let _ = writeln!(
                    out,
                    "      also_deny_bash: {}",
                    format_string_list(&entry.also_deny_bash),
                );
            }
            let _ = writeln!(out, "      cli_allowed: {}", entry.cli_allowed);
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "  deny_ops:");
    if report.deny_ops.is_empty() {
        let _ = writeln!(out, "    (none)");
    } else {
        for entry in &report.deny_ops {
            let _ = writeln!(
                out,
                "    {}  (source: {})",
                entry.path.display(),
                entry.source_file.display(),
            );
            for item in &entry.ops {
                if item.exceptions.is_empty() {
                    let _ = writeln!(out, "      - {}", item.name);
                } else {
                    let _ = writeln!(
                        out,
                        "      - {} (exceptions: {})",
                        item.name,
                        format_string_list(&item.exceptions),
                    );
                }
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "  allow_dot_folders:");
    if report.allow_dot_folders.is_empty() {
        let _ = writeln!(out, "    (none)");
    } else {
        for entry in &report.allow_dot_folders {
            let _ = writeln!(out, "    {}", format_string_list(&entry.names));
        }
    }
    out
}

/// Render a [`CheckOutput`] as human-readable text for `permissions check`.
#[must_use]
pub fn render_check_text(report: &CheckOutput, why: bool) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "restricted: {}", report.restricted);
    if why && let Some(rule) = &report.matching_rule {
        let _ = writeln!(out, "  matched: {}", rule.rule_text);
        let _ = writeln!(out, "  kind:    {}", rule.kind);
        let _ = writeln!(out, "  source:  {}", rule.source_file.display());
    }
    out
}
