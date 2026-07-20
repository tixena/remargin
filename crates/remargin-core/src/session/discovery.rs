//! Downward identity discovery for `remargin session launch`.
//!
//! [`discover_sessions`] walks *down* from a directory and returns one
//! [`DiscoveredSession`] per `.remargin.yaml` that declares its own
//! `identity`. This is the fan-out complement to the upward, single-answer
//! identity resolver in [`crate::config::identity`]: launch needs the whole
//! set of realms living beneath a cwd, each with its resolved system prompt
//! and its own `session:` block.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;

use crate::config::system_prompt::{ResolvedSystemPrompt, resolve_system_prompt};
use crate::config::{CONFIG_FILENAME, Config, SessionConfig};

/// One launchable session found by [`discover_sessions`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DiscoveredSession {
    /// The realm root this session launches in. `cwd` for the root session
    /// even when its identity is inherited from an ancestor.
    pub folder: PathBuf,
    /// The identity that governs this session. For a downward-discovered
    /// realm it is the identity that realm's own `.remargin.yaml` declares;
    /// for the root session it is the identity governing `cwd` (declared
    /// there or inherited from an ancestor).
    pub identity: String,
    /// Root of the subtree this session owns — the folder itself. The
    /// owned subtree stops at each nested realm that declares its own
    /// identity. Informational only: coordination is the workflow owner's
    /// concern, and writes are atomic.
    pub scope_root: PathBuf,
    /// The declaring folder's `session:` block, if any. Validated later
    /// (task 84), not here.
    pub session: Option<SessionConfig>,
    /// Resolved nearest `system_prompt:` for [`Self::folder`].
    pub system_prompt: ResolvedSystemPrompt,
}

/// Walk *down* from `cwd` and enumerate every session to launch.
///
/// The identity governing `cwd` (declared at `cwd`, else inherited from an
/// ancestor) becomes the root session, with `folder = cwd`. Each descendant
/// directory whose `.remargin.yaml` declares its *own* `identity` becomes a
/// further session and a nested-realm boundary. A directory whose config
/// lacks an identity is not emitted (it inherits — nothing new to launch),
/// though the walk still descends through it. Results are deduplicated by
/// `(identity, folder)`; same-name identities in sibling folders stay
/// distinct because the folder disambiguates them.
///
/// When no identity governs `cwd` and none is declared below it, the result
/// is empty.
///
/// # Errors
///
/// Returns an error when a directory cannot be listed, or when a candidate
/// `.remargin.yaml` exists but cannot be read or parsed.
pub fn discover_sessions(system: &dyn System, cwd: &Path) -> Result<Vec<DiscoveredSession>> {
    let mut sessions = Vec::new();
    let mut seen: HashSet<(String, PathBuf)> = HashSet::new();

    if let Some((identity, session)) = governing_identity(system, cwd)? {
        push_session(
            system,
            &mut sessions,
            &mut seen,
            identity,
            cwd.to_path_buf(),
            session,
        )?;
    }

    visit(system, cwd, &mut sessions, &mut seen)?;

    Ok(sessions)
}

/// Nearest identity governing `start_dir`, walking up until one is found.
/// Returns that identity together with the declaring config's `session:`
/// block, or `None` when no ancestor declares an identity.
fn governing_identity(
    system: &dyn System,
    start_dir: &Path,
) -> Result<Option<(String, Option<SessionConfig>)>> {
    let mut current = start_dir.to_path_buf();
    loop {
        if let Some(config) = read_dir_config(system, &current)?
            && let Some(identity) = config.identity
        {
            return Ok(Some((identity, config.session)));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

/// Depth-first descent through `dir`'s subdirectories, emitting a session
/// for every one whose own `.remargin.yaml` declares an identity. Children
/// are visited in sorted order so the result is deterministic.
fn visit(
    system: &dyn System,
    dir: &Path,
    sessions: &mut Vec<DiscoveredSession>,
    seen: &mut HashSet<(String, PathBuf)>,
) -> Result<()> {
    let mut children = Vec::new();
    for entry in system
        .read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?
    {
        // Dot-directories (.git, .obsidian, …) never host a remargin realm;
        // skipping them keeps the walk off large unrelated subtrees.
        if entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        if system
            .is_dir(&entry)
            .with_context(|| format!("checking whether {} is a directory", entry.display()))?
        {
            children.push(entry);
        }
    }
    children.sort();

    for child in children {
        if let Some(config) = read_dir_config(system, &child)?
            && let Some(identity) = config.identity
        {
            push_session(
                system,
                sessions,
                seen,
                identity,
                child.clone(),
                config.session,
            )?;
        }
        visit(system, &child, sessions, seen)?;
    }

    Ok(())
}

/// Emit a session for `folder`, resolving its system prompt, unless
/// `(identity, folder)` was already recorded.
fn push_session(
    system: &dyn System,
    sessions: &mut Vec<DiscoveredSession>,
    seen: &mut HashSet<(String, PathBuf)>,
    identity: String,
    folder: PathBuf,
    session: Option<SessionConfig>,
) -> Result<()> {
    if !seen.insert((identity.clone(), folder.clone())) {
        return Ok(());
    }
    let system_prompt = resolve_system_prompt(system, &folder)?;
    sessions.push(DiscoveredSession {
        folder: folder.clone(),
        identity,
        scope_root: folder,
        session,
        system_prompt,
    });
    Ok(())
}

/// Parse the `.remargin.yaml` directly in `dir`, if present. `None` when
/// the directory has no config file of its own (the caller keeps walking).
fn read_dir_config(system: &dyn System, dir: &Path) -> Result<Option<Config>> {
    let candidate = dir.join(CONFIG_FILENAME);
    if !system
        .exists(&candidate)
        .with_context(|| format!("checking existence of {}", candidate.display()))?
    {
        return Ok(None);
    }
    let content = system
        .read_to_string(&candidate)
        .with_context(|| format!("reading {}", candidate.display()))?;
    let config: Config = serde_yaml::from_str(&content)
        .with_context(|| format!("parsing {}", candidate.display()))?;
    Ok(Some(config))
}
