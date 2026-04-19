//! Configuration loader: `.remargin.yaml` walk-up resolution.

pub mod identity;
pub mod registry;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Deserialize;

use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::parser::AuthorType;
use crate::path::expand_path;

const CONFIG_FILENAME: &str = ".remargin.yaml";
const REGISTRY_FILENAME: &str = ".remargin-registry.yaml";

/// CLI overrides that take precedence over config file values.
///
/// Note: `mode` is intentionally absent. Mode is a property of the
/// directory tree (resolved by walking upward for the nearest
/// `.remargin.yaml`) and is not caller-overridable — allowing a flag
/// like `--mode open` would let an agent silently weaken enforcement on
/// a strict vault (rem-wws).
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct CliOverrides<'cli> {
    pub assets_dir: Option<&'cli str>,
    pub author_type: Option<&'cli str>,
    pub identity: Option<&'cli str>,
    pub key: Option<&'cli str>,
}

/// Parsed contents of a `.remargin.yaml` file.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Config {
    #[serde(default = "default_assets_dir")]
    pub assets_dir: String,
    #[serde(rename = "type")]
    pub author_type: Option<String>,
    pub identity: Option<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
    pub key: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: Mode,
}

/// Enforcement mode for the participant registry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Mode {
    Open,
    Registered,
    Strict,
}

impl Mode {
    /// Canonical lowercase name for the mode, matching the YAML
    /// representation and the CLI's JSON output.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Registered => "registered",
            Self::Strict => "strict",
        }
    }
}

/// The final resolved configuration after merging the config file, registry,
/// and CLI overrides.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedConfig {
    pub assets_dir: String,
    pub author_type: Option<AuthorType>,
    pub identity: Option<String>,
    pub ignore: Vec<String>,
    pub key_path: Option<PathBuf>,
    pub mode: Mode,
    pub registry: Option<Registry>,
    /// Only settable via CLI when compiled with `--features unrestricted`.
    pub unrestricted: bool,
}

impl ResolvedConfig {
    /// Check if a participant is allowed to post (mode + registry enforcement).
    ///
    /// Kept as a crate-private helper used by [`Self::resolve`] and
    /// [`Self::with_identity_overrides`] so every construction path runs
    /// the same registry gate. Op handlers do NOT call this directly
    /// (rem-xc8x); they consume a pre-validated [`ResolvedConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if the participant is not allowed to post in the current
    /// mode (e.g. unregistered in registered/strict mode, or revoked).
    pub(crate) fn can_post(&self, author: &str) -> Result<()> {
        match self.mode {
            Mode::Open => Ok(()),
            Mode::Registered | Mode::Strict => {
                let Some(reg) = &self.registry else {
                    bail!("mode is {:?} but no registry found", self.mode);
                };
                let Some(participant) = reg.participants.get(author) else {
                    bail!(
                        "author {author:?} is not registered (mode: {:?})",
                        self.mode
                    );
                };
                if participant.status == RegistryParticipantStatus::Revoked {
                    bail!("author {author:?} has been revoked");
                }
                Ok(())
            }
        }
    }

    /// Check if a comment must be signed (strict mode + registered participant).
    #[must_use]
    pub fn requires_signature(&self, author: &str) -> bool {
        if self.mode != Mode::Strict {
            return false;
        }
        let Some(reg) = &self.registry else {
            return false;
        };
        reg.participants
            .get(author)
            .is_some_and(|participant| participant.status == RegistryParticipantStatus::Active)
    }

