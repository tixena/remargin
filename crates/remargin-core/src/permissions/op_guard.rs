//! Per-op permission guard for Layer 1 enforcement (rem-yj1j.2 / T23).
//!
//! The single entry point is [`pre_mutate_check`]. It runs the parent-
//! walk resolver (T22), then evaluates `restrict`, `deny_ops`, and the
//! dot-folder default-deny against the op's target. Any match returns a
//! structured [`OpGuardError`] that names the offending rule and source
//! file.
//!
//! ## Design choices
//!
//! - **Per-op resolution** — no caching. The walk runs every call so
//!   `.remargin.yaml` edits take effect immediately.
//! - **Mutating-only `restrict`** — read-side ops are not affected. To
//!   block reads, declare an explicit `deny_ops` entry.
//! - **Dot-folder default-deny under restrict** — once a `restrict`
//!   covers a path, paths inside un-listed dot-folders below it are
//!   refused too. This keeps `.git/`, `.cache/`, etc. out of the
//!   blast radius unless the user explicitly opts them in.
//! - **`.remargin/` always allowed** — remargin owns this folder; the
//!   dot-folder default-deny never fires on it.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use thiserror::Error;

use crate::config::permissions::resolve::{
    ResolvedDenyOps, ResolvedPermissions, ResolvedRestrict, RestrictPath, resolve_permissions,
};

/// The dot-folder remargin owns. Never default-denied.
const REMARGIN_DOT_FOLDER: &str = ".remargin";

/// Canonical names of every mutating op. Membership in this list drives
/// whether `restrict` applies — `deny_ops` is evaluated for ANY op the
/// caller names.
pub const MUTATING_OPS: &[&str] = &[
    "ack",
    "batch",
    "comment",
    "delete",
    "edit",
    "migrate",
    "purge",
    "react",
    "sandbox-add",
    "sandbox-remove",
    "sign",
    "write",
];

/// Structured refusals from [`pre_mutate_check`]. Surfaces through the
/// normal `Result<>` chain into the CLI's error message and the MCP
/// tool's error response.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpGuardError {
    /// A `deny_ops` entry covers `target` and lists `op`.
    #[error("op `{op}` on `{target}` is denied by `deny_ops` rule in {source_file}", target = .target.display(), source_file = .source_file.display())]
    DeniedOp {
        /// The op name (`comment`, `purge`, …).
        op: String,
        /// The source `.remargin.yaml` that declared the rule.
        source_file: PathBuf,
        /// The target path the op was about to mutate.
        target: PathBuf,
    },

    /// The path is inside a dot-folder under a restricted subtree and
    /// the dot-folder is not in `allow_dot_folders`.
    #[error("op `{op}` on `{target}` is denied — path is inside dot-folder `{folder}` (not in allow_dot_folders), under restricted subtree from {source_file}", target = .target.display(), source_file = .source_file.display())]
    DotFolderDenied {
        /// The dot-folder name (e.g. `.git`).
        folder: String,
        /// The op name.
        op: String,
        /// The source `.remargin.yaml` that declared the surrounding
        /// `restrict` rule.
        source_file: PathBuf,
        /// The target path.
        target: PathBuf,
    },

    /// A `restrict` entry covers `target`.
    #[error("op `{op}` on `{target}` is denied by `restrict` rule in {source_file}", target = .target.display(), source_file = .source_file.display())]
    RestrictedPath {
        /// The op name.
        op: String,
        /// The source `.remargin.yaml` that declared the rule.
        source_file: PathBuf,
        /// The target path.
        target: PathBuf,
    },
}

/// `true` when `op` is the canonical name of a mutating op (and hence
/// subject to `restrict`).
#[must_use]
pub fn is_mutating_op(op: &str) -> bool {
    MUTATING_OPS.contains(&op)
}

/// `true` when a [`RestrictPath`] entry covers `target`.
///
/// - Wildcard entries cover every path under their `realm_root`.
/// - Absolute entries cover their own path and any descendant.
///
/// Both inputs should be canonicalized before this is called; the
/// helper does no realpath itself.
#[must_use]
pub fn restrict_covers(entry: &RestrictPath, target: &Path) -> bool {
    match entry {
        RestrictPath::Absolute(path) => path_covers(path, target),
        RestrictPath::Wildcard { realm_root } => path_covers(realm_root, target),
    }
}

