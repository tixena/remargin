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

pub mod resolve;

#[cfg(test)]
mod tests;

use serde::Deserialize;

/// A single entry under `permissions.deny_ops`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DenyOpsEntry {
    /// Op names to deny on `path` (e.g. `["purge", "delete"]`).
    pub ops: Vec<String>,

    /// Subpath relative to the `.remargin.yaml` that declared it, OR
    /// an absolute path. Wildcards are NOT accepted here — `deny_ops`
    /// targets specific files / directories.
    pub path: String,
}

/// On-disk shape of the `permissions:` block in `.remargin.yaml`.
///
/// All fields have `#[serde(default)]` so partial declarations
/// (`permissions: { trusted_roots: [...] }`) are valid.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
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

    /// Subpaths to restrict from agent edits. The literal `"*"` matches
    /// the entire realm anchored at the declaring `.remargin.yaml`.
    #[serde(default)]
    pub restrict: Vec<RestrictEntry>,

    /// Directories the MCP server is allowed to expose. Resolved via
    /// realpath at use time (see [`resolve::resolve_permissions`]).
    #[serde(default)]
    pub trusted_roots: Vec<String>,
}

/// A single entry under `permissions.restrict`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RestrictEntry {
    /// Bash command names that are also denied at the Claude-settings
    /// layer when this restrict is active (e.g. `rm`, `git rm`). Layer
    /// 1 enforcement does not touch Bash; this list is consumed by the
    /// Claude-settings synchronizer (T25 / rem-yj1j.4).
    #[serde(default)]
    pub also_deny_bash: Vec<String>,

    /// When `true`, the CLI is allowed inside this restrict (only the
    /// MCP / agent surfaces are blocked). Defaults to `false`.
    #[serde(default)]
    pub cli_allowed: bool,

    /// Subpath relative to the `.remargin.yaml` that declared it, OR
    /// the literal `"*"` for realm-wide.
    pub path: String,
}
