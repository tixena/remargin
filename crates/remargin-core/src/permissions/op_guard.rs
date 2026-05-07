//! Per-op permission guard.
//!
//! `restrict` is an allow-list: at least one entry in the parent walk
//! engages allow-list mode (target must lie inside some entry); zero
//! entries = open mode. `deny_ops` is evaluated regardless and is the
//! escape hatch for blocking specific ops including reads.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use thiserror::Error;

use crate::config::Mode;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::{
    ResolvedDenyOps, ResolvedPermissions, ResolvedTrustedRoot, resolve_permissions,
    trusted_root_anchor, trusted_root_covers,
};
use crate::parser::AuthorType;

/// The dot-folder remargin owns. Never default-denied.
const REMARGIN_DOT_FOLDER: &str = ".remargin";

/// Canonical names of every read-side op recognised by the guard.
/// Read ops bypass `restrict` and the dot-folder default-deny. They
/// are still subject to `deny_ops`. Keep alphabetical.
///
/// The contents must match [`OpName::READ`] — a parity test in the
/// adjacent `tests` module enforces this.
pub const READ_OPS: &[&str] = &[
    "comments", "get", "lint", "ls", "metadata", "query", "search", "verify",
];

/// Canonical names of every mutating op.
///
/// Membership in this list drives whether `restrict` applies —
/// `deny_ops` is evaluated for ANY op the caller names. The contents
/// must match [`OpName::WRITE`] — a parity test in the adjacent
/// `tests` module enforces this.
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

/// Wording pinned by `denial_error_wording_matches_canonical_template`.
pub const OUTSIDE_ALLOWED_DENIAL_TEMPLATE: &str = "op '{op}' on '{target}' is denied: outside the allow-list declared by 'restrict' in {source_file}";

pub const DENY_OPS_DENIAL_TEMPLATE: &str =
    "op '{op}' on '{target}' is denied by 'deny_ops' rule in {source_file}";

/// Read-vs-write classification for an op.
///
/// Drives whether `restrict` gates the op; `deny_ops` is evaluated
/// for both kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpKind {
    /// Read-side op. Bypasses `restrict` and the dot-folder default-
    /// deny. Still gated by explicit `deny_ops` entries.
    Read,
    /// Write-side op. Gated by `restrict`, the dot-folder default-deny,
    /// and `deny_ops`.
    Write,
}

/// Caller-side identity context the per-op guard needs to evaluate
/// identity-scoped `deny_ops` entries (rem-egp9).
///
/// Open-mode realms cannot trust the caller's declared identity (it is
/// trivially spoofed via `--identity` / `type:` flags), so the
/// identity filter is ignored there. Strict mode validates identity
/// against the registry + signing key, so the filter is meaningful.
///
/// Default carries no identity / `Mode::Open`, which preserves
/// pre-rem-egp9 behaviour for callers that don't supply caller info.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct CallerInfo {
    /// Caller's `type:` field (`human` / `agent`). Drives the
    /// synthesized `~/.ssh/**` default-deny: agents in strict mode
    /// get the deny baked in; humans / open-mode realms do not.
    pub author_type: Option<AuthorType>,
    /// Caller identity name (the `identity:` field from
    /// `.remargin.yaml`). The `to:` filter on `deny_ops` matches this
    /// first, then falls back to `id` (currently `id` and `name` are
    /// the same string in the on-disk schema, but the parameter is
    /// kept distinct so the doc comment on `DenyOpsEntry::to`
    /// accurately describes the matching order).
    pub identity_id: Option<String>,
    /// Caller identity name. See [`Self::identity_id`].
    pub identity_name: Option<String>,
    /// Realm mode. `to:` filtering on `deny_ops` only fires when
    /// `mode == Mode::Strict`; open / registered modes ignore the
    /// filter (the realm cannot trust the declared identity).
    pub mode: Mode,
}

impl CallerInfo {
    /// `true` when the caller is an agent operating in strict mode.
    /// Drives the synthesized `~/.ssh/**` virtual deny (rem-egp9).
    #[must_use]
    pub const fn is_strict_agent(&self) -> bool {
        matches!(self.author_type, Some(AuthorType::Agent)) && matches!(self.mode, Mode::Strict)
    }

