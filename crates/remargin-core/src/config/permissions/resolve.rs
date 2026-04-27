//! Parent-walk resolver for the `permissions:` block (rem-yj1j.1 / T22).
//!
//! Walks from a starting directory up to the filesystem root, parses
//! every `.remargin.yaml` found, and accumulates each file's
//! `permissions:` block into a single [`ResolvedPermissions`]. Each
//! resolved entry preserves the source-file path so downstream
//! diagnostics can name the `.remargin.yaml` that contributed a rule.
//!
//! Order: deepest (closest to `start_dir`) first within each `Vec`,
//! then progressively shallower files. Callers that want a
//! "closest declaration wins" semantics can take `.first()`.
//!
//! ## Path resolution
//!
//! - `trusted_roots` entries are passed through [`expand_path`] (so
//!   `~` and `$VAR` resolve), then through `System::canonicalize`
//!   when possible. If realpath fails (path does not exist yet, mock
//!   FS, etc.) the expanded form is kept as a best-effort absolute.
//! - `restrict.path == "*"` becomes [`RestrictPath::Wildcard`] anchored
//!   at the declaring file's parent directory.
//! - Other `restrict.path` and `deny_ops.path` resolve against
//!   `<source_file_dir>/<path>`. Realpath when possible; raw join
//!   otherwise.
//! - `allow_dot_folders` strings are accumulated as-is (no path math).
//!
//! ## No enforcement
//!
//! This module is intentionally pure data. The Layer 1 op guard,
//! Claude-settings synchronizer, and CLI surfaces all consume this
//! resolver's output but live in their own modules / sibling tasks.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::Deserialize;

use crate::config::permissions::Permissions;
use crate::path::expand_path;

const CONFIG_FILENAME: &str = ".remargin.yaml";

/// Wildcard sentinel preserved from `restrict.path = "*"`.
const RESTRICT_WILDCARD: &str = "*";

/// Minimal projection used to extract just the `permissions:` block
/// from a `.remargin.yaml` without coupling to the full
/// [`crate::config::Config`] schema. Other top-level keys are ignored
/// at this layer — full validation happens through the existing
/// [`crate::config::Config`] loader.
#[derive(Debug, Default, Deserialize)]
struct PermissionsOnly {
    #[serde(default)]
    permissions: Permissions,
}

/// A grouped `allow_dot_folders` declaration after path resolution.
///
/// One entry per declaring `.remargin.yaml` (rem-qdrw): the resolver
/// used to flatten every file's list into one `Vec<String>`, which lost
/// the `source_file` provenance that `restrict` / `deny_ops` already
/// carried. Keeping a per-file group preserves the same shape used by
/// the rest of the resolved-permissions structure and lets diagnostic
/// surfaces (e.g. `permissions show --json`) name the file that
/// contributed each declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedAllowDotFolders {
    /// Folder names declared in the source file's
    /// `allow_dot_folders:` list, in declaration order.
    pub names: Vec<String>,

    /// `.remargin.yaml` that declared the entry.
    pub source_file: PathBuf,
}

/// A `deny_ops` entry after path resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedDenyOps {
    /// Op names to deny on `path`.
    pub ops: Vec<String>,

    /// Resolved absolute path.
    pub path: PathBuf,

    /// `.remargin.yaml` that declared the entry.
    pub source_file: PathBuf,
}

/// Accumulated permissions across every `.remargin.yaml` between
/// `start_dir` and `/`. Each entry remembers its source file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedPermissions {
    /// Per-file dot-folder allow-list groups in walk order (deepest
    /// file first). Each [`ResolvedAllowDotFolders`] preserves the
    /// `.remargin.yaml` that declared it; the flattened name list is
    /// available through [`ResolvedPermissions::allow_dot_folder_names`].
    pub allow_dot_folders: Vec<ResolvedAllowDotFolders>,

    /// Per-path op denials in walk order (deepest file first).
    pub deny_ops: Vec<ResolvedDenyOps>,

    /// Restrict entries in walk order (deepest file first).
    pub restrict: Vec<ResolvedRestrict>,

    /// Trusted roots in walk order (deepest file first).
    pub trusted_roots: Vec<TrustedRoot>,
}

impl ResolvedPermissions {
    /// Flattened view of every declared dot-folder name across all
    /// declaring files, preserving walk order. Equivalent to the old
    /// `allow_dot_folders: Vec<String>` shape; consumers that only
    /// care about names (e.g. the op guard) call this.
    #[must_use]
    pub fn allow_dot_folder_names(&self) -> Vec<String> {
        self.allow_dot_folders
            .iter()
            .flat_map(|entry| entry.names.iter().cloned())
            .collect()
    }
}

/// A `restrict` entry after path resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedRestrict {
    /// Carried verbatim from the on-disk entry. See
    /// [`crate::config::permissions::RestrictEntry::also_deny_bash`].
    pub also_deny_bash: Vec<String>,

    /// Carried verbatim from the on-disk entry.
    pub cli_allowed: bool,

    /// Resolved path or wildcard-with-realm.
    pub path: RestrictPath,

    /// `.remargin.yaml` that declared the entry.
    pub source_file: PathBuf,
}

