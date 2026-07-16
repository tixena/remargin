//! `remargin doctor` core — health checks for the remargin permission stack.
//!
//! Runs a sequence of checks that surface drift and misconfiguration.
//! The hook-installed check runs first and is a gate: when the
//! `PreToolUse` hook is absent from both settings files, no other check
//! can provide meaningful signal (the hook is the single source of
//! truth for enforcement), so the report leads with `HookMissing` and
//! subsequent checks are skipped.
//!
//! Pure (no stdout/stdin): the CLI / MCP handlers own I/O.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use serde::{Deserialize, Serialize};

use crate::permissions::pretool_install::{self, TestOutcome};
use crate::permissions::session_guard_install::{self, TestOutcome as GuardTestOutcome};

/// Severity levels for doctor findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Severity {
    /// No enforcement at all — blocks every other meaningful check.
    Critical,
    /// Enforcement is degraded or a configuration error is present.
    Warning,
}

/// Identifies the specific issue a finding describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FindingKind {
    /// The `PreToolUse` hook (`remargin claude pretool`) is absent from
    /// both user-scope and project-scope settings files. No CLI or
    /// native-tool enforcement is active for any managed path in the
    /// realm. All subsequent checks are skipped.
    HookMissing,
    /// The `SessionStart` guard (`remargin claude session-guard`) is
    /// absent from both user-scope and project-scope settings files.
    /// Without it, a broken enforcement path (e.g. `remargin` fell off
    /// `PATH`) fails open silently — the guard is the fail-open backstop
    /// that surfaces such a failure into the session.
    SessionGuardMissing,
}

/// A single diagnostic finding from a doctor check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DoctorFinding {
    /// What the finding is.
    pub kind: FindingKind,

    /// Human-readable description of the problem.
    pub message: String,

    /// Suggested remediation command or action.
    pub remedy: String,

    /// Severity of the finding.
    pub severity: Severity,
}

/// Output of a `remargin doctor` run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DoctorReport {
    /// Findings in report order. Empty = clean.
    pub findings: Vec<DoctorFinding>,

    /// Whether the hook-installed check passed. When `false`, subsequent
    /// checks were skipped.
    pub hook_installed: bool,

    /// Project-scope settings file that was tested for the hook.
    pub project_settings_file: PathBuf,

    /// Whether the `SessionStart` guard is registered in either scope.
    pub session_guard_installed: bool,

    /// User-scope settings file that was tested for the hook.
    pub user_settings_file: PathBuf,
}

impl DoctorReport {
    /// `true` when there are no findings.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Run `remargin doctor` against the realm at `cwd`.
///
/// Checks (in order):
///
/// 1. **Hook-installed** — verifies the `PreToolUse` hook is present in
///    either `user_settings_file` or `project_settings_file`. When
///    absent from both, emits a `HookMissing` finding and short-circuits
///    (all subsequent checks would be moot).
/// 2. **Session-guard-installed** — verifies the `SessionStart` guard
///    (`remargin claude session-guard`) is present in either scope. When
///    absent from both, emits a `SessionGuardMissing` finding: without
///    the guard, a broken enforcement path fails open silently.
///
/// # Errors
///
/// I/O or JSON parse errors while reading settings files.
pub fn run_doctor(
    system: &dyn System,
    cwd: &Path,
    user_settings_file: &Path,
) -> Result<DoctorReport> {
    let project_settings_file = cwd.join(".claude/settings.local.json");

    let user_outcome = pretool_install::test(system, user_settings_file)?;
    let project_outcome = pretool_install::test(system, &project_settings_file)?;

    let hook_installed = matches!(
        (&user_outcome, &project_outcome),
        (TestOutcome::Installed, _) | (_, TestOutcome::Installed)
    );

    let user_guard = session_guard_install::test(system, user_settings_file)?;
    let project_guard = session_guard_install::test(system, &project_settings_file)?;
    let session_guard_installed = matches!(
        (&user_guard, &project_guard),
        (GuardTestOutcome::Installed, _) | (_, GuardTestOutcome::Installed)
    );

    let mut findings: Vec<DoctorFinding> = Vec::new();

    if !hook_installed {
        findings.push(DoctorFinding {
            kind: FindingKind::HookMissing,
            message: format!(
                "The PreToolUse hook (`remargin claude pretool`) is not registered in either \
                 the user-scope settings ({}) or the project-scope settings ({}). No \
                 enforcement is active — agents can invoke the remargin CLI and bypass \
                 path restrictions without restriction.",
                user_settings_file.display(),
                project_settings_file.display()
            ),
            remedy: String::from("Run `remargin claude pretool install` to register the hook."),
            severity: Severity::Critical,
        });
        // Short-circuit: no further checks are meaningful without the hook.
        return Ok(DoctorReport {
            findings,
            hook_installed,
            session_guard_installed,
            project_settings_file,
            user_settings_file: user_settings_file.to_path_buf(),
        });
    }

    if !session_guard_installed {
        findings.push(DoctorFinding {
            kind: FindingKind::SessionGuardMissing,
            message: format!(
                "The SessionStart guard (`remargin claude session-guard`) is not registered in \
                 either the user-scope settings ({}) or the project-scope settings ({}). The \
                 PreToolUse hook fails open — if `remargin` falls off PATH it exits 127 \
                 (non-blocking) and gated tool calls proceed unprotected with no signal. The \
                 guard is the backstop that surfaces that failure into the session.",
                user_settings_file.display(),
                project_settings_file.display()
            ),
            remedy: String::from(
                "Run `remargin claude session-guard install` to register the guard.",
            ),
            severity: Severity::Critical,
        });
    }

    Ok(DoctorReport {
        findings,
        hook_installed,
        session_guard_installed,
        project_settings_file,
        user_settings_file: user_settings_file.to_path_buf(),
    })
}

/// Render a [`DoctorReport`] as human-readable text.
///
/// When `verbose` is `true`, a `Checks:` section is appended after the
/// findings block (or after the clean message) listing the hook-installed
/// verdict and the paths of both settings files that were inspected.
/// This section appears in both the clean and findings cases.
#[must_use]
pub fn render_doctor_text(report: &DoctorReport, verbose: bool) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    if report.is_clean() {
        let _ = writeln!(out, "doctor: all checks passed");
    } else {
        for finding in &report.findings {
            let label = match finding.severity {
                Severity::Critical => "CRITICAL",
                Severity::Warning => "WARNING",
            };
            let _ = writeln!(out, "[{label}] {}", finding.message);
            let _ = writeln!(out, "  Remedy: {}", finding.remedy);
        }
    }
    if verbose {
        let hook_verdict = if report.hook_installed {
            "ok"
        } else {
            "missing"
        };
        let guard_verdict = if report.session_guard_installed {
            "ok"
        } else {
            "missing"
        };
        let _ = writeln!(out, "Checks:");
        let _ = writeln!(out, "  hook-installed: {hook_verdict}");
        let _ = writeln!(out, "  session-guard: {guard_verdict}");
        let _ = writeln!(
            out,
            "  user-settings: {}",
            report.user_settings_file.display()
        );
        let _ = writeln!(
            out,
            "  project-settings: {}",
            report.project_settings_file.display(),
        );
    }
    out
}
