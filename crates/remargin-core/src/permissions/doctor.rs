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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use serde::{Deserialize, Serialize};

use crate::config::permissions::resolve::resolve_permissions;
use crate::permissions::claude_sync::{self, RuleSet, canonicalize_rule, rules_for};
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
    /// A static `permissions.deny` rule in a settings file is drift the
    /// hook has made redundant: either a path rule `rules_for` still
    /// projects for this realm (now enforced by the `PreToolUse` hook,
    /// so the static copy is a duplicate with no removal path) or a
    /// stale `Bash(remargin *)` CLI deny the synchronizer no longer
    /// emits. Each finding names the file and the exact rule string.
    LeftoverProjectedRule,
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

/// Why a `permissions.deny` rule is flagged as leftover drift.
enum LeftoverReason {
    /// Path rule the hook now covers — `rules_for` still projects it,
    /// so the static copy in settings is a redundant duplicate.
    Projected,
    /// Stale `Bash(remargin *)` CLI deny the synchronizer no longer
    /// emits — CLI denial is the hook's job via `cli_allowed`.
    StaleCli,
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

    let settings_files = [
        user_settings_file.to_path_buf(),
        project_settings_file.clone(),
    ];
    findings.extend(leftover_projected_rule_findings(
        system,
        cwd,
        &settings_files,
    )?);

    Ok(DoctorReport {
        findings,
        hook_installed,
        session_guard_installed,
        project_settings_file,
        user_settings_file: user_settings_file.to_path_buf(),
    })
}

/// One [`FindingKind::LeftoverProjectedRule`] per deny rule the hook has
/// made redundant, across every file in `settings_files`.
///
/// The projected set is computed by reusing [`rules_for`] over the
/// realm's resolved `trusted_roots` (plus `allow_dot_folders`), so the
/// detector shares one rule-shape engine with the synchronizer instead
/// of re-deriving the shapes. Any on-disk deny rule that (canonically)
/// lands in that projected set is flagged; a `Bash(remargin *)`-shaped
/// deny — which `rules_for` no longer emits — is flagged as stale.
fn leftover_projected_rule_findings(
    system: &dyn System,
    cwd: &Path,
    settings_files: &[PathBuf],
) -> Result<Vec<DoctorFinding>> {
    let resolved = resolve_permissions(system, cwd)?;
    let allow_dot_folders = resolved.allow_dot_folder_names();

    let mut projected = RuleSet::default();
    for entry in &resolved.trusted_roots {
        let rules = rules_for(entry, cwd, &allow_dot_folders);
        projected.deny.extend(rules.deny);
        projected.allow.extend(rules.allow);
    }
    let projected_deny: HashSet<String> = projected
        .deny
        .iter()
        .map(|rule| canonicalize_rule(rule))
        .collect();

    // Reuse the synchronizer's simulator for the file read / JSON parse
    // and the on-disk deny extraction; the projected `RuleSet` is what
    // makes its `deny_rules_already_present` split meaningful here.
    let sims = claude_sync::simulate_apply_rules(system, settings_files, &projected)?;

    let mut findings = Vec::new();
    for sim in &sims {
        for rule in &sim.existing_deny_rules {
            let canonical = canonicalize_rule(rule);
            let reason = if projected_deny.contains(&canonical) {
                Some(LeftoverReason::Projected)
            } else if is_stale_remargin_cli_deny(&canonical) {
                Some(LeftoverReason::StaleCli)
            } else {
                None
            };
            if let Some(matched) = reason {
                findings.push(leftover_finding(rule, &sim.path, &matched));
            }
        }
    }
    Ok(findings)
}

/// `true` when `canonical_rule` is a `Bash(remargin …)` deny — the CLI
/// deny shape the synchronizer retired. Matches the bare `Bash(remargin
/// *)` and any path-anchored survivor from an older sync.
fn is_stale_remargin_cli_deny(canonical_rule: &str) -> bool {
    canonical_rule
        .strip_prefix("Bash(")
        .and_then(|inner| inner.strip_suffix(')'))
        .is_some_and(|inner| inner.split_whitespace().next() == Some("remargin"))
}

fn leftover_finding(rule: &str, file: &Path, reason: &LeftoverReason) -> DoctorFinding {
    let message = match reason {
        LeftoverReason::Projected => format!(
            "The deny rule `{rule}` in {} duplicates enforcement the PreToolUse hook now \
             provides for this realm; it is drift with no removal path, since the hook is \
             the single source of truth.",
            file.display()
        ),
        LeftoverReason::StaleCli => format!(
            "The deny rule `{rule}` in {} is a stale entry the synchronizer no longer emits — \
             CLI denial is enforced by the hook via the folder-level `cli_allowed` field.",
            file.display()
        ),
    };
    DoctorFinding {
        kind: FindingKind::LeftoverProjectedRule,
        message,
        remedy: format!(
            "Remove the deny rule `{rule}` from the permissions.deny array in {}.",
            file.display()
        ),
        severity: Severity::Warning,
    }
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

/// Render a [`DoctorReport`] as an agent-executable repair prompt.
///
/// A third renderer beside [`render_doctor_text`] and the `--json`
/// serialization, over the *same* report — it carries no detection
/// logic. Each finding contributes one imperative instruction (its
/// `remedy`); a clean report yields a "nothing to do" line. Piping the
/// output to an agent (`remargin doctor --prompt-mode | claude -p`)
/// repairs exactly what the human report named.
#[must_use]
pub fn render_doctor_prompt(report: &DoctorReport) -> String {
    use core::fmt::Write as _;
    if report.is_clean() {
        return String::from(
            "remargin doctor found no drift in this realm's Claude settings. Nothing to do.\n",
        );
    }
    let mut out = String::new();
    let count = report.findings.len();
    let _ = writeln!(
        out,
        "You are an automated repair agent. `remargin doctor` found {count} issue(s) in this \
         realm's Claude settings. Carry out each instruction below exactly, then stop.\n"
    );
    for (idx, finding) in report.findings.iter().enumerate() {
        let _ = writeln!(out, "{}. {}", idx + 1, finding.remedy);
    }
    out
}
