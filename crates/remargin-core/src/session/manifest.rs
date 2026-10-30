//! Named-session → launchable-fleet resolution for `remargin session launch`.
//!
//! [`discover_sessions`](super::discovery::discover_sessions) walks *down*
//! from a cwd and enumerates the realms living beneath it. This module adds
//! the other half: when the config governing `cwd` carries a `sessions:`
//! manifest, a session name resolves to an explicit roster of agent folders
//! anywhere on disk. [`resolve_fleet`] loads each listed folder's own
//! identity and prompt, merges the entry's per-field overrides, and unions
//! the roster with downward discovery — failing loud on any bad entry so a
//! fleet never launches half-formed. The output is the same
//! [`DiscoveredSession`] vector the launch pipeline already consumes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;

use super::discovery::{DiscoveredSession, discover_sessions};
use crate::config::system_prompt::resolve_system_prompt;
use crate::config::{
    AgentEntry, CONFIG_FILENAME, Config, SessionConfig, SessionDef, SessionsManifest,
    load_config_filtered_with_path,
};
use crate::path::expand_path;
use crate::wait::Wait as _;

/// Resolve the fleet for one launch: the named session's entries (when a
/// `sessions:` manifest governs `cwd`) unioned with downward discovery.
///
/// `requested` is the CLI's optional `<name>`. Selection: an explicit name
/// must name a defined session; otherwise the declared `default`; otherwise
/// the single defined session; otherwise an error listing the defined names.
/// Without a manifest, `None` degrades to exactly
/// [`discover_sessions`](super::discovery::discover_sessions) and `Some(_)`
/// is an error.
///
/// Each entry's `path` resolves against the manifest file's own folder (with
/// `~`/env expansion), never `cwd`, and the resolved folder's own
/// `.remargin.yaml` must declare an `identity`. The union deduplicates by
/// `(identity, folder)` with the manifest entry winning, and entries precede
/// discovered agents.
///
/// This is pure resolution — nothing spawns, no backend or multiplexer is
/// touched.
///
/// # Errors
///
/// Returns an error when downward discovery fails, when a name was requested
/// but no manifest governs `cwd`, when selection is ambiguous or names an
/// undefined session, or when any entry is bad (missing folder, unreadable
/// or absent `.remargin.yaml`, or a config without `identity`). A bad entry
/// fails the whole resolution — no partial fleet is returned.
pub fn resolve_fleet(
    system: &dyn System,
    cwd: &Path,
    requested: Option<&str>,
) -> Result<Vec<DiscoveredSession>> {
    let discovered = discover_sessions(system, cwd)?;

    let Some((manifest_dir, manifest)) = governing_manifest(system, cwd)? else {
        match requested {
            None => return Ok(discovered),
            Some(name) => bail!(
                "no `sessions:` manifest declares {name:?}; none governs {}",
                cwd.display()
            ),
        }
    };

    let (name, def) = select_session(&manifest, requested)?;

    let mut fleet: Vec<DiscoveredSession> = Vec::new();
    for entry in &def.agents {
        fleet.push(resolve_entry(system, &manifest_dir, name, entry)?);
    }

    // Union: a discovered agent joins unless an entry already claims the same
    // `(identity, folder)` — the entry is the more specific declaration.
    let claimed: HashSet<(String, PathBuf)> = fleet
        .iter()
        .map(|session| (session.identity.clone(), session.folder.clone()))
        .collect();
    fleet.extend(
        discovered.into_iter().filter(|session| {
            !claimed.contains(&(session.identity.clone(), session.folder.clone()))
        }),
    );

    Ok(fleet)
}

/// The `sessions:` manifest of the nearest `.remargin.yaml` at or above
/// `cwd`, together with that file's directory — the anchor relative entry
/// paths resolve against. `None` when no config is found or the nearest one
/// declares no `sessions:` block (pure downward discovery then applies).
fn governing_manifest(
    system: &dyn System,
    cwd: &Path,
) -> Result<Option<(PathBuf, SessionsManifest)>> {
    let Some((path, config)) = load_config_filtered_with_path(system, cwd, None)? else {
        return Ok(None);
    };
    let Some(manifest) = config.sessions else {
        return Ok(None);
    };
    let dir = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Ok(Some((dir, manifest)))
}