/// Run Layer 1 enforcement for an upcoming mutating op.
///
/// `target` is the absolute path of the file the op will operate on.
/// `op` is the canonical op name (`comment`, `write`, etc. — the same
/// names the `plan` surface uses).
///
/// # Errors
///
/// - I/O / parse failures from [`resolve_permissions`].
/// - [`OpGuardError::RestrictedPath`] when a mutating op's target is
///   covered by any `restrict` entry.
/// - [`OpGuardError::DeniedOp`] when a `deny_ops` entry covers the
///   target and lists `op`.
/// - [`OpGuardError::DotFolderDenied`] when the target is inside a
///   dot-folder under a restricted subtree and that dot-folder is not
///   in `allow_dot_folders`.
pub fn pre_mutate_check(system: &dyn System, op: &str, target: &Path) -> Result<()> {
    let canonical_target = system
        .canonicalize(target)
        .unwrap_or_else(|_err| target.to_path_buf());
    let walk_anchor = canonical_target
        .parent()
        .map_or_else(|| canonical_target.clone(), Path::to_path_buf);
    let permissions = resolve_permissions(system, &walk_anchor)?;

    check_against_resolved(op, &canonical_target, &permissions)
}

/// Evaluate already-resolved permissions against `target` and `op`.
/// Split out so unit tests can drive the matcher without re-walking.
///
/// # Errors
///
/// Returns the same [`OpGuardError`] variants as [`pre_mutate_check`].
pub fn check_against_resolved(
    op: &str,
    target: &Path,
    permissions: &ResolvedPermissions,
) -> Result<()> {
    if let Some(violation) = find_deny_ops_violation(op, target, &permissions.deny_ops) {
        return Err(violation.into());
    }

    if is_mutating_op(op) {
        if let Some(violation) = find_restrict_violation(op, target, &permissions.restrict) {
            return Err(violation.into());
        }

        if let Some(violation) = find_dot_folder_violation(
            op,
            target,
            &permissions.restrict,
            &permissions.allow_dot_folders,
        ) {
            return Err(violation.into());
        }
    }

    Ok(())
}

fn find_deny_ops_violation(
    op: &str,
    target: &Path,
    deny_ops: &[ResolvedDenyOps],
) -> Option<OpGuardError> {
    deny_ops
        .iter()
        .find(|entry| path_covers(&entry.path, target) && entry.ops.iter().any(|name| name == op))
        .map(|entry| OpGuardError::DeniedOp {
            op: String::from(op),
            source_file: entry.source_file.clone(),
            target: target.to_path_buf(),
        })
}

fn find_dot_folder_violation(
    op: &str,
    target: &Path,
    restrict: &[ResolvedRestrict],
    allow_dot_folders: &[String],
) -> Option<OpGuardError> {
    for entry in restrict {
        let realm_anchor = match &entry.path {
            RestrictPath::Absolute(path) => path.as_path(),
            RestrictPath::Wildcard { realm_root } => realm_root.as_path(),
        };
        if !path_covers(realm_anchor, target) {
            continue;
        }
        if let Some(folder) = first_disallowed_dot_folder(realm_anchor, target, allow_dot_folders) {
            return Some(OpGuardError::DotFolderDenied {
                folder,
                op: String::from(op),
                source_file: entry.source_file.clone(),
                target: target.to_path_buf(),
            });
        }
    }
    None
}

fn find_restrict_violation(
    op: &str,
    target: &Path,
    restrict: &[ResolvedRestrict],
) -> Option<OpGuardError> {
    restrict
        .iter()
        .find(|entry| restrict_covers(&entry.path, target))
        .map(|entry| OpGuardError::RestrictedPath {
            op: String::from(op),
            source_file: entry.source_file.clone(),
            target: target.to_path_buf(),
        })
}

/// Walk `target`'s components beneath `realm_anchor` looking for the
/// first dot-folder component that is not on `allow_dot_folders`. The
/// `.remargin/` folder is always allowed.
fn first_disallowed_dot_folder(
    realm_anchor: &Path,
    target: &Path,
    allow_dot_folders: &[String],
) -> Option<String> {
    let suffix = target.strip_prefix(realm_anchor).ok()?;
    let mut components = suffix.components();

    // The final component is the file itself; only intermediate
    // directory components carry dot-folder semantics for "this file
    // lives inside <dot-folder>". A leading dot on the file name
    // alone (e.g. `.envrc`) does not trigger the guard.
    components.next_back()?;

    components.find_map(|comp| {
        let name = comp.as_os_str().to_string_lossy();
        if !name.starts_with('.') {
            return None;
        }
        if name == REMARGIN_DOT_FOLDER {
            return None;
        }
        if allow_dot_folders
            .iter()
            .any(|allowed| allowed == name.as_ref())
        {
            return None;
        }
        Some(String::from(name))
    })
}

/// `true` when `target` equals `anchor` or is a descendant of it.
fn path_covers(anchor: &Path, target: &Path) -> bool {
    target == anchor || target.starts_with(anchor)
}
