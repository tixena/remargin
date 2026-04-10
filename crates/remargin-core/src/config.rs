//! Configuration loader: `.remargin.yaml` walk-up resolution.

pub mod registry;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Deserialize;

use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::parser::AuthorType;

const CONFIG_FILENAME: &str = ".remargin.yaml";
const REGISTRY_FILENAME: &str = ".remargin-registry.yaml";

/// CLI overrides that take precedence over config file values.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct CliOverrides<'cli> {
    pub assets_dir: Option<&'cli str>,
    pub author_type: Option<&'cli str>,
    pub identity: Option<&'cli str>,
    pub key: Option<&'cli str>,
    pub mode: Option<&'cli str>,
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
fn parse_author_type(type_str: &str) -> Result<AuthorType> {
    match type_str {
        "human" => Ok(AuthorType::Human),
        "agent" => Ok(AuthorType::Agent),
        other => bail!("unknown author type: {other:?}"),
    }
}

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
