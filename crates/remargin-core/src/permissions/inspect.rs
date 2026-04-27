//! Inspection helpers behind `remargin permissions show / check`
//! (rem-yj1j.7 / T28).
//!
//! Both functions are pure given an [`os_shim::System`]; the CLI /
//! MCP surfaces sit on top of these and add only argument parsing and
//! output formatting.
//!
//! ## `show`
//!
//! Walks `.remargin.yaml` from `cwd`, accumulates the resolved
//! permissions, and groups them into a JSON-serialisable
//! [`ShowOutput`]. When a `trusted_roots` entry is itself the parent
//! of a `.remargin.yaml`, that realm's permissions are expanded
//! recursively and attached to the entry's `recursive` field. Cycle
//! detection uses a visited-set of canonical paths and a hard depth
//! cap.
//!
//! ## `check`
//!
//! Gitignore-style: returns `restricted = true` when the path is
//! covered by any `restrict` entry OR by any `deny_ops` entry. With
//! `--why`, the closest matching rule is named with its source file
//! and a human-readable rule text.
//!
//! ## Canonical `permissions show --json` schema (rem-k7e5)
//!
//! The shape below is the contract for `remargin permissions show
//! --json` and the MCP `permissions_show` tool. The Rust types in
//! this module ([`ShowOutput`], [`RestrictView`], [`DenyOpsView`],
//! [`AllowDotFoldersView`], [`TrustedRootView`]) are the
//! single-source-of-truth — `permissions_show_json_shape_is_canonical`
//! in `tests/cli_permissions.rs` deserialises real CLI output into
//! `#[serde(deny_unknown_fields)]` mirrors and fails the build if a
//! field is added without updating this schema. `elapsed_ms` is
//! injected at the surface (CLI / MCP wrapper) and is therefore
//! NOT part of the [`ShowOutput`] struct in this module.
//!
//! ```text
//! ShowOutput
//!   allow_dot_folders : Array<AllowDotFoldersView>
//!   deny_ops          : Array<DenyOpsView>
//!   restrict          : Array<RestrictView>
//!   trusted_roots     : Array<TrustedRootView>
//!   elapsed_ms        : number   -- injected by the CLI / MCP layer
//!
//! RestrictView
//!   absolute_path  : string | null   -- canonicalised path; null for `path: '*'`
//!   also_deny_bash : Array<string>   -- bash-token deny list, defaults to []
//!   cli_allowed    : boolean         -- whether `remargin` itself is allowed to read
//!   path_text      : string          -- exact yaml `path:` text ("src/secret" or "*")
//!   realm_root     : string | null   -- non-null only for the `path: '*'` wildcard
//!   source_file    : string          -- absolute path of the .remargin.yaml that declared it
//!
//! DenyOpsView
//!   ops         : Array<string>      -- e.g. ["purge", "rm"]
//!   path        : string             -- canonical path the rule covers
//!   source_file : string
//!
//! AllowDotFoldersView
//!   names       : Array<string>      -- the dot-folder basenames allowed (".obsidian", ...)
//!   source_file : string
//!
//! TrustedRootView
//!   path        : string             -- canonical absolute path of the trusted root
//!   recursive   : ShowOutput | null  -- nested realm permissions; null when not anchored
//!   source_file : string
//! ```
//!
//! `permissions check --json` returns a separate [`CheckOutput`]
//! shape (`matching_rule`, `path`, `restricted`) and is documented
//! on that struct directly.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use serde::Serialize;

use crate::config::permissions::resolve::{ResolvedPermissions, RestrictPath, resolve_permissions};
use crate::permissions::op_guard::restrict_covers;

/// Maximum depth for recursive `trusted_roots` expansion. Stops the
/// resolver from running away if a chain ever reaches into itself.
pub const MAX_RECURSION_DEPTH: usize = 3;

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

/// Serialised view of a single `restrict` declaration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestrictView {
    /// Canonical absolute path; `None` for wildcard entries.
    pub absolute_path: Option<PathBuf>,
    pub also_deny_bash: Vec<String>,
    pub cli_allowed: bool,
    /// On-disk text from `.remargin.yaml` — `"src/secret"` or `"*"`.
    pub path_text: String,
    /// Anchor directory for wildcard entries; `None` otherwise.
    pub realm_root: Option<PathBuf>,
    pub source_file: PathBuf,
}

/// Top-level output of `show()`. Serialises directly to the JSON
/// payload returned by the CLI / MCP.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShowOutput {
    pub allow_dot_folders: Vec<AllowDotFoldersView>,
    pub deny_ops: Vec<DenyOpsView>,
    pub restrict: Vec<RestrictView>,
    pub trusted_roots: Vec<TrustedRootView>,
}

