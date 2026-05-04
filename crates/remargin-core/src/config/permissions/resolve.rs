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
//! ## `trusted_roots` narrowing + containment (rem-egp9)
//!
//! Unlike `restrict` / `deny_ops` / `allow_dot_folders`, which
//! accumulate across the parent walk, `trusted_roots` *narrow* as the
//! walk descends:
//!
//! - The outermost (shallowest) declaration sets the initial set.
//! - Each descendant declaration intersects the running set: entries
//!   that are not subsets of the parent's set are rejected at parse
//!   time with a hard error citing the offending path and source file.
//! - Each declared entry must be the declaring `.remargin.yaml`'s
//!   parent directory itself, or a subfolder of it (containment rule).
//!   A folder cannot bless a sibling.
//!
//! Containment + symlink handling: the resolver canonicalizes both the
//! declared entry and the declaring file's parent directory before the
//! containment check so a symlink that points outside the declaring
//! folder is rejected. When canonicalization fails (path does not
//! exist yet, mock FS), the expanded-but-uncanonicalized form is used
//! for the check — best-effort, the same fallback the per-entry
//! resolver uses elsewhere.
//!
//! When no `.remargin.yaml` is found anywhere on the walk, OR when the
//! walk found configs but none of them declared `trusted_roots`, the
//! effective set falls back to `[cwd]`. That preserves open semantics
//! for the no-config case and lets a user limit the visible surface
//! by simply changing directories.
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
use crate::config::permissions::op_name::OpName;
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

/// A single permissions-config parse failure scoped to one `.remargin.yaml`.
///
/// Surfaces the offending file, message, and (when available) line / column
/// from the underlying `serde_yaml` error. Used by
/// [`lint_permissions_in_parents`] and the public `lint` surfaces
/// (CLI / MCP).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionsLintError {
    /// 1-indexed column where the offending value starts; `None`
    /// when `serde_yaml` did not surface a location.
    pub column: Option<usize>,

    /// 1-indexed line where the offending value starts; `None` when
    /// `serde_yaml` did not surface a location.
    pub line: Option<usize>,

    /// User-facing diagnostic — the raw `serde_yaml` message, which
    /// already names the offending value and lists the valid ops on
    /// an unknown-variant failure.
    pub message: String,

    /// Absolute path of the `.remargin.yaml` that failed to parse.
    pub source_file: PathBuf,
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
    /// Op names to deny on `path`. Validated at parse time via
    /// [`OpName`].
    pub ops: Vec<OpName>,

    /// Resolved absolute path.
    pub path: PathBuf,

    /// `.remargin.yaml` that declared the entry.
    pub source_file: PathBuf,

    /// Optional identity filter (rem-egp9). Empty means "all
    /// identities" — back-compat with pre-rem-egp9 `deny_ops` entries.
    /// Honored only when the realm is in strict mode; open-mode
    /// realms ignore the filter and `lint_permissions_in_parents`
    /// surfaces a warning.
    pub to: Vec<String>,
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

    /// Carried verbatim from the on-disk entry. Purely deny-side
    /// (rem-si27): suppresses the projected `Bash(remargin *)` deny
    /// rule when `true`; never adds anything to the allow list.
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
) -> Result<()> {
    let source_dir = source_file.parent().unwrap_or(source_file);

    for raw in &block.trusted_roots {
        let path = resolve_trusted_root(system, raw);
        // Containment: the declared entry must be the declaring
        // file's parent directory, or a subfolder of it. Canonicalize
        // both sides to handle symlinks; fall back to the expanded
        // form when realpath fails (mock FS, dangling path, etc.).
        let canonical_dir = canonicalize_or_passthrough(system, source_dir.to_path_buf());
        if !path_covers(&canonical_dir, &path) {
            anyhow::bail!(
                "trusted_root {} declared in {} is outside the declaring \
                 directory {} (entries must be the declaring folder or a \
                 subfolder)",
                path.display(),
                source_file.display(),
                canonical_dir.display(),
            );
        }
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
            to: entry.to.clone(),
        });
    }

    if !block.allow_dot_folders.is_empty() {
        acc.allow_dot_folders.push(ResolvedAllowDotFolders {
            names: block.allow_dot_folders.clone(),
            source_file: source_file.to_path_buf(),
        });
    }

    Ok(())
}

