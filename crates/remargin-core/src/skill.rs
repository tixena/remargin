//! Skill install, uninstall, and test operations.
//!
//! Manages agent skill files that teach AI agents how to invoke remargin
//! commands. The skill is embedded in the binary at compile time and extracted
//! to the target agent's skill directory on install.

#[cfg(test)]
mod tests;

use core::str;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use include_dir::{Dir, include_dir};
use os_shim::System;

/// Skill files embedded at compile time from the `skill/` directory.
static SKILL_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skill");

/// The AI agent to install the skill for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Agent {
    /// Claude Code — installs to `.claude/skills/remargin/`.
    Claude,
    /// Gemini CLI — installs to `.gemini/skills/remargin/`.
    Gemini,
}

impl Agent {
    /// The canonical lowercase name used in config filenames and YAML content.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }

    /// The skill subdirectory relative to the project root or home directory.
    #[must_use]
    pub const fn skill_subdir(self) -> &'static str {
        match self {
            Self::Claude => ".claude/skills/remargin",
            Self::Gemini => ".gemini/skills/remargin",
        }
    }
}

/// Installation status of the skill.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillStatus {
    /// The skill directory does not exist.
    NotInstalled,
    /// The skill exists but content differs from the embedded version.
    Outdated,
    /// The skill matches the embedded version exactly.
    UpToDate,
}

/// Install the skill by extracting embedded files.
///
/// If `global` is true, installs to `~/<agent-dir>/skills/remargin/`.
/// Otherwise installs to `./<agent-dir>/skills/remargin/`.
///
/// # Errors
///
/// Returns an error if:
/// - The `HOME` env var is not set (for global install)
/// - Directory creation or file writing fails
pub fn install(system: &dyn System, agent: Agent, global: bool) -> Result<PathBuf> {
    let root = resolve_root(system, global)?;
    let skill_path = root.join(agent.skill_subdir());

    system
        .create_dir_all(&skill_path)
        .with_context(|| format!("creating skill directory {}", skill_path.display()))?;

    for file in SKILL_DIR.files() {
        let dest = skill_path.join(file.path());
        system
            .write(&dest, file.contents())
            .with_context(|| format!("writing {}", dest.display()))?;
    }

    let name = agent.name();
    let config_path = root.join(format!(".remargin.{name}.yaml"));
    if !system.exists(&config_path).unwrap_or(false) {
        let content = format!("identity: {name}\ntype: agent\n");
        system
            .write(&config_path, content.as_bytes())
            .with_context(|| format!("writing {}", config_path.display()))?;
    }

    Ok(skill_path)
}

/// Check installation status via byte-for-byte comparison.
///
/// # Errors
///
/// Returns an error if:
/// - The `HOME` env var is not set (for global check)
pub fn test_status(system: &dyn System, agent: Agent, global: bool) -> Result<SkillStatus> {
    let skill_path = resolve_skill_path(system, agent, global)?;

    if !system.exists(&skill_path).unwrap_or(false) {
        return Ok(SkillStatus::NotInstalled);
    }

    for file in SKILL_DIR.files() {
        let dest = skill_path.join(file.path());
        let Ok(existing) = system.read_to_string(&dest) else {
            return Ok(SkillStatus::Outdated);
        };

        let Ok(embedded) = str::from_utf8(file.contents()) else {
            return Ok(SkillStatus::Outdated);
        };

        if existing != embedded {
            return Ok(SkillStatus::Outdated);
        }
    }

    Ok(SkillStatus::UpToDate)
}

/// Uninstall the skill by removing the directory.
///
/// # Errors
///
/// Returns an error if:
/// - The `HOME` env var is not set (for global uninstall)
/// - The directory cannot be removed
pub fn uninstall(system: &dyn System, agent: Agent, global: bool) -> Result<()> {
    let skill_path = resolve_skill_path(system, agent, global)?;

    if !system.exists(&skill_path).unwrap_or(false) {
        bail!("skill is not installed at {}", skill_path.display());
    }

    system
        .remove_dir_all(&skill_path)
        .with_context(|| format!("removing {}", skill_path.display()))?;

    Ok(())
}

/// Resolve the install root: home directory for global, cwd for project.
fn resolve_root(system: &dyn System, global: bool) -> Result<PathBuf> {
    if global {
        let home = system
            .env_var("HOME")
            .map_err(|_err| anyhow::anyhow!("HOME environment variable not set"))?;
        Ok(PathBuf::from(home))
    } else {
        system.current_dir().context("getting current directory")
    }
}

/// Resolve the skill installation path.
fn resolve_skill_path(system: &dyn System, agent: Agent, global: bool) -> Result<PathBuf> {
    Ok(resolve_root(system, global)?.join(agent.skill_subdir()))
}
