//! `remargin claude session-guard` core — Claude Code `SessionStart` hook.
//!
//! Re-verifies that path enforcement will be live for the session:
//!
//! 1. a `PreToolUse` hook entry is registered at all, and the command it
//!    names can be spawned — a command Claude Code cannot find exits 127,
//!    which it treats as a *non-blocking* failure, and the gated tool call
//!    then proceeds unprotected, silently. Installs embed an absolute
//!    binary path, so the probe is that the binary is still on disk; for an
//!    entry left by an older install, which names the binary by bare name,
//!    the probe is that `remargin` still resolves on `PATH`;
//! 2. the realm's `.remargin.yaml` above cwd parses — a malformed config
//!    would surface at tool-call time instead of session start.
//!
//! ## `SessionStart` cannot block
//!
//! Per the Claude Code hooks contract, a `SessionStart` hook has no
//! blocking or decision control: exit 2 only renders stderr as a
//! non-blocking notice that Claude never sees, and `continue: false` is
//! not honored for this event. JSON is processed only on exit 0. The
//! strongest available signal is therefore exit-0 JSON on stdout:
//! `hookSpecificOutput.additionalContext` is injected into Claude's
//! context (the model reads it) and `systemMessage` is shown to the
//! user. This module emits both on failure — it surfaces a loud
//! diagnostic; it does not, and cannot, halt the session.
//!
//! Pure (no stdin / stdout / `process::exit`): the CLI handler owns I/O,
//! so unit tests run without spawning the binary.

#[cfg(test)]
mod tests;

use std::env::split_paths;
use std::path::{Path, PathBuf};

use os_shim::System;
use serde::Serialize;

use crate::config;
use crate::permissions::pretool_install::{self, TestOutcome};

/// The name a `PATH`-relative `PreToolUse` entry resolves; if it does not, enforcement is off.
const BINARY_NAME: &str = "remargin";

/// Relative to its scope root; the file both `install` and `install --local` write.
const SETTINGS_FILE: &str = ".claude/settings.json";

/// Reported for an entry that names the binary by bare name, the only kind `PATH` decides.
const PATH_FAILURE: &str = "the `remargin` binary does not resolve on PATH -- a PreToolUse hook \
                            (`remargin claude pretool`) that cannot find `remargin` exits 127, \
                            which Claude Code treats as non-blocking, so every gated tool call \
                            proceeds unprotected";

/// `SessionStart` diagnostic JSON shape Claude Code reads on stdout (exit 0).
#[derive(Debug, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct GuardDiagnostic {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: GuardDiagnosticInner,
    /// Shown to the user as a session warning.
    #[serde(rename = "systemMessage")]
    pub system_message: String,
}

/// Inner `hookSpecificOutput` body — pinned to the `SessionStart` schema.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct GuardDiagnosticInner {
    /// Injected into Claude's context at session start.
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
    #[serde(rename = "hookEventName")]
    pub hook_event_name: &'static str,
}

/// Outcome of the guard. The caller emits stdout and always exits 0 (JSON
/// is honored only on exit 0; `SessionStart` cannot block regardless).
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardOutcome {
    /// Enforcement may be silently disabled; the diagnostic JSON goes to stdout.
    Fail(GuardDiagnostic),
    /// Enforcement will be live; emit nothing.
    Ok,
}

/// Re-verify that enforcement will be live for a session rooted at `cwd`.
#[must_use]
pub fn session_guard(system: &dyn System, cwd: &Path) -> GuardOutcome {
    let mut failures: Vec<String> = Vec::new();

    if let Some(failure) = hook_failure(system, cwd) {
        failures.push(failure);
    }

    if let Err(err) = config::load_config(system, cwd) {
        failures.push(format!(
            "the realm's .remargin.yaml above {} failed to parse ({err:#}) -- enforcement would \
             fail only at tool-call time",
            cwd.display()
        ));
    }

    if failures.is_empty() {
        GuardOutcome::Ok
    } else {
        GuardOutcome::Fail(build_diagnostic(&failures))
    }
}