/// `true` when `target` equals `anchor` or is a descendant of it. Local
/// helper so this module does not have to depend on `op_guard`.
fn path_covers(anchor: &Path, target: &Path) -> bool {
    target == anchor || target.starts_with(anchor)
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

/// Walk parents of `start_dir` and lint each `.remargin.yaml`'s permissions block.
///
/// Surfaces any deserialisation errors (including unknown op names in
/// `permissions.deny_ops.ops`) as a structured list. The walk does NOT
/// short-circuit on the first failure — every offending file is
/// reported so the user fixes them in one pass.
///
/// rem-egp9: also surfaces a non-fatal "`deny_ops` `to:` is ignored
/// in non-strict mode" warning per offending entry — open / registered
/// mode realms cannot trust the caller's declared identity (it is
/// trivially spoofed via `--identity` flags), so the per-op guard
/// ignores the `to:` filter and emits a wider deny than the user
/// likely intended. Strict-mode realms validate identity against the
/// registry + signing key, so the filter is meaningful there.
///
/// I/O failures (e.g. read errors) are propagated up as `Err`; only
/// parse failures and lint warnings become `PermissionsLintError`s.
///
/// # Errors
///
/// I/O failure while walking the parent chain or reading any
/// `.remargin.yaml` on the path.
pub fn lint_permissions_in_parents(
    system: &dyn System,
    start_dir: &Path,
) -> Result<Vec<PermissionsLintError>> {
    use crate::config::{Mode, resolve_mode};
    let realm_mode = resolve_mode(system, start_dir).map_or(Mode::Open, |r| r.mode);
    let mut findings = Vec::new();
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
            match serde_yaml::from_str::<PermissionsOnly>(&raw) {
                Ok(projection) => {
                    if !matches!(realm_mode, Mode::Strict) {
                        for entry in projection.permissions.deny_ops {
                            if !entry.to.is_empty() {
                                findings.push(PermissionsLintError {
                                    column: None,
                                    line: None,
                                    message: format!(
                                        "deny_ops 'to:' on path '{}' is ignored in non-strict mode (realm mode is {:?}); the deny will fire for every identity",
                                        entry.path,
                                        realm_mode,
                                    ),
                                    source_file: candidate.clone(),
                                });
                            }
                        }
                    }
                }
                Err(err) => {
                    let location = err.location();
                    findings.push(PermissionsLintError {
                        column: location.as_ref().map(serde_yaml::Location::column),
                        line: location.as_ref().map(serde_yaml::Location::line),
                        message: err.to_string(),
                        source_file: candidate.clone(),
                    });
                }
            }
        }

        if !current.pop() {
            break;
        }
    }

    Ok(findings)
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
/// `trusted_roots` are narrowed across the walk per rem-egp9 — see the
/// module-level docs. The final `acc.trusted_roots` carries one entry
/// per surviving `(path, source_file)` tuple from the deepest level
/// that actually constrained the set; entries that violate
/// intersection or containment surface as parse-time errors.
///
/// # Errors
///
/// - I/O failure while checking existence of or reading any
///   `.remargin.yaml` on the walk.
/// - YAML parse failure on any `.remargin.yaml` on the walk; the error
///   message includes the file path.
/// - Unknown fields under `permissions:` are rejected (the on-disk
///   structs use `#[serde(deny_unknown_fields)]`).
/// - A `trusted_roots` entry that is not inside the declaring file's
///   parent directory (containment violation; rem-egp9).
/// - A `trusted_roots` entry on a child `.remargin.yaml` that is not a
///   subset of the parent's set (intersection violation; rem-egp9).
pub fn resolve_permissions(system: &dyn System, start_dir: &Path) -> Result<ResolvedPermissions> {
    let mut acc = ResolvedPermissions::default();
    // Track the per-source-file trusted_roots declarations in walk
    // order (deepest first) so we can apply intersection narrowing
    // outermost-to-deepest after the walk completes.
    let mut trusted_root_levels: Vec<(PathBuf, Vec<TrustedRoot>)> = Vec::new();
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
            // Snapshot acc.trusted_roots before extend_resolved appends
            // this file's entries so we can capture the per-file slice.
            let prior_trusted_len = acc.trusted_roots.len();
            extend_resolved(&mut acc, system, &block, &candidate)?;
            if acc.trusted_roots.len() > prior_trusted_len {
                let level: Vec<TrustedRoot> = acc.trusted_roots[prior_trusted_len..].to_vec();
                trusted_root_levels.push((candidate.clone(), level));
            }
        }

        if !current.pop() {
            break;
        }
    }

    // Apply trusted_roots narrowing across the walk (rem-egp9).
    // `extend_resolved` already appended every declared entry to
    // `acc.trusted_roots` in walk order. Replace that flat list with
    // the narrowed set: outermost declaration wins; descendants must
    // be subsets.
    acc.trusted_roots = narrow_trusted_roots(&trusted_root_levels)?;

    Ok(acc)
}