    /// `true` when the caller's name or id matches one of `to`.
    /// Empty `to` returns `true` (entry applies to all identities).
    #[must_use]
    pub fn matches_to(&self, to: &[String]) -> bool {
        if to.is_empty() {
            return true;
        }
        if let Some(name) = self.identity_name.as_deref()
            && to.iter().any(|t| t == name)
        {
            return true;
        }
        if let Some(id) = self.identity_id.as_deref()
            && to.iter().any(|t| t == id)
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
    /// A `deny_ops` entry covers `target` and lists `op`.
    ///
    /// `to` carries the identity filter that fired (empty when the
    /// entry's `to:` was empty / not supplied — the legacy "all
    /// identities" deny). When non-empty the refusal text names the
    /// matching identity per the rem-egp9 wording:
    /// `op '<op>' on '<path>' is denied by 'deny_ops { to: <identity> }'
    /// rule in <source_file>`.
    #[error(
        "op `{op}` on `{target}` is denied by `deny_ops{to_clause}` rule in {source_file}",
        target = .target.display(),
        source_file = .source_file.display(),
        to_clause = format_to_clause(.to),
    )]
    DeniedOp {
        /// The op name (`comment`, `purge`, …).
        op: String,
        /// The source `.remargin.yaml` that declared the rule.
        source_file: PathBuf,
        /// The target path the op was about to mutate.
        target: PathBuf,
        /// Identities the entry's `to:` field lists. Empty means the
        /// entry's `to:` was empty / not supplied (legacy shape).
        to: Vec<String>,
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

    /// `target` is outside every allow-listed root from `restrict`.
    #[error("op `{op}` on `{target}` is denied: outside the allow-list declared by `restrict` in {source_file}", target = .target.display(), source_file = .source_file.display())]
    OutsideAllowedRoots {
        op: String,
        /// First `.remargin.yaml` in walk order that declared a
        /// `restrict` entry (used to point the user at where the
        /// allow-list lives).
        source_file: PathBuf,
        target: PathBuf,
    },
}

/// Render the `{ to: <ids> }` slot of [`OpGuardError::DeniedOp`].
/// Empty `to` produces an empty string so the legacy wording (no
/// `to:` slot) round-trips for back-compat with pre-rem-egp9 entries.
fn format_to_clause(to: &[String]) -> String {
    if to.is_empty() {
        String::new()
    } else {
        format!(" {{ to: {} }}", to.join(", "))
    }
}

/// `true` when `op` is the canonical name of a mutating op (and hence
/// subject to `restrict`).
///
/// Unknown op names default to `true` so a missing classification fails
/// closed under `restrict`. Callers should ensure new ops are added to
/// [`READ_OPS`] or [`MUTATING_OPS`].
#[must_use]
pub fn is_mutating_op(op: &str) -> bool {
    !matches!(op_kind(op), Some(OpKind::Read))
}

/// Classify `op` as read or write.
///
/// Returns `None` for op names the guard does not know about. Callers
/// that pass an unknown op (e.g. an op handler that forgets to plumb
/// the canonical name into either [`READ_OPS`] or [`MUTATING_OPS`])
/// will fall through the `None` arm; [`is_mutating_op`] treats unknown
/// ops as write-side for safety.
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
    pre_mutate_check_for_caller(system, op, target, &CallerInfo::default())
}

/// Run Layer 1 enforcement for an upcoming mutating op, with caller
/// context for identity-scoped `deny_ops` (rem-egp9).
///
/// Identical to [`pre_mutate_check`] except the `caller` lets the
/// guard:
/// - skip `deny_ops` entries whose `to:` filter does not match the
///   caller (when in strict mode), and
/// - synthesize a virtual `~/.ssh/**` deny when the caller is an
///   agent in strict mode (overridable via an explicit `to:` entry
///   for the same path with `ops: []`).
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

    check_against_resolved_for_caller(op, &canonical_target, &permissions, caller)
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
    check_against_resolved_for_caller(op, target, permissions, &CallerInfo::default())
}