    /// Build from config file, registry, and CLI overrides.
    /// CLI flags take precedence over config file values.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - An unknown author type string is provided
    /// - Key path resolution fails
    pub fn resolve(
        system: &dyn System,
        config: Option<Config>,
        reg: Option<Registry>,
        cli: &CliOverrides<'_>,
    ) -> Result<Self> {
        let base = config.unwrap_or_else(|| Config {
            assets_dir: default_assets_dir(),
            author_type: None,
            identity: None,
            ignore: Vec::new(),
            key: None,
            mode: default_mode(),
        });

        // If CLI overrides author_type to a different type than the config's,
        // an explicit identity must also be provided — otherwise the config's
        // identity (which belongs to a different role) would be silently
        // inherited.
        if let Some(cli_type) = cli.author_type
            && let Some(config_type) = &base.author_type
            && cli_type != config_type
            && cli.identity.is_none()
        {
            bail!(
                "author type override {cli_type:?} does not match config type \
                 {config_type:?}; provide an explicit --identity for the {cli_type} role",
            );
        }

        let identity = cli.identity.map(String::from).or(base.identity);

        let author_type_str = cli.author_type.map(String::from).or(base.author_type);
        let author_type = author_type_str
            .map(|type_val| parse_author_type(&type_val))
            .transpose()?;

        // Mode is no longer a CLI-overridable value (rem-wws): it comes
        // from the config file's `mode:` field and nothing else.
        let mode = base.mode;

        let key_str = cli.key.map(String::from).or(base.key);
        let key_path = key_str
            .map(|key_val| resolve_key_path(system, &key_val))
            .transpose()?;

        let assets_dir = cli.assets_dir.map(String::from).unwrap_or(base.assets_dir);

        let resolved = Self {
            assets_dir,
            author_type,
            identity,
            ignore: base.ignore,
            key_path,
            mode,
            registry: reg,
            unrestricted: false,
        };

        // Validate the resolved identity at construction time (rem-xc8x).
        // Op handlers can assume the config they receive has already
        // passed registry + key-presence enforcement for its active mode.
        resolved.validate_identity()?;

        Ok(resolved)
    }

    /// Return the signing key for `author` when the active mode requires
    /// signing, otherwise `None`.
    ///
    /// This is a trivial accessor: the key-presence fail-fast that used
    /// to live here has moved into [`Self::validate_identity`] (rem-xc8x),
    /// so every caller-visible [`ResolvedConfig`] has already been
    /// verified to carry a key path when strict mode requires one.
    /// Unregistered authors in strict mode return `None` here because
    /// they never reach op handlers — the resolver rejects them up
    /// front.
    #[must_use]
    pub fn resolve_signing_key(&self, author: &str) -> Option<&Path> {
        if !self.requires_signature(author) {
            return None;
        }
        self.key_path.as_deref()
    }

    /// Enforce mode-level invariants on the current identity: registry
    /// membership (registered / strict) and a resolvable signing key
    /// (strict).
    ///
    /// Invoked automatically by [`Self::resolve`] and
    /// [`Self::with_identity_overrides`] so every surface that produces a
    /// [`ResolvedConfig`] runs the same gate (rem-xc8x). Absent identity
    /// is not an error here — some read-only commands intentionally
    /// resolve without one; op handlers check for the identity they need
    /// separately.
    ///
    /// # Errors
    ///
    /// Returns an error when the active mode is registered/strict and
    /// the declared identity is not an active registry participant, or
    /// when strict mode is active but no signing key is resolvable for
    /// the registered identity.
    fn validate_identity(&self) -> Result<()> {
        let Some(identity) = self.identity.as_deref() else {
            return Ok(());
        };

        self.can_post(identity)?;

        // Strict + registered active identity but no key path: fail-fast
        // (rem-dyz). We use `requires_signature` so unregistered authors
        // in strict (already rejected by can_post above) do not reach
        // this branch.
        if self.mode == Mode::Strict && self.requires_signature(identity) && self.key_path.is_none()
        {
            bail!(
                "strict mode: no signing key resolved for {identity:?} \
                 (checked: --key flag, config `.remargin.yaml` key field). \
                 Fix your config or pass --key explicitly."
            );
        }

        Ok(())
    }