/// Apply the rem-egp9 intersection rule to `trusted_roots` declarations
/// gathered during the parent walk.
///
/// `levels` is in walk order (deepest first). The outermost
/// declaration sets the initial set; each descendant level narrows it
/// by intersection (each child entry must be a subset of some entry in
/// the parent's set, otherwise we hard-error citing the offending
/// path + source file).
fn narrow_trusted_roots(levels: &[(PathBuf, Vec<TrustedRoot>)]) -> Result<Vec<TrustedRoot>> {
    if levels.is_empty() {
        return Ok(Vec::new());
    }
    // levels are deepest-first; reverse to outermost-first so we can
    // apply narrowing top-down.
    let mut iter = levels.iter().rev();
    let Some((_first_file, first_level)) = iter.next() else {
        return Ok(Vec::new());
    };
    let mut current_set: Vec<TrustedRoot> = first_level.clone();
    for (child_file, child_level) in iter {
        let mut narrowed: Vec<TrustedRoot> = Vec::new();
        for child_entry in child_level {
            let covered_by_parent = current_set
                .iter()
                .any(|parent| path_covers(&parent.path, &child_entry.path));
            if !covered_by_parent {
                let parent_paths: Vec<String> = current_set
                    .iter()
                    .map(|p| p.path.display().to_string())
                    .collect();
                anyhow::bail!(
                    "trusted_root {} declared in {} is not a subset of the \
                     parent set [{}]; descendant declarations may only narrow",
                    child_entry.path.display(),
                    child_file.display(),
                    parent_paths.join(", "),
                );
            }
            narrowed.push(child_entry.clone());
        }
        current_set = narrowed;
    }
    Ok(current_set)
}

/// Resolve `trusted_roots` against a parent walk (rem-egp9).
///
/// When no `.remargin.yaml` declared `trusted_roots` anywhere on the
/// walk, falls back to `[cwd]` — open semantics for the
/// no-declaration case.
///
/// The returned `Vec<PathBuf>` is the canonical absolute-path set the
/// per-op sandbox layer consults. Each path is canonicalized when
/// possible; the expanded form is kept otherwise.
///
/// # Errors
///
/// Surfaces the same parse-time errors as [`resolve_permissions`]
/// (containment + intersection violations).
pub fn resolve_trusted_roots_for_cwd(system: &dyn System, cwd: &Path) -> Result<Vec<PathBuf>> {
    let resolved = resolve_permissions(system, cwd)?;
    if resolved.trusted_roots.is_empty() {
        // No declaration anywhere on the walk → CWD fallback.
        Ok(vec![canonicalize_or_passthrough(system, cwd.to_path_buf())])
    } else {
        Ok(resolved
            .trusted_roots
            .iter()
            .map(|tr| tr.path.clone())
            .collect())
    }
}

/// Expand `~` / `$VAR` then realpath; fall back to the expanded form
/// if canonicalization fails (path does not exist, mock FS lacks the
/// inode, etc.). Errors from [`expand_path`] also fall back to the raw
/// string — the resolver is best-effort by design (see module docs).
fn resolve_trusted_root(system: &dyn System, raw: &str) -> PathBuf {
    let expanded = expand_path(system, raw).unwrap_or_else(|_err| PathBuf::from(raw));
    canonicalize_or_passthrough(system, expanded)
}