/// Serialised view of a single `trusted_roots` declaration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrustedRootView {
    pub path: PathBuf,
    /// Permissions inside `path` when it itself anchors a realm
    /// (i.e. holds a `.remargin.yaml`). Bounded by
    /// [`MAX_RECURSION_DEPTH`]; cycles are detected and stop expansion.
    pub recursive: Option<Box<ShowOutput>>,
    pub source_file: PathBuf,
}

/// Run `permissions check`: gitignore-style coverage test for `path`.
///
/// `cwd` drives the parent walk that picks up applicable `.remargin.yaml`
/// files. `path` is canonicalised through the [`os_shim::System`] before
/// matching. When `why` is `true`, the returned [`CheckOutput`] carries
/// the closest matching rule (closest = first in the resolver's
/// deepest-first list).
///
/// # Errors
///
/// Forwards I/O / parse failures from
/// [`crate::config::permissions::resolve::resolve_permissions`].
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

/// Run `permissions show`: parent-walk + recursive trusted-root
/// expansion.
///
/// # Errors
///
/// Forwards I/O / parse failures from
/// [`crate::config::permissions::resolve::resolve_permissions`].
pub fn show(system: &dyn System, cwd: &Path) -> Result<ShowOutput> {
    let mut visited: Vec<PathBuf> = Vec::new();
    show_inner(system, cwd, 0, &mut visited)
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
        .restrict
        .iter()
        .find(|entry| restrict_covers(&entry.path, canonical))
    {
        let rule_text = match &entry.path {
            RestrictPath::Absolute(path) => format!("restrict path {}", path.display()),
            RestrictPath::Wildcard { realm_root } => {
                format!("restrict wildcard under realm {}", realm_root.display())
            }
        };
        return Some(MatchingRule {
            kind: "restrict",
            rule_text,
            source_file: entry.source_file.clone(),
        });
    }

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

fn group_restrict(resolved: &ResolvedPermissions) -> Vec<RestrictView> {
    resolved
        .restrict
        .iter()
        .map(|entry| match &entry.path {
            RestrictPath::Absolute(path) => RestrictView {
                absolute_path: Some(path.clone()),
                also_deny_bash: entry.also_deny_bash.clone(),
                cli_allowed: entry.cli_allowed,
                path_text: path.display().to_string(),
                realm_root: None,
                source_file: entry.source_file.clone(),
            },
            RestrictPath::Wildcard { realm_root } => RestrictView {
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

fn group_trusted_roots(
    system: &dyn System,
    resolved: &ResolvedPermissions,
    depth: usize,
    visited: &mut Vec<PathBuf>,
) -> Result<Vec<TrustedRootView>> {
    let mut out = Vec::with_capacity(resolved.trusted_roots.len());
    for entry in &resolved.trusted_roots {
        let recursive = recursive_for_trusted_root(system, &entry.path, depth, visited)?;
        out.push(TrustedRootView {
            path: entry.path.clone(),
            recursive,
            source_file: entry.source_file.clone(),
        });
    }
    Ok(out)
}

fn recursive_for_trusted_root(
    system: &dyn System,
    candidate: &Path,
    depth: usize,
    visited: &mut Vec<PathBuf>,
) -> Result<Option<Box<ShowOutput>>> {
    if depth + 1 >= MAX_RECURSION_DEPTH {
        return Ok(None);
    }
    let canonical = system
        .canonicalize(candidate)
        .unwrap_or_else(|_err| candidate.to_path_buf());
    if visited.iter().any(|seen| seen == &canonical) {
        return Ok(None);
    }
    let inner_config = candidate.join(".remargin.yaml");
    let exists = system.exists(&inner_config).unwrap_or(false);
    if !exists {
        return Ok(None);
    }
    visited.push(canonical);
    let nested = show_inner(system, candidate, depth + 1, visited)?;
    Ok(Some(Box::new(nested)))
}

fn show_inner(
    system: &dyn System,
    cwd: &Path,
    depth: usize,
    visited: &mut Vec<PathBuf>,
) -> Result<ShowOutput> {
    let resolved = resolve_permissions(system, cwd)?;
    Ok(ShowOutput {
        allow_dot_folders: group_allow_dot_folders(&resolved),
        deny_ops: group_deny_ops(&resolved),
        restrict: group_restrict(&resolved),
        trusted_roots: group_trusted_roots(system, &resolved, depth, visited)?,
    })
}
