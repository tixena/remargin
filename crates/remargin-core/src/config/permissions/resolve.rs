//! Parent-walk resolver for the `permissions:` block. Pure data — no
//! enforcement.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::Deserialize;

use crate::config::permissions::Permissions;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::{DenyOpsItem, DenyOpsItemFull};
use crate::path::expand_path;

const CONFIG_FILENAME: &str = ".remargin.yaml";

const LEGACY_TO_MIGRATION_HINT: &str = "legacy `to:` field on deny_ops is removed; replace entry-level `to: [identities]` with per-op `exceptions: [identities]` on each item in `ops:` (deny EXCEPT for the listed identities)";

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
    /// 1-indexed; `None` when `serde_yaml` gave no location.
    pub column: Option<usize>,

    /// 1-indexed; `None` when `serde_yaml` gave no location.
    pub line: Option<usize>,

    pub message: String,

    pub source_file: PathBuf,
}

/// One entry per declaring `.remargin.yaml` so diagnostic surfaces
/// can name the file that contributed each declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedAllowDotFolders {
    pub names: Vec<String>,

    pub source_file: PathBuf,
}

/// A `deny_ops` entry after path resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedDenyOps {
    pub ops: Vec<ResolvedDenyOpsItem>,

    pub path: PathBuf,

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

    /// `None` when no walked file declared it; callers treat that as denied.
    pub cli_allowed: Option<bool>,

    pub deny_ops: Vec<ResolvedDenyOps>,

    /// Walk order, deepest first.
    pub trusted_roots: Vec<ResolvedTrustedRoot>,

    /// The deepest `.remargin.yaml` that declared `trusted_roots: []`, locking the realm.
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

    /// Effective CLI policy: `true` = CLI allowed for agents. The
    /// nearest declaration in the parent walk wins; when no
    /// `.remargin.yaml` in the walk declares it, the CLI is denied.
    #[must_use]
    pub const fn cli_allowed(&self) -> bool {
        match self.cli_allowed {
            Some(v) => v,
            None => false,
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

/// A `trusted_roots` entry after path resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedTrustedRoot {
    pub also_deny_bash: Vec<String>,

    /// When `true`, suppress the projected `Bash(remargin *)` deny.
    pub cli_allowed: bool,

    pub path: TrustedRootPath,

    pub source_file: PathBuf,
}

/// A `trusted_roots` entry whose resolved anchor escapes the realm that
/// declared it.
///
/// Covers an out-of-realm absolute path, a `../` escape, or a `~`/`$VAR`
/// expansion landing outside. Fail-closed at resolve time; also surfaced
/// by lint and doctor so the misconfig is named, not silently inverted
/// into protecting a sibling while denying the realm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrustedRootEscape {
    pub anchor: PathBuf,

    pub entry: String,

    pub realm_dir: PathBuf,

    pub source_file: PathBuf,
}

impl TrustedRootEscape {
    /// Diagnostic naming the file, the entry as written, the resolved
    /// anchor, and the realm it escaped. Shared verbatim by the
    /// resolve-time error, the lint finding, and the doctor finding.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "trusted_roots entry `{}` in {} resolves to {}, outside the realm it declares ({}); a \
             trusted root must resolve at or below the directory of the .remargin.yaml that \
             declares it",
            self.entry,
            self.source_file.display(),
            self.anchor.display(),
            self.realm_dir.display(),
        )
    }
}

/// Where a trusted root points: one absolute path, or the whole realm of the declaring file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrustedRootPath {
    Absolute(PathBuf),
    Wildcard { realm_root: PathBuf },
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
    // Fail closed before recording anything: an out-of-realm anchor would protect a sibling and
    // deny the realm's own files.
    if let Some(escape) = block_trusted_root_escapes(system, block, source_file)
        .into_iter()
        .next()
    {
        anyhow::bail!("{}", escape.message());
    }

    let source_dir = source_file.parent().unwrap_or(source_file);

    // Nearest wins: the walk is deepest-first, so only the first declaration is recorded.
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

    Ok(())
}

/// Every `trusted_roots` entry in `block` whose resolved anchor escapes
/// the declaring realm. Wildcard entries anchor at the realm root and are
/// inherently contained, so they are skipped. This is the one containment
/// engine — resolve (fail-closed), lint, and doctor all consult it.
fn block_trusted_root_escapes(
    system: &dyn System,
    block: &Permissions,
    source_file: &Path,
) -> Vec<TrustedRootEscape> {
    let source_dir = source_file.parent().unwrap_or(source_file);
    let Some(entries) = block.trusted_roots.as_ref() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let raw = entry.path();
            if raw == TRUSTED_ROOT_WILDCARD {
                return None;
            }
            let anchor = resolve_relative(system, source_dir, raw);
            (!anchor_within_realm(source_dir, &anchor)).then(|| TrustedRootEscape {
                anchor,
                entry: raw.to_owned(),
                realm_dir: source_dir.to_path_buf(),
                source_file: source_file.to_path_buf(),
            })
        })
        .collect()
}

/// `true` when `anchor` resolves at or below `source_dir`. Both sides are
/// lexically normalized first so a `../` escape survives `MemorySystem`'s
/// join-only `canonicalize` (which never collapses parent traversals).
fn anchor_within_realm(source_dir: &Path, anchor: &Path) -> bool {
    let realm = lexical_normalize(source_dir);
    let candidate = lexical_normalize(anchor);
    candidate == realm || candidate.starts_with(&realm)
}

/// Collapse `.` / `..` without touching disk. Leading `..` that cannot be
/// popped are preserved so an escape above the root never masquerades as
/// contained.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            Component::CurDir => {}
            Component::Normal(_) | Component::Prefix(_) | Component::RootDir => {
                parts.push(component);
            }
        }
    }
    parts.iter().collect()
}

/// Walk parents of `start_dir` and collect every out-of-realm
/// `trusted_roots` entry.
///
/// Read-only — parse failures are surfaced by the lint / resolve
/// surfaces, so a file that fails to parse contributes no escape here.
/// Lets doctor name the misconfig without triggering the fail-closed
/// resolve error.
///
/// # Errors
///
/// I/O failure while walking the parent chain or reading any
/// `.remargin.yaml` on the path.
pub fn find_trusted_root_escapes(
    system: &dyn System,
    start_dir: &Path,
) -> Result<Vec<TrustedRootEscape>> {
    let mut escapes = Vec::new();
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
            if let Ok(projection) = serde_yaml::from_str::<PermissionsOnly>(&raw) {
                escapes.extend(block_trusted_root_escapes(
                    system,
                    &projection.permissions,
                    &candidate,
                ));
            }
        }

        if !current.pop() {
            break;
        }
    }

    Ok(escapes)
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
            match serde_yaml::from_str::<PermissionsOnly>(&raw) {
                Ok(projection) => {
                    for escape in
                        block_trusted_root_escapes(system, &projection.permissions, &candidate)
                    {
                        findings.push(PermissionsLintError {
                            column: None,
                            line: None,
                            message: escape.message(),
                            source_file: candidate.clone(),
                        });
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
            extend_resolved(&mut acc, system, &block, &candidate)?;
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
