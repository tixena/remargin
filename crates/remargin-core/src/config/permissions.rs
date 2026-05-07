//! Permissions schema for the `permissions:` block in `.remargin.yaml`
//! (rem-yj1j.1 / T22).
//!
//! This module defines the on-disk shape only — accumulation and path
//! resolution live in the [`resolve`] submodule. Enforcement is
//! intentionally absent; it lands in T23 (Layer 1 op guard) and the
//! surrounding sibling features (T24-T29) which all consume the resolved
//! data layer produced here.
//!
//! ## Back-compat
//!
//! The corresponding field on [`crate::config::Config`] is annotated
//! `#[serde(default)]` so existing `.remargin.yaml` files without a
//! `permissions:` block continue to load with [`Permissions::default`].
//!
//! ## Strict field validation
//!
//! Every struct in this module is `#[serde(deny_unknown_fields)]`. A
//! typo under `permissions:` is a configuration bug, not a forward-
//! compat opportunity — fail loudly at parse time so the user sees the
//! mistake before any op runs.

pub mod op_name;
pub mod resolve;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use self::op_name::OpName;

/// A single entry under `permissions.deny_ops`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DenyOpsEntry {
    /// Op names to deny on `path` (e.g. `[purge, delete]`). Unknown
    /// op names are rejected at parse time — see [`OpName`] for the
    /// authoritative list.
    pub ops: Vec<OpName>,

    /// Subpath relative to the `.remargin.yaml` that declared it, OR
    /// an absolute path. Wildcards are NOT accepted here — `deny_ops`
    /// targets specific files / directories.
    pub path: String,

    /// Optional identity (or list of identities) the deny applies to.
    /// Empty (the default) means "all identities" — current behavior
    /// for back-compat with `deny_ops` entries written before rem-egp9.
    ///
    /// Honored only in strict mode. Open-mode realms cannot trust the
    /// caller's declared identity (it is trivially spoofed via
    /// `--identity`), so the resolver records the entry but `op_guard`
    /// ignores the `to:` filter and `lint_permissions_in_parents`
    /// surfaces a warning. Strict mode validates identity against the
    /// registry + signing key, so the filter is meaningful there.
    ///
    /// Identity matching: each entry compares against the caller's
    /// `identity.name` first, falling back to `identity.id` (the
    /// `.remargin.yaml`'s `identity:` field IS the name). This matches
    /// the convention used elsewhere in the resolver.
    #[serde(default)]
    pub to: Vec<String>,
}

/// On-disk shape of the `permissions:` block in `.remargin.yaml`.
///
/// All fields have `#[serde(default)]` so partial declarations are
/// valid (an empty `permissions: {}` block parses as
/// [`Permissions::default`]).
///
/// ## Allow-list polarity
///
/// `restrict` is an **allow-list**: each entry names a path where
/// remargin's mutating ops are sanctioned. With at least one `restrict`
/// declared in the parent walk, the per-op guard runs in allow-list
/// mode and refuses targets outside every entry. With zero `restrict`
/// declared anywhere on the walk, remargin runs in open mode and any
/// target is allowed.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Permissions {
    /// Dot-folders that are otherwise hidden but should be visible to
    /// the file browser / sandbox (e.g. `.github`).
    #[serde(default)]
    pub allow_dot_folders: Vec<String>,

    /// Per-path op denials (e.g. block `purge` on `src/secret/`).
    #[serde(default)]
    pub deny_ops: Vec<DenyOpsEntry>,

    /// Allow-list of paths where remargin's mutating ops are sanctioned.
    /// The literal `"*"` matches the entire realm anchored at the
    /// declaring `.remargin.yaml`.
    #[serde(default)]
    pub restrict: Vec<RestrictEntry>,
}

/// A single entry under `permissions.restrict`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RestrictEntry {
    /// Bash command names that are also denied at the Claude-settings
    /// layer when this restrict is active (e.g. `rm`, `git rm`). Layer
    /// 1 enforcement does not touch Bash; this list is consumed by the
    /// Claude-settings synchronizer (T25 / rem-yj1j.4).
    #[serde(default)]
    pub also_deny_bash: Vec<String>,

    /// When `true`, suppresses the projected `Bash(remargin *)` deny
    /// rule so the CLI stays usable inside this restrict (only the MCP
    /// / agent surfaces are blocked). Purely deny-side: this flag does
    /// NOT add anything to the projected `permissions.allow` list, and
    /// since rem-si27 dropped the auto-emitted `mcp__remargin__*`
    /// allow, `restrict` no longer projects ANY allow rule unless the
    /// user explicitly names an `allow_dot_folders` entry. Defaults to
    /// `false`.
    #[serde(default)]
    pub cli_allowed: bool,

    /// Subpath relative to the `.remargin.yaml` that declared it, OR
    /// the literal `"*"` for realm-wide.
    pub path: String,
}
