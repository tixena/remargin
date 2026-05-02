//! MCP sandbox boundary built from the parent-walked
//! `permissions.trusted_roots` block (rem-yj1j.3 / T24, slice 1 — see
//! `rem-v0ky` for this module, `rem-w6m1` for the actual MCP wiring).
//!
//! [`McpSandbox::from_walk`] runs once at server startup and resolves
//! the set of canonical absolute paths the MCP is allowed to serve
//! files under. Empty `trusted_roots` falls back to the spawn cwd —
//! that's the back-compat path for MCP sessions launched in repos
//! without a `.remargin.yaml`.
//!
//! # Boundary semantics
//!
//! [`McpSandbox::covers`] canonicalises the candidate path before
//! matching. Symlinks that escape the sandbox (e.g. a link inside a
//! root pointing at `/etc/passwd`) resolve to outside-of-root and the
//! check returns `false`. Paths that do not yet exist are resolved
//! lexically against the nearest existing ancestor so writes-of-new-
//! files inside a covered root succeed.
//!
//! # Static at boot
//!
//! The sandbox is captured once and never reloaded. Editing
//! `trusted_roots` mid-session does not change the active sandbox —
//! the user must restart the MCP. (Per-op re-evaluation of the
//! `restrict` / `deny_ops` blocks still happens through the existing
//! `op_guard` parent walk.)
//!
//! # Interaction with `restrict`
//!
//! At the op-guard layer, `trusted_roots` also carve outer `restrict`
//! entries out for any target inside them — see
//! [`crate::permissions::op_guard`] for the full rule. The two layers
//! are not orthogonal: `trusted_roots` simultaneously gates the MCP
//! file-access boundary (this module) and acts as a write-allowlist
//! exception against parent-realm restricts.
//!
//! # No transitive trust
//!
//! Trust is explicit per session. A trusted root's own `.remargin.yaml`
//! `trusted_roots` are NOT auto-mounted. The check fires at
//! [`McpSandbox::covers`] time on the spawn-cwd-resolved root list
//! only; the parent-walk inside a target realm is independent.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;

use crate::config::permissions::resolve::resolve_permissions;

/// MCP server's allowed-paths surface, resolved at boot from
/// `permissions.trusted_roots` in the parent-walked `.remargin.yaml`.
///
/// Empty `trusted_roots` ⇒ a single root containing the spawn cwd.
/// All entries are canonicalised; duplicates are deduped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpSandbox {
    /// Canonical absolute paths the MCP is authorised to serve.
    /// Sorted + deduped; never empty after [`McpSandbox::from_walk`].
    pub roots: Vec<PathBuf>,
}

impl McpSandbox {
    /// `true` when the canonicalised `target` lives under any sandbox
    /// root. Symlinks are followed via canonicalisation, so a link
    /// escaping the sandbox resolves to its real path and is rejected.
    /// Non-existent targets are resolved lexically against the nearest
    /// existing ancestor.
    ///
    /// # Errors
    ///
    /// Returns an error only if the lexical fallback's parent chain
    /// cannot be walked (extremely rare — almost always implies the
    /// caller passed an empty path).
    pub fn covers(&self, system: &dyn System, target: &Path) -> Result<bool> {
        let canonical = canonicalize_or_lexical(system, target)?;
        Ok(self
            .roots
            .iter()
            .any(|root| canonical == *root || canonical.starts_with(root)))
    }

    /// Same as [`McpSandbox::covers`] but bails with a uniform error
    /// message when the path escapes. Callers that want the boolean
    /// directly can use [`McpSandbox::covers`].
    ///
    /// # Errors
    ///
    /// Returns an error when the path is not covered by any root, or
    /// when the lexical fallback fails.
    pub fn ensure_covers(&self, system: &dyn System, target: &Path) -> Result<()> {
        if self.covers(system, target)? {
            Ok(())
        } else {
            anyhow::bail!("path escapes MCP sandbox: {}", target.display());
        }
    }

    /// Build the sandbox from the walked permissions block.
    ///
    /// Called once at server startup. Per-op work goes through
    /// [`McpSandbox::ensure_covers`].
    ///
    /// # Errors
    ///
    /// Forwards I/O / parse failures from
    /// [`crate::config::permissions::resolve::resolve_permissions`].
    pub fn from_walk(system: &dyn System, spawn_cwd: &Path) -> Result<Self> {
        let perms = resolve_permissions(system, spawn_cwd)?;

        let mut roots: Vec<PathBuf> = perms.trusted_roots.into_iter().map(|tr| tr.path).collect();

        if roots.is_empty() {
            let canonical = system
                .canonicalize(spawn_cwd)
                .unwrap_or_else(|_err| spawn_cwd.to_path_buf());
            roots.push(canonical);
        }

        roots.sort();
        roots.dedup();

        Ok(Self { roots })
    }
}

/// Canonicalise `target` if it exists; otherwise walk parents until one
/// canonicalises and append the missing tail. This lets the boundary
/// admit writes to not-yet-existing files inside a covered root while
/// still rejecting paths outside it.
///
/// Returns the canonical-or-best-effort absolute path.
fn canonicalize_or_lexical(system: &dyn System, target: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = system.canonicalize(target) {
        return Ok(canonical);
    }

    // Walk up until canonicalize succeeds, recording the suffix to
    // re-attach. The loop terminates because `parent()` shrinks each
    // iteration; if we run out of ancestors we fall back to the
    // original input (treat it as already-absolute).
    let mut suffix = PathBuf::new();
    let mut cursor = target.to_path_buf();
    while let Some(parent) = cursor.parent().map(Path::to_path_buf) {
        if parent.as_os_str().is_empty() {
            break;
        }
        let tail = cursor
            .file_name()
            .map(PathBuf::from)
            .with_context(|| format!("path missing file component: {}", target.display()))?;
        suffix = tail.join(&suffix);
        if let Ok(canonical_parent) = system.canonicalize(&parent) {
            return Ok(canonical_parent.join(suffix));
        }
        cursor = parent;
    }

    Ok(target.to_path_buf())
}