    /// Apply per-call identity overrides to an already-resolved config
    /// (rem-3a2).
    ///
    /// Returns `Ok(None)` when both overrides are absent (the caller
    /// continues to use the base config). Returns `Ok(Some(cfg))` with a
    /// cloned and overridden config otherwise. Canonicalizes the
    /// override-validation rules so CLI / MCP / future surfaces cannot
    /// drift on the same knob:
    ///
    /// - `identity_override` replaces `identity`.
    /// - `type_override` replaces `author_type`. When it is the only
    ///   override supplied and disagrees with the base `author_type`, the
    ///   call bails with the same message the config resolver emits for
    ///   the equivalent CLI flag combination — you cannot swap roles
    ///   without also declaring whose identity should be used.
    ///
    /// # Errors
    ///
    /// Returns an error when `type_override` is an unknown author type
    /// string, or when it disagrees with the base config without an
    /// accompanying `identity_override`.
    pub fn with_identity_overrides(
        &self,
        identity_override: Option<&str>,
        type_override: Option<&str>,
    ) -> Result<Option<Self>> {
        if identity_override.is_none() && type_override.is_none() {
            return Ok(None);
        }

        let mut overridden = self.clone();

        if let Some(id) = identity_override {
            overridden.identity = Some(String::from(id));
        }

        if let Some(type_str) = type_override {
            let new_type = parse_author_type(type_str)?;
            if identity_override.is_none() && self.author_type.as_ref() != Some(&new_type) {
                bail!(
                    "author_type override {type_str:?} does not match resolved type {:?}; \
                     provide an explicit identity",
                    self.author_type,
                );
            }
            overridden.author_type = Some(new_type);
        }

        // Re-run the registry + key-presence gate now that the identity
        // / author_type fields have been swapped (rem-xc8x). Without this
        // the MCP per-call override path would sidestep the resolver's
        // validation.
        overridden.validate_identity()?;

        Ok(Some(overridden))
    }
}

/// Resolved mode with provenance, produced by [`resolve_mode`].
///
/// Unlike the identity walk-up, this resolution ignores the `type:` field:
/// it returns whichever `.remargin.yaml` appears first on the walk (closest
/// to `start_dir`). `mode` is a directory-tree property, not an identity
/// property, so it must not be filtered by author type.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedMode {
    /// Effective mode for `start_dir`. Defaults to [`Mode::Open`] when no
    /// config with a `mode:` field is found on the walk.
    pub mode: Mode,
    /// Path to the `.remargin.yaml` that declared the mode, or `None` when
    /// the resolution fell back to the default.
    pub source: Option<PathBuf>,
}

fn default_assets_dir() -> String {
    String::from("assets")
}

const fn default_mode() -> Mode {
    Mode::Open
}

/// Walk up from `start_dir` looking for a file with the given name.
/// Returns the path to the first found file, or `None`.
///
/// # Errors
///
/// Returns an error if checking file existence fails.
fn find_file_upward(
    system: &dyn System,
    start_dir: &Path,
    filename: &str,
) -> Result<Option<PathBuf>> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(filename);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            return Ok(Some(candidate));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

/// Resolve the effective mode for a directory by walking up from `start_dir`
/// looking for the first `.remargin.yaml` — without any `type:` filtering.
///
/// Mode is a directory-tree property, independent of whose identity lives in
/// the config. This function is the clean way to ask "what mode applies
/// here?" without going through the identity machinery (which filters by
/// author type and can fall through to a different config).
///
/// Falls back silently to [`Mode::Open`] when no config is found, matching
/// the CLI's existing open-by-default posture.
///
/// # Errors
///
/// Returns an error if a `.remargin.yaml` exists on the walk but cannot be
/// read or parsed.
pub fn resolve_mode(system: &dyn System, start_dir: &Path) -> Result<ResolvedMode> {
    match load_config_filtered_with_path(system, start_dir, None)? {
        Some((path, cfg)) => Ok(ResolvedMode {
            mode: cfg.mode,
            source: Some(path),
        }),
        None => Ok(ResolvedMode {
            mode: Mode::Open,
            source: None,
        }),
    }
}

