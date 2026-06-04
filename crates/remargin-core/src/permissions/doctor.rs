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
            project_settings_file,
            user_settings_file: user_settings_file.to_path_buf(),
        });
    }

    Ok(DoctorReport {
        findings,
        hook_installed,
        project_settings_file,
        user_settings_file: user_settings_file.to_path_buf(),
    })
}