/// The reason the `PreToolUse` hook will not run, if it will not.
///
/// The hook counts as live when either settings scope declares an entry
/// whose absolute binary is on disk — `install --local` is a supported
/// wiring. An entry that names the binary by bare name is checked the only
/// way a bare name can be: against `PATH`. A pair of scopes declaring no
/// entry at all is the loud case: nothing is registered to spawn, so a
/// `remargin` sitting on `PATH` gates nothing.
fn hook_failure(system: &dyn System, cwd: &Path) -> Option<String> {
    let files = settings_files(system, cwd);
    let outcomes: Vec<TestOutcome> = files
        .iter()
        // A probe that cannot answer is not evidence of a live hook, so an unreadable settings file
        // reads as broken and carries its own cause.
        .map(|file| {
            pretool_install::test(system, file).unwrap_or_else(|err| {
                TestOutcome::Broken(format!(
                    "{} could not be inspected ({err:#})",
                    file.display()
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
    if outcomes
        .iter()
        .any(|outcome| matches!(*outcome, TestOutcome::PathRelative(_)))
    {
        return path_failure(system);
    }
    for outcome in &outcomes {
        if let TestOutcome::Broken(reason) = outcome {
            return Some(format!(
                "the PreToolUse hook cannot run: {reason} -- Claude Code treats a hook command it \
                 cannot spawn as non-blocking, so every gated tool call proceeds unprotected"
            ));
        }
    }
    Some(no_entry_failure(&files))
}

/// [`PATH_FAILURE`] when the bare binary does not resolve, else clean.
fn path_failure(system: &dyn System) -> Option<String> {
    (!remargin_on_path(system)).then(|| String::from(PATH_FAILURE))
}

/// The failure every scope reporting no entry reports: enforcement was
/// never wired, so nothing gates a tool call for this session.
fn no_entry_failure(files: &[PathBuf]) -> String {
    let scopes = files
        .iter()
        .map(|file| file.display().to_string())
        .collect::<Vec<_>>()
        .join(" or ");
    format!(
        "the PreToolUse hook (`remargin claude pretool`) is not registered in {scopes}, so no hook \
         gates a tool call for this session -- run `remargin claude pretool install` to register \
         it"
    )
}

/// Both settings files a hook entry can live in: user scope under `$HOME`,
/// project scope under the session's working directory. A missing `HOME`
/// drops only the user-scope file — the project one still answers.
fn settings_files(system: &dyn System, cwd: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(home) = system.env_var("HOME") {
        files.push(Path::new(&home).join(SETTINGS_FILE));
    }
    files.push(cwd.join(SETTINGS_FILE));
    files
}

/// Resolve the bare [`BINARY_NAME`] against `PATH` through the [`System`]
/// shim (never raw `std::env`), so the check reflects the same lookup a
/// child `remargin claude pretool` invocation would perform when the entry
/// names no absolute path. [`split_paths`]
/// operates on the value we read — it does not touch process env — so it
/// stays hermetic under `MemorySystem`.
fn remargin_on_path(system: &dyn System) -> bool {
    let Ok(path_var) = system.env_var("PATH") else {
        return false;
    };
    split_paths(&path_var).any(|dir| system.is_file(&dir.join(BINARY_NAME)).unwrap_or(false))
}

fn build_diagnostic(failures: &[String]) -> GuardDiagnostic {
    let reasons = failures.join("; ");
    GuardDiagnostic {
        hook_specific_output: GuardDiagnosticInner {
            additional_context: format!(
                "REMARGIN SESSION GUARD FAILURE -- remargin path enforcement may be silently \
                 disabled for this session: {reasons}. Do NOT assume remargin-managed files are \
                 protected: treat every `.md` under a `.remargin.yaml` realm as remargin-managed \
                 regardless, and run `remargin doctor` to diagnose before touching managed paths."
            ),
            hook_event_name: "SessionStart",
        },
        system_message: format!(
            "remargin session guard: path enforcement may be SILENTLY DISABLED for this session \
             ({reasons}). Run `remargin doctor`."
        ),
    }
}