/// Load config by walking up from `start_dir`.
///
/// Returns `None` if no `.remargin.yaml` was found (defaults to open mode).
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_config(system: &dyn System, start_dir: &Path) -> Result<Option<Config>> {
    load_config_filtered(system, start_dir, None)
}

/// Load config by walking up from `start_dir`, optionally filtering by type.
///
/// If `type_filter` is `Some("human")`, only `.remargin.yaml` files with
/// `type: human` are considered. Files with a different type (or no type
/// field) are skipped and the walk continues upward.
///
/// If `type_filter` is `None`, the first `.remargin.yaml` found wins
/// (existing behavior).
///
/// Returns `None` if no matching config was found in the entire walk.
///
/// # Errors
///
/// Returns an error if a config file exists but cannot be read or parsed.
pub fn load_config_filtered(
    system: &dyn System,
    start_dir: &Path,
    type_filter: Option<&str>,
) -> Result<Option<Config>> {
    Ok(load_config_filtered_with_path(system, start_dir, type_filter)?.map(|(_, cfg)| cfg))
}

/// Like [`load_config_filtered`] but also returns the path to the matching
/// config file. Useful for tooling that needs to report *where* the config
/// was resolved from.
///
/// # Errors
///
/// Returns an error if a config file exists but cannot be read or parsed.
pub fn load_config_filtered_with_path(
    system: &dyn System,
    start_dir: &Path,
    type_filter: Option<&str>,
) -> Result<Option<(PathBuf, Config)>> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(CONFIG_FILENAME);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            let content = system
                .read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let config: Config = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", candidate.display()))?;

            match type_filter {
                None => return Ok(Some((candidate, config))),
                Some(filter) => {
                    if config.author_type.as_deref() == Some(filter) {
                        return Ok(Some((candidate, config)));
                    }
                }
            }
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

/// Load registry by walking up from `start_dir` (independent from config).
///
/// Returns `None` if no `.remargin-registry.yaml` was found.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_registry(system: &dyn System, start_dir: &Path) -> Result<Option<Registry>> {
    let path = find_file_upward(system, start_dir, REGISTRY_FILENAME)?;
    match path {
        Some(found) => {
            let content = system
                .read_to_string(&found)
                .with_context(|| format!("reading {}", found.display()))?;
            let reg: Registry = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", found.display()))?;
            Ok(Some(reg))
        }
        None => Ok(None),
    }
}

/// # Errors
///
/// Returns an error for unknown type strings.
/// Parse the canonical lowercase name of an [`AuthorType`] (`"human"` or
/// `"agent"`). Exposed publicly so per-call adapters (MCP tool handlers,
/// future IPC surfaces) can accept the same strings the config loader does
/// and reject unknown values identically.
///
/// # Errors
///
/// Returns an error when `type_str` is not one of the canonical lowercase
/// names.
pub fn parse_author_type(type_str: &str) -> Result<AuthorType> {
    match type_str {
        "human" => Ok(AuthorType::Human),
        "agent" => Ok(AuthorType::Agent),
        other => bail!("unknown author type: {other:?}"),
    }
}

/// Resolve the key path shorthand:
/// - Plain name (no `/` or `~` or `$`) maps to `~/.ssh/<name>`.
/// - Anything else is treated as a literal path and run through
///   [`expand_path`] so `~`, `$VAR`, and `${VAR}` resolve
///   identically to every other path surface (rem-3xo).
///
/// # Errors
///
/// Returns an error if the `HOME` environment variable is not set (when
/// resolving a plain name) or if [`expand_path`] rejects the literal
/// form.
pub fn resolve_key_path(system: &dyn System, key: &str) -> Result<PathBuf> {
    if key.contains('/') || key.starts_with('~') || key.contains('$') {
        Ok(expand_path(system, key)?)
    } else {
        let home = system
            .env_var("HOME")
            .context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join(".ssh").join(key))
    }
}
