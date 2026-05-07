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
    ResolvedPermissions, TrustedRootPath, resolve_permissions,
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
    /// `true` when any `restrict` or `deny_ops` rule covers the path.
    pub restricted: bool,
}

/// Serialised view of a single `deny_ops` declaration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct DenyOpsView {
    pub ops: Vec<String>,
    pub path: PathBuf,
    pub source_file: PathBuf,
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
/// declared by `restrict`, or covered by a `deny_ops` entry. Routes
/// through the same predicates the per-op guard uses so the two layers
/// cannot drift.
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
        let op_names: Vec<&str> = entry.ops.iter().map(|op| op.as_str()).collect();
        return Some(MatchingRule {
            kind: "deny_ops",
            rule_text: format!(
                "deny_ops {{ path: {}, ops: {op_names:?} }}",
                entry.path.display(),
            ),
            source_file: entry.source_file.clone(),
        });
    }

    if !target_is_sanctioned(canonical, &resolved.trusted_roots)
        && let Some(first) = resolved.trusted_roots.first()
    {
        return Some(MatchingRule {
            kind: "trusted_roots",
            rule_text: format!(
                "outside trusted_roots (target {} is not inside any entry)",
                canonical.display(),
            ),
            source_file: first.source_file.clone(),
        });
    }

    None
}

fn group_allow_dot_folders(resolved: &ResolvedPermissions) -> Vec<AllowDotFoldersView> {
    // Per rem-qdrw the resolver now preserves one entry per declaring
    // `.remargin.yaml` so each view's `source_file` mirrors the
    // provenance already carried by `restrict` and `deny_ops`.
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
                .map(|op| String::from(op.as_str()))
                .collect(),
            path: entry.path.clone(),
            source_file: entry.source_file.clone(),
        })
        .collect()
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
