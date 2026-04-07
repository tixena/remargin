//! Configuration loader: `.remargin.yaml` walk-up resolution.
//!
//! This module handles loading and resolving Remargin configuration from
//! `.remargin.yaml` files found by walking up the directory tree, as well as
//! merging with CLI overrides and the participant registry.

pub mod registry;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Deserialize;

use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::parser::AuthorType;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// File name for the local Remargin config (gitignored).
const CONFIG_FILENAME: &str = ".remargin.yaml";

/// File name for the participant registry (can be committed).
const REGISTRY_FILENAME: &str = ".remargin-registry.yaml";

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// CLI overrides that take precedence over config file values.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct CliOverrides<'cli> {
    /// Override for assets directory.
    pub assets_dir: Option<&'cli str>,
    /// Override for author type.
    pub author_type: Option<&'cli str>,
    /// Override for identity.
    pub identity: Option<&'cli str>,
    /// Override for signing key path.
    pub key: Option<&'cli str>,
    /// Override for mode.
    pub mode: Option<&'cli str>,
}

/// Parsed contents of a `.remargin.yaml` file.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Config {
    /// Assets directory (relative to document location).
    #[serde(default = "default_assets_dir")]
    pub assets_dir: String,
    /// Default author type.
    #[serde(rename = "type")]
    pub author_type: Option<String>,
    /// Default identity for this machine.
    pub identity: Option<String>,
    /// Ignore patterns for ls/query (glob syntax).
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Path to signing key.
    pub key: Option<String>,
    /// Registry mode: open, registered, or strict.
    #[serde(default = "default_mode")]
    pub mode: Mode,
}

/// Enforcement mode for the participant registry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Mode {
    /// Anyone can post comments.
    Open,
    /// Only registered participants can post.
    Registered,
    /// Registered participants only, and signatures are required.
    Strict,
}

/// The final resolved configuration after merging the config file, registry,
/// and CLI overrides.
#[derive(Debug)]
#[non_exhaustive]
pub struct ResolvedConfig {
    /// Assets directory.
    pub assets_dir: String,
    /// Resolved author type.
    pub author_type: Option<AuthorType>,
    /// Resolved identity.
    pub identity: Option<String>,
    /// Ignore patterns.
    pub ignore: Vec<String>,
    /// Resolved path to signing key.
    pub key_path: Option<PathBuf>,
    /// Enforcement mode.
    pub mode: Mode,
    /// Loaded registry (if found).
    pub registry: Option<Registry>,
    /// Whether to bypass path sandbox checks.
    /// Only settable via CLI when compiled with `--features unrestricted`.
    pub unrestricted: bool,
}

// ---------------------------------------------------------------------------
// Impl blocks
// ---------------------------------------------------------------------------

impl ResolvedConfig {
    /// Check if a participant is allowed to post (mode + registry enforcement).
    ///
    /// # Errors
    ///
    /// Returns an error if the participant is not allowed to post in the current
    /// mode (e.g. unregistered in registered/strict mode, or revoked).
    pub fn can_post(&self, author: &str) -> Result<()> {
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
    /// - An unknown author type or mode string is provided
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

        let identity = cli.identity.map(String::from).or(base.identity);

        let author_type_str = cli.author_type.map(String::from).or(base.author_type);
        let author_type = author_type_str
            .map(|type_val| parse_author_type(&type_val))
            .transpose()?;

        let mode = if let Some(mode_str) = cli.mode {
            parse_mode(mode_str)?
        } else {
            base.mode
        };

        let key_str = cli.key.map(String::from).or(base.key);
        let key_path = key_str
            .map(|key_val| resolve_key_path(system, &key_val))
            .transpose()?;

        let assets_dir = cli.assets_dir.map(String::from).unwrap_or(base.assets_dir);

        Ok(Self {
            assets_dir,
            author_type,
            identity,
            ignore: base.ignore,
            key_path,
            mode,
            registry: reg,
            unrestricted: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default assets directory name.
fn default_assets_dir() -> String {
    String::from("assets")
}

/// Default enforcement mode.
const fn default_mode() -> Mode {
    Mode::Open
}

// ---------------------------------------------------------------------------
// Walk-up file resolution
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

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
                None => return Ok(Some(config)),
                Some(filter) => {
                    if config.author_type.as_deref() == Some(filter) {
                        return Ok(Some(config));
                    }
                    // Type does not match; continue walking up.
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

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse a string into an `AuthorType`.
///
/// # Errors
///
/// Returns an error for unknown type strings.
fn parse_author_type(type_str: &str) -> Result<AuthorType> {
    match type_str {
        "human" => Ok(AuthorType::Human),
        "agent" => Ok(AuthorType::Agent),
        other => bail!("unknown author type: {other:?}"),
    }
}

/// Parse a string into a `Mode`.
///
/// # Errors
///
/// Returns an error for unknown mode strings.
fn parse_mode(mode_str: &str) -> Result<Mode> {
    match mode_str {
        "open" => Ok(Mode::Open),
        "registered" => Ok(Mode::Registered),
        "strict" => Ok(Mode::Strict),
        other => bail!("unknown mode: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Key path resolution
// ---------------------------------------------------------------------------

/// Resolve the key path shorthand:
/// - Plain name (no `/` or `~`) maps to `~/.ssh/<name>`
/// - Path with `/` or `~` is treated as a literal path
///
/// Uses the `HOME` environment variable from the system abstraction.
///
/// # Errors
///
/// Returns an error if `HOME` is not set (when resolving a plain name).
pub fn resolve_key_path(system: &dyn System, key: &str) -> Result<PathBuf> {
    if key.contains('/') {
        if let Some(rest) = key.strip_prefix("~/") {
            let home = system
                .env_var("HOME")
                .context("HOME environment variable not set")?;
            Ok(PathBuf::from(home).join(rest))
        } else {
            Ok(PathBuf::from(key))
        }
    } else {
        let home = system
            .env_var("HOME")
            .context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join(".ssh").join(key))
    }
}
