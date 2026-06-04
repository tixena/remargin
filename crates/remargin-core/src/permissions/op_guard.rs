//! Per-op permission guard. `trusted_roots` is the single allow-list
//! for reads and writes; `deny_ops` is the per-op escape hatch.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use thiserror::Error;

use crate::config::Mode;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::{
    ResolvedDenyOps, ResolvedDenyOpsItem, ResolvedPermissions, ResolvedTrustedRoot,
    resolve_permissions, trusted_root_anchor, trusted_root_covers,
};
use crate::parser::AuthorType;

/// The dot-folder remargin owns. Never default-denied.
const REMARGIN_DOT_FOLDER: &str = ".remargin";

/// Keep alphabetical. Must match [`OpName::READ`] (parity-tested).
pub const READ_OPS: &[&str] = &[
    "comments", "get", "lint", "ls", "metadata", "query", "search", "verify",
];

/// Keep alphabetical. Must match [`OpName::WRITE`] (parity-tested).
pub const MUTATING_OPS: &[&str] = &[
    "ack",
    "batch",
    "comment",
    "cp",
    "delete",
    "edit",
    "mv",
    "purge",
    "react",
    "replace",
    "sandbox-add",
    "sandbox-remove",
    "sign",
    "write",
];

/// Wording pinned by `denial_error_wording_matches_canonical_template`.
pub const OUTSIDE_ALLOWED_DENIAL_TEMPLATE: &str = "op '{op}' on '{target}' is denied: outside the allow-list declared by 'trusted_roots' in {source_file}";

pub const DENY_OPS_DENIAL_TEMPLATE: &str =
    "op '{op}' on '{target}' is denied by 'deny_ops' rule in {source_file}";

/// Op kind. The guard treats both symmetrically; classification is
/// preserved so projection surfaces can reason about shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpKind {
    Read,
    Write,
}

/// Caller-side context for identity-scoped `deny_ops` exceptions.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct CallerInfo {
    pub author_type: Option<AuthorType>,
    pub identity_id: Option<String>,
    pub identity_name: Option<String>,
    pub mode: Mode,
}

impl CallerInfo {
    #[must_use]
    pub fn display_identity(&self) -> String {
        self.identity_name
            .clone()
            .or_else(|| self.identity_id.clone())
            .unwrap_or_else(|| String::from("<anonymous>"))
    }

    #[must_use]
    pub const fn is_strict_agent(&self) -> bool {
        matches!(self.author_type, Some(AuthorType::Agent)) && matches!(self.mode, Mode::Strict)
    }

    /// `true` when the caller's name or id matches one of `list`.
    /// Empty `list` returns `false` (caller is in no list).
    #[must_use]
    pub fn matches_identity_list(&self, list: &[String]) -> bool {
        if list.is_empty() {
            return false;
        }
        if let Some(name) = self.identity_name.as_deref()
            && list.iter().any(|t| t == name)
        {
            return true;
        }
        if let Some(id) = self.identity_id.as_deref()
            && list.iter().any(|t| t == id)
        {
            return true;
        }
        false
    }
}

/// Structured refusals from [`pre_mutate_check`]. Surfaces through the
/// normal `Result<>` chain into the CLI's error message and the MCP
/// tool's error response.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpGuardError {
    /// A `deny_ops` item covers `target` and matches `op` with no
    /// `exceptions` configured — blanket deny for every identity.
    #[error(
        "op `{op}` on `{target}` is denied by `deny_ops` rule in {source_file}",
        target = .target.display(),
        source_file = .source_file.display(),
    )]
    DeniedOp {
        op: String,
        source_file: PathBuf,
        target: PathBuf,
    },

    /// A `deny_ops` item covers `target` and matches `op`, has
    /// non-empty `exceptions`, but caller is not in that allowlist.
    #[error(
        "op `{op}` on `{target}` is denied by `deny_ops` rule in {source_file} (caller `{caller}` is not in the exception list)",
        target = .target.display(),
        source_file = .source_file.display(),
    )]
    DeniedOpNotExcepted {
        caller: String,
        op: String,
        source_file: PathBuf,
        target: PathBuf,
    },

    /// The path is inside a dot-folder under a restricted subtree and
    /// the dot-folder is not in `allow_dot_folders`.
    #[error("op `{op}` on `{target}` is denied — path is inside dot-folder `{folder}` (not in allow_dot_folders), under restricted subtree from {source_file}", target = .target.display(), source_file = .source_file.display())]
    DotFolderDenied {
        folder: String,
        op: String,
        source_file: PathBuf,
        target: PathBuf,
    },

    /// `target` is outside every allow-listed root in `trusted_roots`.
    #[error("op `{op}` on `{target}` is denied: outside the allow-list declared by `trusted_roots` in {source_file}", target = .target.display(), source_file = .source_file.display())]
    OutsideAllowedRoots {
        op: String,
        source_file: PathBuf,
        target: PathBuf,
    },
}

