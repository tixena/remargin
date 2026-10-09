//! Three-branch identity resolver.
//!
//! A strict, disjoint flow that cannot produce a partially-inherited
//! identity. CLI args declare identity — they are either:
//!
//! 1. A complete identity declaration via `--config <path>` (branch 1).
//! 2. A complete manual declaration via `--identity` + `--type` (+ `--key`
//!    when mode is strict) (branch 2).
//! 3. Strict-equality filters on a directory walk (branch 3). Any of
//!    `--identity`, `--type`, `--key` that is supplied must match the
//!    candidate `.remargin.yaml`'s corresponding field; missing field in
//!    the file never matches a concrete value in the flag.

use core::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;

use serde::Serialize;

use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::config::{Config, Mode, ResolvedConfig, parse_author_type, resolve_key_path};
use crate::parser::AuthorType;

const CONFIG_FILENAME: &str = ".remargin.yaml";

/// CLI / adapter-layer shape of the four identity-affecting flags.
///
/// All fields are optional at the parse level; the resolver interprets
/// their combination to decide which branch applies. At the clap layer,
/// `config_path` is declared with `conflicts_with_all = [identity,
/// author_type, key]` so the "config plus manual" combination cannot
/// reach this struct in the first place — the resolver still defends
/// against it as a belt-and-braces check for non-clap adapters.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct IdentityFlags {
    pub author_type: Option<AuthorType>,

    /// Already expanded for `~` and `$VAR` by the adapter.
    pub config_path: Option<PathBuf>,

    pub identity: Option<String>,

    /// Already expanded for `~` and `$VAR`; a bare name still resolves under `~/.ssh`.
    pub key: Option<String>,
}

impl IdentityFlags {
    /// Construct a flags struct that names only `--config <path>`.
    #[must_use]
    pub const fn for_config_path(config_path: PathBuf) -> Self {
        Self {
            author_type: None,
            config_path: Some(config_path),
            identity: None,
            key: None,
        }
    }

    /// True when every field is `None` — the resolver takes branch 3
    /// (plain walk, no filters).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.author_type.is_none()
            && self.config_path.is_none()
            && self.identity.is_none()
            && self.key.is_none()
    }
}

/// A fully-resolved identity. Never partially populated: `identity` and
/// `author_type` are always present; `key_path` is present when the
/// caller-supplied [`Mode`] was [`Mode::Strict`], absent otherwise.
///
/// The `source` field records which branch produced the result, and
/// (for branches 1 and 3) the path of the file that declared the
/// identity. Adapters use this for diagnostics and tests.
///
/// The `source_config` field carries the parsed [`Config`] for branches 1
/// and 3 (the two branches that read a `.remargin.yaml`). It is `None`
/// for branch 2 (manual) because no file was consulted. Callers that
/// build a full [`crate::config::ResolvedConfig`] use this to pick up
/// `assets_dir`, `ignore`, and `mode` from the same file the identity
/// came from — without re-reading and re-parsing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedIdentity {
    pub author_type: AuthorType,
    pub identity: String,
    pub key_path: Option<PathBuf>,
    pub source: IdentitySource,
    pub source_config: Option<Config>,
}

/// Provenance of a [`ResolvedIdentity`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum IdentitySource {
    /// The file `--config` named.
    ConfigFlag(PathBuf),
    Manual,
    /// The file that matched every supplied filter.
    Walk(PathBuf),
}

impl fmt::Display for IdentitySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigFlag(path) => write!(f, "--config {}", path.display()),
            Self::Manual => write!(f, "manual CLI flags"),
            Self::Walk(path) => write!(f, "walk match at {}", path.display()),
        }
    }
}

/// Snapshot of the effective identity at a given `cwd`.
///
/// `found: false` is a soft miss (no config + empty flags, or walk
/// exhausted) — every other resolver error propagates as `Err` from
/// [`resolve_identity_report`].
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct IdentityReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_type: Option<String>,
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl IdentityReport {
    #[must_use]
    pub fn from_resolved(config: &ResolvedConfig) -> Self {
        let Some(identity) = config.identity.as_deref() else {
            return Self::not_found();
        };
        Self {
            author_type: config
                .author_type
                .as_ref()
                .map(|t| String::from(t.as_str())),
            found: true,
            identity: Some(identity.to_owned()),
            key: config.key_path.as_ref().map(|p| p.display().to_string()),
            mode: Some(String::from(config.mode.as_str())),
            path: config.source_path.as_ref().map(|p| p.display().to_string()),
        }
    }

    #[must_use]
    pub const fn not_found() -> Self {
        Self {
            author_type: None,
            found: false,
            identity: None,
            key: None,
            mode: None,
            path: None,
        }
    }
}