/// Resolved form of a `restrict.path`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RestrictPath {
    /// Concrete absolute path (canonicalized when possible).
    Absolute(PathBuf),
    /// `"*"` — applies to the entire realm anchored at the declaring
    /// `.remargin.yaml`'s parent directory.
    Wildcard {
        /// Parent directory of the declaring `.remargin.yaml`.
        realm_root: PathBuf,
    },
}

/// A `trusted_roots` entry after `~` / `$VAR` expansion and realpath
/// canonicalization (best-effort).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrustedRoot {
    /// Canonicalized absolute path. When the path does not exist on
    /// the active `System`, the expanded-but-uncanonicalized form is
    /// stored instead — verifying physical existence is the MCP
    /// startup checker's job (T24 / rem-yj1j.3), not this resolver.
    pub path: PathBuf,

    /// `.remargin.yaml` that declared the entry.
    pub source_file: PathBuf,
}

fn canonicalize_or_passthrough(system: &dyn System, path: PathBuf) -> PathBuf {
    system.canonicalize(&path).unwrap_or(path)
}

fn extend_resolved(
    acc: &mut ResolvedPermissions,
    system: &dyn System,
    block: &Permissions,
    source_file: &Path,
) {
    let source_dir = source_file.parent().unwrap_or(source_file);

    for raw in &block.trusted_roots {
        let path = resolve_trusted_root(system, raw);
        acc.trusted_roots.push(TrustedRoot {
            path,
            source_file: source_file.to_path_buf(),
        });
    }

    for entry in &block.restrict {
        let resolved_path = if entry.path == RESTRICT_WILDCARD {
            RestrictPath::Wildcard {
                realm_root: source_dir.to_path_buf(),
            }
        } else {
            RestrictPath::Absolute(resolve_relative(system, source_dir, &entry.path))
        };
        acc.restrict.push(ResolvedRestrict {
            also_deny_bash: entry.also_deny_bash.clone(),
            cli_allowed: entry.cli_allowed,
            path: resolved_path,
            source_file: source_file.to_path_buf(),
        });
    }

    for entry in &block.deny_ops {
        let path = resolve_relative(system, source_dir, &entry.path);
        acc.deny_ops.push(ResolvedDenyOps {
            ops: entry.ops.clone(),
            path,
            source_file: source_file.to_path_buf(),
        });
    }

    if !block.allow_dot_folders.is_empty() {
        acc.allow_dot_folders.push(ResolvedAllowDotFolders {
            names: block.allow_dot_folders.clone(),
            source_file: source_file.to_path_buf(),
        });
    }
}

fn parse_permissions_block(raw: &str, source_file: &Path) -> Result<Permissions> {
    let projection: PermissionsOnly =
        serde_yaml::from_str(raw).with_context(|| format!("parsing {}", source_file.display()))?;
    Ok(projection.permissions)
}

/// Resolve a relative-to-source path. Absolute inputs pass through to
/// canonicalization directly. Relative inputs are joined onto
/// `source_dir`. `~` / `$VAR` in either form are expanded first.
fn resolve_relative(system: &dyn System, source_dir: &Path, raw: &str) -> PathBuf {
    let expanded = expand_path(system, raw).unwrap_or_else(|_err| PathBuf::from(raw));
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        source_dir.join(expanded)
    };
    canonicalize_or_passthrough(system, absolute)
}

/// Walk up from `start_dir`, parse every `.remargin.yaml` found, and
/// accumulate the `permissions:` blocks. Returns the accumulated set
/// with source-file provenance.
///
/// Order is "deepest file first" — `restrict[0]` comes from the
/// `.remargin.yaml` closest to `start_dir`, with progressively
/// shallower files appended. The full set is preserved so callers
/// can audit every declaration without re-walking.
///
/// Files without a `permissions:` block contribute nothing (back-compat:
/// the field defaults to [`Permissions::default`]). Files that fail to
/// parse stop the walk and surface an error whose context names the
/// offending file.
///
/// # Errors
///
/// - I/O failure while checking existence of or reading any
///   `.remargin.yaml` on the walk.
/// - YAML parse failure on any `.remargin.yaml` on the walk; the error
///   message includes the file path.
/// - Unknown fields under `permissions:` are rejected (the on-disk
///   structs use `#[serde(deny_unknown_fields)]`).
pub fn resolve_permissions(system: &dyn System, start_dir: &Path) -> Result<ResolvedPermissions> {
    let mut acc = ResolvedPermissions::default();
    let mut current = start_dir.to_path_buf();

    loop {
        let candidate = current.join(CONFIG_FILENAME);
        let exists = system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?;

        if exists {
            let raw = system
                .read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let block = parse_permissions_block(&raw, &candidate)?;
            extend_resolved(&mut acc, system, &block, &candidate);
        }

        if !current.pop() {
            break;
        }
    }

    Ok(acc)
}

/// Expand `~` / `$VAR` then realpath; fall back to the expanded form
/// if canonicalization fails (path does not exist, mock FS lacks the
/// inode, etc.). Errors from [`expand_path`] also fall back to the raw
/// string — the resolver is best-effort by design (see module docs).
fn resolve_trusted_root(system: &dyn System, raw: &str) -> PathBuf {
    let expanded = expand_path(system, raw).unwrap_or_else(|_err| PathBuf::from(raw));
    canonicalize_or_passthrough(system, expanded)
}