/// Caller-aware variant of [`check_against_resolved`].
///
/// The caller-scoped `deny_ops { to: ... }` filter is honored only in
/// strict mode (open-mode realms record the entry but ignore the
/// filter — `lint_permissions_in_parents` surfaces a warning). Agents
/// in strict mode get a synthesized `~/.ssh/**` virtual deny that the
/// user can override by listing the same path with explicit
/// `to: [<identity>]` and `ops: []`.
///
/// # Errors
///
/// Same as [`check_against_resolved`].
pub fn check_against_resolved_for_caller(
    op: &str,
    target: &Path,
    permissions: &ResolvedPermissions,
    caller: &CallerInfo,
) -> Result<()> {
    let home = system_home_or_passthrough();
    let deny_ops = effective_deny_ops(&home, permissions, caller);
    if let Some(violation) = find_deny_ops_violation(op, target, &deny_ops, caller) {
        return Err(violation.into());
    }

    if is_mutating_op(op) {
        if let Some(violation) =
            find_trusted_roots_violation(op, target, &permissions.trusted_roots)
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
    }

    Ok(())
}

/// Where to expand `~/.ssh/**` for the synthesized agent default-deny.
/// Falls back to the literal `~/.ssh` when `$HOME` is unset; the per-
/// op layer compares canonical paths so a literal `~` is harmless on
/// systems that have no home directory.
fn system_home_or_passthrough() -> PathBuf {
    use std::env;
    env::var_os("HOME").map_or_else(|| PathBuf::from("~"), PathBuf::from)
}

/// Build the effective `deny_ops` list the caller should be checked
/// against (rem-egp9). Synthesizes a virtual `~/.ssh/**` deny entry
/// when the caller is an agent in strict mode and the user has not
/// explicitly opted out via a same-path entry with `ops: []` and a
/// `to:` filter naming the caller.
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
    // Skip synthesis when the user has an explicit override: a
    // deny_ops entry on the same path with empty `ops` and a `to:`
    // that names the caller. That signals "trust this agent here".
    let overridden = permissions.deny_ops.iter().any(|entry| {
        path_covers(&entry.path, &ssh_dir) && entry.ops.is_empty() && caller.matches_to(&entry.to)
    });
    if overridden {
        return out;
    }
    // Synthesize a virtual deny covering EVERY op (read + mutating)
    // on `~/.ssh/**`. The `to:` field is empty because the entry is
    // already gated on `is_strict_agent`.
    let virtual_entry = ResolvedDenyOps {
        ops: OpName::ALL.to_vec(),
        path: ssh_dir,
        source_file: PathBuf::from("<rem-egp9 default: agents cannot read ~/.ssh/**>"),
        to: Vec::new(),
    };
    // Prepend so it fires before any user-declared deny on overlapping
    // paths — order is "first match wins" in [`find_deny_ops_violation`].
    out.insert(0, virtual_entry);
    out
}

fn find_deny_ops_violation(
    op: &str,
    target: &Path,
    deny_ops: &[ResolvedDenyOps],
    caller: &CallerInfo,
) -> Option<OpGuardError> {
    deny_ops
        .iter()
        .find(|entry| {
            if !path_covers(&entry.path, target) {
                return false;
            }
            if !entry.ops.iter().any(|name| name.as_str() == op) {
                return false;
            }
            // `to:` filtering is honored only in strict mode. Open /
            // registered modes ignore the filter (the realm cannot
            // trust the declared identity); the lint surface warns.
            if entry.to.is_empty() {
                return true;
            }
            if matches!(caller.mode, Mode::Strict) {
                caller.matches_to(&entry.to)
            } else {
                // Open / registered: legacy wide deny; ignore `to:`.
                true
            }
        })
        .map(|entry| OpGuardError::DeniedOp {
            op: String::from(op),
            source_file: entry.source_file.clone(),
            target: target.to_path_buf(),
            to: entry.to.clone(),
        })
}

/// `true` when `target` is inside at least one `trusted_roots` entry,
/// or when the list is empty (open mode). Shared by `op_guard` and
/// `inspect::check` so the two layers can't drift.
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
    trusted_roots: &[ResolvedTrustedRoot],
) -> Option<OpGuardError> {
    if target_is_sanctioned(target, trusted_roots) {
        return None;
    }
    let first = trusted_roots.first()?;
    Some(OpGuardError::OutsideAllowedRoots {
        op: String::from(op),
        source_file: first.source_file.clone(),
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