/// Resolves the effective identity for `cwd` under the given [`Mode`] through one of three
/// branches: `--config`, a complete manual declaration, or a filtered walk up from `cwd`.
///
/// `registry` is used for membership checks in registered/strict mode
/// (branches 1 and 2 always check; branch 3 checks after the walk
/// matches). It may be `None` when mode is `Open`.
///
/// # Errors
///
/// Returns an error when the declaring file cannot be read or lacks a field the mode requires, a
/// manual declaration is incomplete, the walk matches no file, or the identity is not active in
/// the registry.
pub fn resolve_identity(
    system: &dyn System,
    cwd: &Path,
    mode: &Mode,
    flags: &IdentityFlags,
    registry: Option<&Registry>,
) -> Result<ResolvedIdentity> {
    // Clap already refuses this combination; other adapters can still build it.
    if flags.config_path.is_some()
        && (flags.identity.is_some() || flags.author_type.is_some() || flags.key.is_some())
    {
        bail!(
            "--config conflicts with --identity, --type, and --key: \
             pass one complete identity declaration, not a mix"
        );
    }

    if let Some(config_path) = &flags.config_path {
        return resolve_from_config_flag(system, config_path, mode, registry);
    }

    // A partial declaration falls through to the filtered walk, so a caller can ask for the walked
    // config belonging to one identity without redeclaring it.
    if is_complete_manual_declaration(mode, flags) {
        return resolve_from_manual(system, mode, flags, registry);
    }

    resolve_from_walk(system, cwd, mode, flags, registry)
}

/// Resolve the effective identity at `cwd` under `flags`. Walk
/// exhaustion with empty flags is a soft miss (startup-poll
/// friendly); every other resolver error propagates.
///
/// # Errors
///
/// Resolver errors other than the walk-exhaust soft miss.
pub fn resolve_identity_report(
    system: &dyn System,
    cwd: &Path,
    flags: &IdentityFlags,
) -> Result<IdentityReport> {
    match ResolvedConfig::resolve(system, cwd, flags, None) {
        Ok(cfg) => Ok(IdentityReport::from_resolved(&cfg)),
        Err(err) if flags.is_empty() || looks_like_walk_miss(&err) => {
            Ok(IdentityReport::not_found())
        }
        Err(err) => Err(err),
    }
}

/// True when `flags` contains a complete manual identity declaration
/// for the current `mode`. Used to choose between branch 2 (manual)
/// and branch 3 (filtered walk).
const fn is_complete_manual_declaration(mode: &Mode, flags: &IdentityFlags) -> bool {
    if flags.identity.is_none() || flags.author_type.is_none() {
        return false;
    }
    if matches!(mode, Mode::Strict) && flags.key.is_none() {
        return false;
    }
    true
}

/// Branch 1: `--config <path>` declares the identity.
fn resolve_from_config_flag(
    system: &dyn System,
    config_path: &Path,
    mode: &Mode,
    registry: Option<&Registry>,
) -> Result<ResolvedIdentity> {
    let config = read_and_parse_config(system, config_path)?;
    let (identity, author_type, key_path) =
        validate_declared_identity(system, &config, mode, config_path)?;
    check_registry_membership(&identity, mode, registry)?;
    Ok(ResolvedIdentity {
        author_type,
        identity,
        key_path,
        source: IdentitySource::ConfigFlag(config_path.to_path_buf()),
        source_config: Some(config),
    })
}

/// Branch 2: manual declaration via `--identity` + `--type` (+ `--key`).
fn resolve_from_manual(
    system: &dyn System,
    mode: &Mode,
    flags: &IdentityFlags,
    registry: Option<&Registry>,
) -> Result<ResolvedIdentity> {
    let Some(identity) = flags.identity.clone() else {
        bail!(
            "manual identity declaration requires --identity \
             (got --type without --identity)"
        );
    };
    let Some(author_type) = flags.author_type.clone() else {
        bail!(
            "manual identity declaration requires --type \
             (got --identity without --type)"
        );
    };

    let key_path = match (mode, flags.key.as_deref()) {
        (Mode::Strict, None) => bail!(
            "strict mode: --key is required alongside --identity and --type \
             for a manual identity declaration"
        ),
        (_, Some(key)) => Some(resolve_key_path(system, key)?),
        (_, None) => None,
    };

    check_registry_membership(&identity, mode, registry)?;
    Ok(ResolvedIdentity {
        author_type,
        identity,
        key_path,
        source: IdentitySource::Manual,
        source_config: None,
    })
}

