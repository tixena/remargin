//! `remargin goose session-guard` core — goose `SessionStart` hook.
//!
//! Re-verifies that path enforcement will be live for the goose session:
//!
//! 1. the `PreToolUse` guard plugin is wired in at least one scope — its
//!    manifest parses, declares the entry, and the absolute binary that
//!    entry names is still on disk. goose fails open when a hook cannot be
//!    spawned, so a plugin pointing at a binary that moved is an unguarded
//!    session that looks guarded;
//! 2. the realm's `.remargin.yaml` above the session's working directory
//!    parses — a malformed config would otherwise surface at tool-call
//!    time instead of session start.
//!
//! ## `SessionStart` cannot block
//!
//! Only `PreToolUse` and `Stop` carry a blocking decision in goose; a
//! block from any other event is ignored. This guard is therefore a
//! DIAGNOSTIC and nothing more: it makes a broken guard loud, it cannot
//! stop the session. The diagnostic goes to stdout, the channel goose
//! surfaces, rather than riding an exit code the platform treats as a
//! fail-open pass.
//!
//! Pure (no stdin / stdout / `process::exit`): the CLI handler owns I/O,
//! so unit tests run without spawning the binary.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use os_shim::System;
use serde::Deserialize;

use crate::config;
use crate::permissions::goose_install::{self, TestOutcome};

/// Outcome of the guard. The caller emits the diagnostic and always exits
/// 0 — `SessionStart` has no blocking control, and a non-zero exit is a
/// hook failure goose swallows.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardOutcome {
    /// Enforcement may be silently disabled; the diagnostic goes to stdout.
    Fail(String),
    /// Enforcement will be live; emit nothing.
    Ok,
}

/// goose's `SessionStart` envelope. Only the session's working directory
/// matters here — it roots both the realm config walk and the
/// project-scope plugin lookup.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct SessionStartEvent {
    pub working_dir: Option<PathBuf>,
}

/// Re-verify that enforcement will be live for the session `stdin_bytes`
/// describes, falling back to `cwd` when the envelope names no working
/// directory.
#[must_use]
pub fn goose_session_guard(system: &dyn System, stdin_bytes: &[u8], cwd: &Path) -> GuardOutcome {
    let root = session_root(stdin_bytes, cwd);
    let mut failures: Vec<String> = Vec::new();

    if let Some(failure) = plugin_failure(system, &root) {
        failures.push(failure);
    }

    if let Err(err) = config::load_config(system, &root) {
        failures.push(format!(
            "the realm's .remargin.yaml above {} failed to parse ({err:#}) -- enforcement would \
             fail only at tool-call time",
            root.display()
        ));
    }

    if failures.is_empty() {
        GuardOutcome::Ok
    } else {
        GuardOutcome::Fail(diagnostic(&failures))
    }
}

fn diagnostic(failures: &[String]) -> String {
    let reasons = failures.join("; ");
    format!(
        "REMARGIN GOOSE SESSION GUARD FAILURE -- remargin path enforcement may be silently \
         disabled for this session: {reasons}. goose lets a tool call through when its hook \
         cannot run, so the breakage is silent from inside the session. Do NOT assume \
         remargin-managed files are protected: treat every `.md` under a `.remargin.yaml` realm \
         as remargin-managed regardless, and run `remargin doctor` to diagnose before touching \
         managed paths."
    )
}

/// The reason the `PreToolUse` guard is not live, if it is not.
///
/// The guard counts as live when either plugin scope reports it wired, so
/// a project-scope-only install is clean. A broken plugin is reported
/// ahead of a plain absence: it names a different repair.
fn plugin_failure(system: &dyn System, root: &Path) -> Option<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(home) = system.env_var("HOME") {
        dirs.push(goose_install::plugin_dir(Path::new(&home)));
    }
    dirs.push(goose_install::plugin_dir(root));

    let outcomes: Vec<TestOutcome> = dirs
        .iter()
        // A probe that cannot answer is not evidence of a live guard, so an I/O failure reads as a
        // corrupt plugin and carries its own cause.
        .map(|dir| {
            goose_install::test(system, dir).unwrap_or_else(|err| {
                TestOutcome::Broken(format!(
                    "{} could not be inspected ({err:#})",
                    dir.display()
                ))
            })
        })
        .collect();

    if outcomes
        .iter()
        .any(|outcome| matches!(*outcome, TestOutcome::Installed))
    {
        return None;
    }
    for outcome in &outcomes {
        if let TestOutcome::Broken(reason) = outcome {
            return Some(format!(
                "the remargin guard plugin does not describe a live {} hook ({reason})",
                goose_install::HOOK_EVENT
            ));
        }
    }
    Some(format!(
        "the remargin guard plugin is absent from {}, so no {} guard runs for this session",
        scopes(&dirs),
        goose_install::HOOK_EVENT,
    ))
}

fn scopes(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(" and ")
}

/// The directory the session runs in: goose's `working_dir` when the
/// envelope carries one, else the process cwd.
///
/// An envelope the guard cannot read is NOT reported. `SessionStart`
/// cannot block, so this guard's only power is being believed when it
/// speaks; a diagnostic fired at a healthy session over the shape of its
/// own input is the noise that teaches users to ignore it.
fn session_root(stdin_bytes: &[u8], cwd: &Path) -> PathBuf {
    serde_json::from_slice::<SessionStartEvent>(stdin_bytes)
        .ok()
        .and_then(|event| event.working_dir)
        .unwrap_or_else(|| cwd.to_path_buf())
}