/// Unknown ops default to `true` so missing classification fails
/// closed (treated as write for projection surfaces).
#[must_use]
pub fn is_mutating_op(op: &str) -> bool {
    !matches!(op_kind(op), Some(OpKind::Read))
}

/// `None` for unknown ops. [`is_mutating_op`] treats those as write.
#[must_use]
pub fn op_kind(op: &str) -> Option<OpKind> {
    if READ_OPS.contains(&op) {
        Some(OpKind::Read)
    } else if MUTATING_OPS.contains(&op) {
        Some(OpKind::Write)
    } else {
        None
    }
}

/// Layer 1 enforcement for an upcoming op (read or write). `op` is
/// the canonical name (`comment`, `write`, `get`, …); name kept for
/// back-compat — the guard gates reads as well.
///
/// # Errors
///
/// - I/O / parse failures from [`resolve_permissions`].
/// - [`OpGuardError::OutsideAllowedRoots`], [`OpGuardError::DeniedOp`],
///   [`OpGuardError::DotFolderDenied`] when the corresponding rule
///   fires.
pub fn pre_mutate_check(system: &dyn System, op: &str, target: &Path) -> Result<()> {
    pre_mutate_check_for_caller(system, op, target, &CallerInfo::default())
}

/// Caller-aware variant: skips `deny_ops` `to:` mismatches in strict
/// mode and synthesizes the agent `~/.ssh/**` default-deny.
///
/// # Errors
///
/// Same as [`pre_mutate_check`].
pub fn pre_mutate_check_for_caller(
    system: &dyn System,
    op: &str,
    target: &Path,
    caller: &CallerInfo,
) -> Result<()> {
    let canonical_target = system
        .canonicalize(target)
        .unwrap_or_else(|_err| target.to_path_buf());
    let walk_anchor = canonical_target
        .parent()
        .map_or_else(|| canonical_target.clone(), Path::to_path_buf);
    let permissions = resolve_permissions(system, &walk_anchor)?;

    check_against_resolved_for_caller(system, op, &canonical_target, &permissions, caller)
}

/// Evaluate already-resolved permissions against `target` and `op`.
/// Split out so unit tests can drive the matcher without re-walking.
///
/// # Errors
///
/// Returns the same [`OpGuardError`] variants as [`pre_mutate_check`].
pub fn check_against_resolved(
    system: &dyn System,
    op: &str,
    target: &Path,
    permissions: &ResolvedPermissions,
) -> Result<()> {
    check_against_resolved_for_caller(system, op, target, permissions, &CallerInfo::default())
}

/// Caller-aware variant. Reads HOME via `system` so `MockSystem`
/// tests drive `~/.ssh/**` via `with_env`.
///
/// # Errors
///
/// Same as [`check_against_resolved`].
pub fn check_against_resolved_for_caller(
    system: &dyn System,
    op: &str,
    target: &Path,
    permissions: &ResolvedPermissions,
    caller: &CallerInfo,
) -> Result<()> {
    let home = system_home_or_passthrough(system);
    let deny_ops = effective_deny_ops(&home, permissions, caller);
    if let Some(violation) = find_deny_ops_violation(op, target, &deny_ops, caller) {
        return Err(violation.into());
    }

    // No opinion anywhere on the walk leaves the guard silent so the
    // call site can supply the implicit cwd root.
    if !permissions.trusted_roots_unconstrained()
        && let Some(violation) = find_trusted_roots_violation(op, target, permissions)
    {
        return Err(violation.into());
    }

    let allow_dot_folder_names = permissions.allow_dot_folder_names();
    if let Some(violation) = find_dot_folder_violation(
        op,
        target,
        &permissions.trusted_roots,
        &allow_dot_folder_names,
    ) {
        return Err(violation.into());
    }

    Ok(())
}

