//! Parent-walk resolver for the `permissions:` block. Pure data — no
//! enforcement.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::Deserialize;

use crate::config::permissions::Permissions;
use crate::config::permissions::op_name::OpName;
use crate::path::expand_path;

const CONFIG_FILENAME: &str = ".remargin.yaml";

/// Wildcard sentinel preserved from `trusted_roots[].path = "*"`.
const TRUSTED_ROOT_WILDCARD: &str = "*";

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
/// `start_dir` and `/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedPermissions {
    pub allow_dot_folders: Vec<ResolvedAllowDotFolders>,

    pub deny_ops: Vec<ResolvedDenyOps>,

    /// Paths where remargin is sanctioned to operate, in walk order
    /// (deepest first). Non-empty → allow-list mode for the per-op
    /// guard; empty → open mode. Also drives the MCP sandbox boundary
    /// and Claude-side rule emission.
    pub trusted_roots: Vec<ResolvedTrustedRoot>,
}

impl ResolvedPermissions {
    #[must_use]
    pub fn allow_dot_folder_names(&self) -> Vec<String> {
        self.allow_dot_folders
            .iter()
            .flat_map(|entry| entry.names.iter().cloned())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedTrustedRoot {
    pub also_deny_bash: Vec<String>,

    /// When `true`, suppress the projected `Bash(remargin *)` deny.
    pub cli_allowed: bool,

    pub path: TrustedRootPath,

    pub source_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrustedRootPath {
    Absolute(PathBuf),
    /// `"*"` — the entire realm anchored at the declaring
    /// `.remargin.yaml`'s parent directory.
    Wildcard {
        realm_root: PathBuf,
    },
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

    for entry in &block.trusted_roots {
        let raw_path = entry.path();
        let resolved_path = if raw_path == TRUSTED_ROOT_WILDCARD {
            TrustedRootPath::Wildcard {
                realm_root: source_dir.to_path_buf(),
            }
        } else {
            TrustedRootPath::Absolute(resolve_relative(system, source_dir, raw_path))
        };
        acc.trusted_roots.push(ResolvedTrustedRoot {
            also_deny_bash: entry.also_deny_bash().to_vec(),
            cli_allowed: entry.cli_allowed(),
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
}

/// Anchor for a [`ResolvedTrustedRoot`] — its absolute path or its
/// realm root.
#[must_use]
pub fn trusted_root_anchor(entry: &ResolvedTrustedRoot) -> &Path {
    match &entry.path {
        TrustedRootPath::Absolute(p) => p.as_path(),
        TrustedRootPath::Wildcard { realm_root } => realm_root.as_path(),
    }
}

/// `true` when the entry covers `target` (descendant or exact match).
#[must_use]
pub fn trusted_root_covers(entry: &TrustedRootPath, target: &Path) -> bool {
    let anchor = match entry {
        TrustedRootPath::Absolute(p) => p.as_path(),
        TrustedRootPath::Wildcard { realm_root } => realm_root.as_path(),
    };
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

/// MCP / `allowlist::resolve_sandboxed` boundary set for `cwd`. Reads
/// `permissions.trusted_roots` from the parent walk; falls back to
/// `[cwd]` when none declared.
///
/// # Errors
///
/// Surfaces the same parse-time errors as [`resolve_permissions`].
pub fn resolve_trusted_roots_for_cwd(system: &dyn System, cwd: &Path) -> Result<Vec<PathBuf>> {
    let resolved = resolve_permissions(system, cwd)?;
    if resolved.trusted_roots.is_empty() {
        Ok(vec![canonicalize_or_passthrough(system, cwd.to_path_buf())])
    } else {
        Ok(resolved
            .trusted_roots
            .iter()
            .map(|entry| match &entry.path {
                TrustedRootPath::Absolute(p) => p.clone(),
                TrustedRootPath::Wildcard { realm_root } => realm_root.clone(),
            })
            .collect())
    }
}
