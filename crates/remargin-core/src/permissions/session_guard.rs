//! `remargin claude session-guard` core — Claude Code `SessionStart` hook.
//!
//! Re-verifies that path enforcement will be live for the session:
//!
//! 1. the bare `remargin` binary resolves on `PATH` — the `PreToolUse`
//!    hook command is `remargin claude pretool`, and a command that
//!    cannot be found exits 127, which Claude Code treats as a
//!    *non-blocking* failure. The gated tool call then proceeds
//!    unprotected, silently. If the guard itself ran but bare `remargin`
//!    no longer resolves, every future tool call in the session is
//!    exposed;
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
use std::path::Path;

use os_shim::System;
use serde::Serialize;

use crate::config;

/// The bare command name a `PreToolUse` hook (`remargin claude pretool`)
/// resolves through `PATH`. If this does not resolve, enforcement is off.
const BINARY_NAME: &str = "remargin";

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
    /// Injected into Claude's context at session start — the model reads
    /// this and must not treat managed files as protected.
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
    /// Enforcement may be silently disabled. Emit the diagnostic JSON on
    /// stdout so the failure is surfaced into the session.
    Fail(GuardDiagnostic),
    /// Enforcement will be live. Emit nothing; the session proceeds clean.
    Ok,
}

/// Re-verify that enforcement will be live for a session rooted at `cwd`.
#[must_use]
pub fn session_guard(system: &dyn System, cwd: &Path) -> GuardOutcome {
    let mut failures: Vec<String> = Vec::new();

    if !remargin_on_path(system) {
        failures.push(String::from(
            "the `remargin` binary does not resolve on PATH -- a PreToolUse hook \
             (`remargin claude pretool`) that cannot find `remargin` exits 127, which Claude Code \
             treats as non-blocking, so every gated tool call proceeds unprotected",
        ));
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

/// Resolve the bare [`BINARY_NAME`] against `PATH` through the [`System`]
/// shim (never raw `std::env`), so the check reflects the same lookup a
/// child `remargin claude pretool` invocation would perform. [`split_paths`]
/// operates on the value we read — it does not touch process env — so it
/// stays hermetic under `MockSystem`.
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
