//! Three-branch identity resolver (rem-11u).
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
//!
//! Replaces the earlier field-by-field CLI overlay onto a walked config,
//! which let rem-ce4 silently misattribute by mixing halves of two
//! different identities.

use core::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;

use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::config::{Config, Mode, parse_author_type, resolve_key_path};
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
    /// Explicit author type (`human` or `agent`).
    pub author_type: Option<AuthorType>,

    /// Explicit `--config <path>` pointing at the file that declares the
    /// identity. `~` and `$VAR` are expanded by the adapter before this
    /// reaches the resolver.
    pub config_path: Option<PathBuf>,

    /// Explicit identity (author) name.
    pub identity: Option<String>,

    /// Explicit signing key path. `~` / `$VAR` expanded by the adapter.
    /// Bare-name shorthand (`mykey` → `~/.ssh/mykey`) is still resolved
    /// by [`resolve_key_path`] inside this module.
    pub key: Option<String>,
}

impl IdentityFlags {
    /// Construct a flags struct that names only `--config <path>`.
    ///
    /// Convenience for adapters (CLI / MCP) that already have a path
    /// they want to push through branch 1 of the resolver and don't
    /// need the full default-and-mutate dance — the struct is
    /// `#[non_exhaustive]` so out-of-crate callers can't build it via
    /// the literal expression syntax.
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
    /// Branch 1: `--config <path>`. The `PathBuf` is the file that
    /// declared the identity.
    ConfigFlag(PathBuf),
    /// Branch 2: manual declaration via `--identity` + `--type`
    /// (+ `--key` when strict). No file was consulted.
    Manual,
    /// Branch 3: walk-up from CWD. The `PathBuf` is the file that
    /// matched all supplied filters.
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

/// Resolve the effective identity for `cwd` under the given [`Mode`]
/// using the three-branch flow described in the module docs.
///
/// `registry` is used for membership checks in registered/strict mode
/// (branches 1 and 2 always check; branch 3 checks after the walk
/// matches). It may be `None` when mode is `Open`.
///
/// # Errors
///
/// Every error path in the three-branch flow:
///
/// - Branch 1: `--config` file cannot be read, parsed, or is missing
///   required fields (`identity`, `type`, or `key` when strict); when
///   mode is registered/strict, the declared identity is not active in
///   the registry.
/// - Branch 2: `--identity` + `--type` not both supplied; `--key`
///   missing when strict; author type is not `human` or `agent`; same
///   registry membership check as branch 1.
/// - Branch 3: walk exhausts without finding a file that matches every
///   supplied filter; the matching file is then run through the same
///   validation as branch 1 and may fail there too.
pub fn resolve_identity(
    system: &dyn System,
    cwd: &Path,
    mode: &Mode,
    flags: &IdentityFlags,
    registry: Option<&Registry>,
) -> Result<ResolvedIdentity> {
    // Belt-and-braces check: the clap layer declares
    // `conflicts_with_all`, but non-clap adapters could in theory
    // construct the forbidden combination.
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

    // Branch 2 is entered ONLY when --identity AND --type are both
    // given (AND --key when strict). Anything else — including
    // "--identity alone", "--type alone", or "--key alone" — falls
    // through to branch 3 filtered walk. This is what lets a caller
    // say "find the walked config belonging to alice" without also
    // re-declaring alice's full identity.
    if is_complete_manual_declaration(mode, flags) {
        return resolve_from_manual(system, mode, flags, registry);
    }

    resolve_from_walk(system, cwd, mode, flags, registry)
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

/// Read + parse a `.remargin.yaml` at `path`.
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

/// Anchor a `key:` value to the config file's directory when it would
/// otherwise resolve against CWD.
///
/// `resolve_key_path` only handles `~` / `$VAR` expansion; relative
/// paths like `.remargin/agent_key` pass through unchanged and are
/// later resolved by the OS against the process's CWD. That works by
/// accident when the operator's own config is found by walking up from
/// CWD (config dir == CWD), but fails for any config loaded by absolute
/// path (e.g. `--config /elsewhere/.remargin.yaml`) where the relative
/// `key:` path is meant to be relative to the config file, not the CWD.
///
/// This helper prepends `source_path.parent()` when the resolved key
/// path is still relative. Absolute paths (and paths that started with
/// `~` / `$` and were already expanded to absolute) pass through
/// unchanged.
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

/// In registered/strict mode, the declared identity must correspond to
/// an `active` registry entry. Used by every branch.
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