/// Pick the session a launch uses, returning its name and definition.
///
/// An explicit `requested` name must name a defined session. Otherwise the
/// declared `default` wins; failing that, the single defined session; failing
/// that (two or more, no default), an error listing the names.
fn select_session<'manifest>(
    manifest: &'manifest SessionsManifest,
    requested: Option<&str>,
) -> Result<(&'manifest str, &'manifest SessionDef)> {
    let name = match requested {
        Some(name) => name.to_owned(),
        None => match (&manifest.default, manifest.sessions.len()) {
            (Some(default), _) => default.clone(),
            (None, 1) => manifest.sessions.keys().next().cloned().unwrap(),
            (None, _) => bail!(
                "no default session declared; pick one of: {}",
                defined_names(manifest)
            ),
        },
    };
    manifest
        .sessions
        .get_key_value(&name)
        .map(|(key, def)| (key.as_str(), def))
        .with_context(|| {
            format!(
                "session {name:?} is not defined; defined: {}",
                defined_names(manifest)
            )
        })
}

/// Resolve one roster entry into a launchable session: locate its folder,
/// require its own `.remargin.yaml` to declare an `identity`, resolve the
/// folder's system prompt, and merge the entry's overrides over the folder's
/// own `session:` block.
fn resolve_entry(
    system: &dyn System,
    manifest_dir: &Path,
    session_name: &str,
    entry: &AgentEntry,
) -> Result<DiscoveredSession> {
    let folder = resolve_entry_path(system, manifest_dir, &entry.path)?;
    let loaded = read_own_config(system, &folder).with_context(|| {
        format!(
            "session {session_name:?}: reading manifest entry {:?}",
            entry.path
        )
    })?;
    let Some(config) = loaded else {
        bail!(
            "session {session_name:?}: manifest entry {:?} resolves to {}, which has no {CONFIG_FILENAME}",
            entry.path,
            folder.display()
        );
    };
    let Some(identity) = config.identity else {
        bail!(
            "session {session_name:?}: manifest entry {:?} ({}) declares no `identity`",
            entry.path,
            folder.display()
        );
    };

    let session = merge_session(config.session, entry);
    let system_prompt = resolve_system_prompt(system, &folder)?;
    Ok(DiscoveredSession {
        folder: folder.clone(),
        identity,
        scope_root: folder,
        session: Some(session),
        system_prompt,
    })
}

/// Resolve an entry `path` against the manifest folder. Absolute paths (and
/// `~`/env expansions that produce them) stand alone; relative ones join the
/// manifest folder. Interior `.` components are normalized away so the result
/// compares and deduplicates cleanly against discovery's paths.
fn resolve_entry_path(system: &dyn System, manifest_dir: &Path, raw: &str) -> Result<PathBuf> {
    let expanded =
        expand_path(system, raw).with_context(|| format!("expanding entry path {raw:?}"))?;
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        manifest_dir.join(expanded)
    };
    Ok(joined.components().collect())
}

/// Merge an entry's overrides over the target folder's own `session:` block.
/// Each field replaces as a whole value; the entry wins when it declares one.
fn merge_session(target: Option<SessionConfig>, entry: &AgentEntry) -> SessionConfig {
    let base = target.unwrap_or_default();
    SessionConfig {
        budget: entry.budget.clone().or(base.budget),
        claude: entry.claude.clone().or(base.claude),
        goal: entry.goal.clone().or(base.goal),
        loop_interval: entry.loop_interval.clone().or(base.loop_interval),
    }
}

/// Parse the `.remargin.yaml` directly in `folder` (never walking up).
/// `None` when the folder has no config file of its own — the caller turns
/// that into the fail-loud "missing config" error with full entry context.
fn read_own_config(system: &dyn System, folder: &Path) -> Result<Option<Config>> {
    let candidate = folder.join(CONFIG_FILENAME);
    if !system
        .exists(&candidate)
        .wait()
        .with_context(|| format!("checking existence of {}", candidate.display()))?
    {
        return Ok(None);
    }
    let content = system
        .read_to_string(&candidate)
        .wait()
        .with_context(|| format!("reading {}", candidate.display()))?;
    let config: Config = serde_yaml::from_str(&content)
        .with_context(|| format!("parsing {}", candidate.display()))?;
    Ok(Some(config))
}

/// Comma-joined list of the manifest's defined session names, in the map's
/// sorted order — for the "which sessions exist?" error messages.
fn defined_names(manifest: &SessionsManifest) -> String {
    manifest
        .sessions
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}
