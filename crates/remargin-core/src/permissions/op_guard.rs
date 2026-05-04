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
//! - **`trusted_roots` carve out outer restricts** — when a target is
//!   inside a `trusted_root` whose path is at-or-below a `restrict`
//!   entry's anchor, that restrict (and its associated dot-folder
//!   default-deny) is bypassed for that target. This enables the
//!   allowlist pattern: declare `restrict '*'` at a parent realm
//!   (e.g. `~/.remargin.yaml`) and list the writable subtrees in
//!   `trusted_roots`. Restricts declared *inside* a trusted root still
//!   fire — they are the more specific opt-out and win, because the
//!   trusted root is no longer at-or-below their anchor. `deny_ops` is
//!   never affected; it always fires regardless of `trusted_roots`, so
//!   it remains the right primitive for "block this op everywhere".
//!
//! ## Op classification (read vs write)
//!
//! [`OpKind`] is the canonical read-vs-write classifier. Every op the
//! CLI / MCP surface dispatches to remargin-core MUST be classified by
//! [`op_kind`]. The classification drives whether `restrict` (and the
//! dot-folder default-deny) gates the op:
//!
//! - [`OpKind::Read`] ops bypass `restrict`. To block a read on a
//!   restricted path, declare an explicit `deny_ops` entry naming the
//!   read op. Current read ops: `comments`, `get`, `lint`, `ls`,
//!   `metadata`, `query`, `search`, `verify`.
//! - [`OpKind::Write`] ops are gated by `restrict`. Current write ops:
//!   `ack`, `batch`, `comment`, `delete`, `edit`, `migrate`, `purge`,
//!   `react`, `sandbox-add`, `sandbox-remove`, `sign`, `write`.
//!
//! `deny_ops` is evaluated for every op regardless of kind — that is
//! the read-side carve-out's escape hatch.
//!
//! ## Denial-error wording (pinned)
//!
//! The user-visible error text for a denial is documented and pinned by
//! a unit test:
//!
//! - [`RESTRICT_DENIAL_TEMPLATE`] — `op '<op>' on '<target>' is denied
//!   by 'restrict' rule in <yaml>`.
//! - [`DENY_OPS_DENIAL_TEMPLATE`] — `op '<op>' on '<target>' is denied
//!   by 'deny_ops' rule in <yaml>`.
//!
//! Templates use backtick-delimited slots in the actual `Display`
//! impls. Both forms are recognised by the wording test so wording
//! drift in either direction is caught.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use thiserror::Error;

use crate::config::Mode;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::{
    ResolvedDenyOps, ResolvedPermissions, ResolvedRestrict, RestrictPath, TrustedRoot,
    resolve_permissions,
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

/// Documented template for [`OpGuardError::RestrictedPath`].
///
/// The actual `Display` impl uses backticks around the slots; this
/// template uses single quotes to match the wording in design docs
/// and acceptance criteria. The wording-pin test
/// (`denial_error_wording_matches_canonical_template`) accepts either
/// delimiter so neither form can drift without notice.
pub const RESTRICT_DENIAL_TEMPLATE: &str =
    "op '{op}' on '{target}' is denied by 'restrict' rule in {source_file}";

/// Documented template for [`OpGuardError::DeniedOp`]. See
/// [`RESTRICT_DENIAL_TEMPLATE`] for the delimiter convention.
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
        if let Some(violation) = find_restrict_violation(
            op,
            target,
            &permissions.restrict,
            &permissions.trusted_roots,
        ) {
            return Err(violation.into());
        }

        let allow_dot_folder_names = permissions.allow_dot_folder_names();
        if let Some(violation) = find_dot_folder_violation(
            op,
            target,
            &permissions.restrict,
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

fn find_dot_folder_violation(
    op: &str,
    target: &Path,
    restrict: &[ResolvedRestrict],
    trusted_roots: &[TrustedRoot],
    allow_dot_folders: &[String],
) -> Option<OpGuardError> {
    for entry in restrict {
        let realm_anchor = restrict_anchor(entry);
        if !path_covers(realm_anchor, target) {
            continue;
        }
        if trusted_roots_carve_out(realm_anchor, target, trusted_roots) {
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
    trusted_roots: &[TrustedRoot],
) -> Option<OpGuardError> {
    restrict
        .iter()
        .find(|entry| {
            restrict_covers(&entry.path, target)
                && !trusted_roots_carve_out(restrict_anchor(entry), target, trusted_roots)
        })
        .map(|entry| OpGuardError::RestrictedPath {
            op: String::from(op),
            source_file: entry.source_file.clone(),
            target: target.to_path_buf(),
        })
}

/// The anchor path of a `restrict` entry — the absolute path for an
/// `Absolute` entry, or the realm root for a `Wildcard` entry.
fn restrict_anchor(entry: &ResolvedRestrict) -> &Path {
    match &entry.path {
        RestrictPath::Absolute(path) => path.as_path(),
        RestrictPath::Wildcard { realm_root } => realm_root.as_path(),
    }
}

/// `true` when any `trusted_root` carves `target` out of the `restrict`
/// entry anchored at `restrict_anchor`.
///
/// A trusted root T carves out a restrict R for target X when:
/// - T's path is at-or-below R's anchor (so R would otherwise cover T), AND
/// - X is at-or-below T's path (so X is in the carved-out region).
///
/// This means restricts *inside* a trusted root still fire — those
/// restrict anchors live below the trusted root, so the at-or-below
/// check fails and no carve-out applies. They are the more specific
/// opt-out and win.
fn trusted_roots_carve_out(
    restrict_anchor: &Path,
    target: &Path,
    trusted_roots: &[TrustedRoot],
) -> bool {
    trusted_roots
        .iter()
        .any(|tr| path_covers(restrict_anchor, &tr.path) && path_covers(&tr.path, target))
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
