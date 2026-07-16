//! Parent-walk resolver for the `permissions:` block. Pure data — no
//! enforcement.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::Deserialize;

use crate::config::permissions::Permissions;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::{DenyOpsItem, DenyOpsItemFull};
use crate::path::expand_path;

const CONFIG_FILENAME: &str = ".remargin.yaml";

const LEGACY_TO_MIGRATION_HINT: &str = "legacy `to:` field on deny_ops is removed; replace entry-level `to: [identities]` with per-op `exceptions: [identities]` on each item in `ops:` (deny EXCEPT for the listed identities)";

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

/// One entry per declaring `.remargin.yaml` so diagnostic surfaces
/// can name the file that contributed each declaration.
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
    /// Per-op items to deny on `path`. Each item is either a blanket
    /// deny (empty `exceptions`) or a deny with an identity allowlist.
    pub ops: Vec<ResolvedDenyOpsItem>,

    /// Resolved absolute path.
    pub path: PathBuf,

    /// `.remargin.yaml` that declared the entry.
    pub source_file: PathBuf,
}

/// A single resolved per-op deny rule.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedDenyOpsItem {
    /// Identities exempt from this deny. Empty = blanket deny.
    pub exceptions: Vec<String>,

    pub name: OpName,
}

/// Accumulated permissions across every `.remargin.yaml` between
/// `start_dir` and `/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedPermissions {
    pub allow_dot_folders: Vec<ResolvedAllowDotFolders>,

    /// Effective folder-level CLI policy resolved by nearest-wins
    /// parent-walk. `None` means no `.remargin.yaml` in the walk
    /// declared `cli_allowed`; callers treat `None` as allowed
    /// (effective default = true).
    pub cli_allowed: Option<bool>,

    pub deny_ops: Vec<ResolvedDenyOps>,

    /// Walk order, deepest first.
    pub trusted_roots: Vec<ResolvedTrustedRoot>,

    /// `Some` = some `.remargin.yaml` in the walk declared
    /// `trusted_roots: []`, locking the realm. Records the deepest
    /// locker so refusal messages can name it.
    pub trusted_roots_lock: Option<PathBuf>,
}

impl ResolvedPermissions {
    #[must_use]
    pub fn allow_dot_folder_names(&self) -> Vec<String> {
        self.allow_dot_folders
            .iter()
            .flat_map(|entry| entry.names.iter().cloned())
            .collect()
    }

    /// Effective CLI policy: `true` = CLI allowed (default when absent).
    /// Nearest-wins declaration in the parent walk wins; absent = allowed.
    #[must_use]
    pub const fn cli_allowed(&self) -> bool {
        match self.cli_allowed {
            Some(v) => v,
            None => true,
        }
    }

    /// A realm that locked itself to an empty allow-set: some walked
    /// `.remargin.yaml` declared `trusted_roots: []` and no entry
    /// survived. The op guard treats this as deny-all; the pretool hook
    /// mirrors it so a locked realm is unreachable through a native tool
    /// or a shell word.
    #[must_use]
    pub const fn locked_to_empty_roots(&self) -> bool {
        self.trusted_roots.is_empty() && self.trusted_roots_lock.is_some()
    }

    /// No opinion stated: every walked file was silent and nothing
    /// locked the realm. Callers fall back to cwd.
    #[must_use]
    pub const fn trusted_roots_unconstrained(&self) -> bool {
        self.trusted_roots.is_empty() && self.trusted_roots_lock.is_none()
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

    // Nearest-wins: only record the first declaration found (deepest,
    // since walk is deepest-first). Shallower files are skipped when
    // a deeper one already set the policy.
    if acc.cli_allowed.is_none() && block.cli_allowed.is_some() {
        acc.cli_allowed = block.cli_allowed;
    }

    if let Some(entries) = block.trusted_roots.as_ref() {
        // First observed lock = deepest, since walk is deepest-first.
        if entries.is_empty() && acc.trusted_roots_lock.is_none() {
            acc.trusted_roots_lock = Some(source_file.to_path_buf());
        }
        for entry in entries {
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
    }

    for entry in &block.deny_ops {
        let path = resolve_relative(system, source_dir, &entry.path);
        let ops = entry.ops.iter().map(resolve_deny_ops_item).collect();
        acc.deny_ops.push(ResolvedDenyOps {
            ops,
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

fn resolve_deny_ops_item(item: &DenyOpsItem) -> ResolvedDenyOpsItem {
    match item {
        DenyOpsItem::Bare(name) => ResolvedDenyOpsItem {
            exceptions: Vec::new(),
            name: *name,
        },
        DenyOpsItem::Full(DenyOpsItemFull { name, exceptions }) => ResolvedDenyOpsItem {
            exceptions: exceptions.clone(),
            name: *name,
        },
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

/// Walk parents of `start_dir` and lint each `.remargin.yaml`.
///
/// Doesn't short-circuit — every offending file is reported in one
/// pass. Flags any legacy `to:` field on `deny_ops` entries as a
/// hard error with the migration recipe.
///
/// # Errors
///
/// I/O failure while walking the parent chain or reading any
/// `.remargin.yaml` on the path.
pub fn lint_permissions_in_parents(
    system: &dyn System,
    start_dir: &Path,
) -> Result<Vec<PermissionsLintError>> {
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
            collect_legacy_to_findings(&raw, &candidate, &mut findings);
            if let Err(err) = serde_yaml::from_str::<PermissionsOnly>(&raw) {
                let location = err.location();
                findings.push(PermissionsLintError {
                    column: location.as_ref().map(serde_yaml::Location::column),
                    line: location.as_ref().map(serde_yaml::Location::line),
                    message: err.to_string(),
                    source_file: candidate.clone(),
                });
            }
        }

        if !current.pop() {
            break;
        }
    }

    Ok(findings)
}

fn collect_legacy_to_findings(
    raw: &str,
    candidate: &Path,
    findings: &mut Vec<PermissionsLintError>,
) {
    let Ok(value): Result<serde_yaml::Value, _> = serde_yaml::from_str(raw) else {
        return;
    };
    let Some(permissions) = value.get("permissions").and_then(|v| v.as_mapping()) else {
        return;
    };
    let Some(deny_ops) = permissions
        .get(serde_yaml::Value::String(String::from("deny_ops")))
        .and_then(|v| v.as_sequence())
    else {
        return;
    };
    let to_key = serde_yaml::Value::String(String::from("to"));
    for entry in deny_ops {
        let Some(mapping) = entry.as_mapping() else {
            continue;
        };
        if mapping.contains_key(&to_key) {
            findings.push(PermissionsLintError {
                column: None,
                line: None,
                message: String::from(LEGACY_TO_MIGRATION_HINT),
                source_file: candidate.to_path_buf(),
            });
        }
    }
}

/// Walk up from `start_dir`, parse every `.remargin.yaml`, accumulate
/// `permissions:` blocks. Order is deepest-first.
///
/// # Errors
///
/// I/O or YAML parse failure on any `.remargin.yaml` in the walk.
/// Unknown fields under `permissions:` are rejected.
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

/// MCP / `allowlist::resolve_sandboxed` boundary set for `cwd`.
/// Unconstrained → `[cwd]`. Locked or constrained → exactly the
/// resolved entries.
///
/// # Errors
///
/// Surfaces the same parse-time errors as [`resolve_permissions`].
pub fn resolve_trusted_roots_for_cwd(system: &dyn System, cwd: &Path) -> Result<Vec<PathBuf>> {
    let resolved = resolve_permissions(system, cwd)?;
    if resolved.trusted_roots_unconstrained() {
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