fn system_home_or_passthrough(system: &dyn System) -> PathBuf {
    system
        .env_var("HOME")
        .map_or_else(|_err| PathBuf::from("~"), PathBuf::from)
}

/// Synthesizes a virtual `~/.ssh/**` deny for strict-mode agents
/// unless a user entry on `~/.ssh` opts the caller out via per-op
/// `exceptions`.
fn effective_deny_ops(
    home: &Path,
    permissions: &ResolvedPermissions,
    caller: &CallerInfo,
) -> Vec<ResolvedDenyOps> {
    let mut out: Vec<ResolvedDenyOps> = permissions.deny_ops.clone();
    if !caller.is_strict_agent() {
        return out;
    }
    let ssh_dir = home.join(".ssh");
    let overridden = permissions.deny_ops.iter().any(|entry| {
        path_covers(&entry.path, &ssh_dir)
            && entry
                .ops
                .iter()
                .any(|item| caller.matches_identity_list(&item.exceptions))
    });
    if overridden {
        return out;
    }
    let virtual_items: Vec<ResolvedDenyOpsItem> = OpName::ALL
        .iter()
        .copied()
        .map(|name| ResolvedDenyOpsItem {
            exceptions: Vec::new(),
            name,
        })
        .collect();
    let virtual_entry = ResolvedDenyOps {
        ops: virtual_items,
        path: ssh_dir,
        source_file: PathBuf::from("<default: agents cannot access ~/.ssh/**>"),
    };
    out.insert(0, virtual_entry);
    out
}

fn find_deny_ops_violation(
    op: &str,
    target: &Path,
    deny_ops: &[ResolvedDenyOps],
    caller: &CallerInfo,
) -> Option<OpGuardError> {
    for entry in deny_ops {
        if !path_covers(&entry.path, target) {
            continue;
        }
        for item in &entry.ops {
            if item.name.as_str() != op {
                continue;
            }
            if item.exceptions.is_empty() {
                return Some(OpGuardError::DeniedOp {
                    op: String::from(op),
                    source_file: entry.source_file.clone(),
                    target: target.to_path_buf(),
                });
            }
            if caller.matches_identity_list(&item.exceptions) {
                continue;
            }
            return Some(OpGuardError::DeniedOpNotExcepted {
                caller: caller.display_identity(),
                op: String::from(op),
                source_file: entry.source_file.clone(),
                target: target.to_path_buf(),
            });
        }
    }
    None
}

/// Empty list returns `true`; callers decide what that means.
/// Shared between `op_guard` and inspect to keep them in sync.
#[must_use]
pub fn target_is_sanctioned(target: &Path, trusted_roots: &[ResolvedTrustedRoot]) -> bool {
    if trusted_roots.is_empty() {
        return true;
    }
    trusted_roots
        .iter()
        .any(|entry| trusted_root_covers(&entry.path, target))
}

fn find_dot_folder_violation(
    op: &str,
    target: &Path,
    trusted_roots: &[ResolvedTrustedRoot],
    allow_dot_folders: &[String],
) -> Option<OpGuardError> {
    for entry in trusted_roots {
        let realm_anchor = trusted_root_anchor(entry);
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

fn find_trusted_roots_violation(
    op: &str,
    target: &Path,
    permissions: &ResolvedPermissions,
) -> Option<OpGuardError> {
    // `target_is_sanctioned` returns `true` for an empty list (open
    // mode), so the lock case (`trusted_roots: []` with no inherited
    // entries) needs an explicit check: any target outside the empty
    // set is denied.
    let inside = !permissions.trusted_roots.is_empty()
        && target_is_sanctioned(target, &permissions.trusted_roots);
    if inside {
        return None;
    }
    let source_file = permissions
        .trusted_roots
        .first()
        .map(|entry| entry.source_file.clone())
        .or_else(|| permissions.trusted_roots_lock.clone())?;
    Some(OpGuardError::OutsideAllowedRoots {
        op: String::from(op),
        source_file,
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