/// Branch 3: walk upward from `cwd`; each supplied flag is a
/// strict-equality filter on the candidate file's corresponding field.
fn resolve_from_walk(
    system: &dyn System,
    cwd: &Path,
    mode: &Mode,
    flags: &IdentityFlags,
    registry: Option<&Registry>,
) -> Result<ResolvedIdentity> {
    let mut current = cwd.to_path_buf();
    loop {
        let candidate = current.join(CONFIG_FILENAME);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            let config = read_and_parse_config(system, &candidate)?;
            if walk_filter_matches(&config, flags) {
                let (identity, author_type, key_path) =
                    validate_declared_identity(system, &config, mode, &candidate)?;
                check_registry_membership(&identity, mode, registry)?;
                return Ok(ResolvedIdentity {
                    author_type,
                    identity,
                    key_path,
                    source: IdentitySource::Walk(candidate),
                    source_config: Some(config),
                });
            }
        }
        if !current.pop() {
            bail!(
                "no identity resolved: walked upward from {} to /, \
                 no .remargin.yaml matched the supplied filters",
                cwd.display(),
            );
        }
    }
}

fn read_and_parse_config(system: &dyn System, path: &Path) -> Result<Config> {
    let content = system
        .read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let config: Config =
        serde_yaml::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
    Ok(config)
}

/// Validate that a declared identity has every field the current mode
/// requires, and convert the raw config fields into typed values. Used
/// by branches 1 and 3.
fn validate_declared_identity(
    system: &dyn System,
    config: &Config,
    mode: &Mode,
    source_path: &Path,
) -> Result<(String, AuthorType, Option<PathBuf>)> {
    let Some(identity) = config.identity.clone() else {
        bail!(
            "{}: missing required `identity:` field",
            source_path.display(),
        );
    };
    let Some(author_type_str) = config.author_type.clone() else {
        bail!("{}: missing required `type:` field", source_path.display());
    };
    let author_type = parse_author_type(&author_type_str)
        .with_context(|| format!("in {}", source_path.display()))?;

    let key_path = match (mode, config.key.as_deref()) {
        (Mode::Strict, None) => bail!(
            "{}: strict mode requires `key:` field",
            source_path.display(),
        ),
        (_, Some(key)) => Some(anchor_key_path_to_config_dir(
            resolve_key_path(system, key)?,
            source_path,
        )),
        (_, None) => None,
    };

    Ok((identity, author_type, key_path))
}

/// Anchors a still-relative `key:` path to the directory of the config file that declared it, so
/// a config loaded by absolute path from another working directory finds its key. Absolute
/// paths, including expanded `~` and `$VAR` forms, pass through unchanged.
pub(crate) fn anchor_key_path_to_config_dir(key_path: PathBuf, source_path: &Path) -> PathBuf {
    if key_path.is_absolute() {
        return key_path;
    }
    match source_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(key_path),
        _ => key_path,
    }
}

/// Strict-equality filter match for branch 3.
///
/// A flag that is `None` does not filter. A flag that is `Some(value)`
/// requires the corresponding config field to be present AND equal.
/// Absent field in the config never matches a concrete value in the
/// flag — the walk continues.
fn walk_filter_matches(config: &Config, flags: &IdentityFlags) -> bool {
    if let Some(wanted) = &flags.identity
        && config.identity.as_deref() != Some(wanted.as_str())
    {
        return false;
    }
    if let Some(wanted) = &flags.author_type {
        let matches = config
            .author_type
            .as_deref()
            .and_then(|t| parse_author_type(t).ok())
            .as_ref()
            == Some(wanted);
        if !matches {
            return false;
        }
    }
    if let Some(wanted) = flags.key.as_deref()
        && config.key.as_deref() != Some(wanted)
    {
        return false;
    }
    true
}

/// Cheap heuristic for the branch-3 walk-exhaust error message
/// emitted by `resolve_identity`. Distinguishes "walk didn't match"
/// (soft - map to `found: false`) from every other resolver error
/// (hard - propagate).
fn looks_like_walk_miss(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    msg.contains("no identity resolved")
        || msg.contains("no .remargin.yaml matched the supplied filters")
}

/// Outside open mode the identity must be in the registry and not revoked.
fn check_registry_membership(
    identity: &str,
    mode: &Mode,
    registry: Option<&Registry>,
) -> Result<()> {
    if matches!(mode, Mode::Open) {
        return Ok(());
    }
    let Some(reg) = registry else {
        bail!("mode is {mode:?} but no .remargin-registry.yaml found on the walk");
    };
    let Some(participant) = reg.participants.get(identity) else {
        bail!("{identity:?} is not in the registry (mode: {mode:?})");
    };
    if participant.status == RegistryParticipantStatus::Revoked {
        bail!("{identity:?} has been revoked in the registry");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
