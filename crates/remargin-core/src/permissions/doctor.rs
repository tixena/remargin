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

use crate::config::identity::IdentityFlags;
use crate::config::permissions::resolve::{
    PermissionsLintError, TrustedRootEscape, find_trusted_root_escapes,
    lint_permissions_in_parents, resolve_permissions,
};
use crate::config::{Mode, ResolvedConfig};
use crate::parser::AuthorType;
use crate::permissions::claude_sync::{self, RuleSet, canonicalize_rule, hook_covered_rules};
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
    /// An agent identity whose resolved key path lives under the user's
    /// primary SSH directory (`~/.ssh`) — the agent can read and sign
    /// with the human's keys, a privilege-boundary violation.
    AgentKeyUnderUserSsh,
    /// A `.remargin.yaml` in the realm's parent walk fails the
    /// permissions schema: a YAML syntax error, an unknown key under
    /// `permissions:`, an unknown op name, or the retired legacy `to:`
    /// field on a `deny_ops` entry. Names the file, the parser location
    /// when one was surfaced, and the raw diagnostic. `trusted_roots`
    /// escapes are excluded — they carry their own
    /// [`FindingKind::TrustedRootEscape`].
    ConfigSchemaLint,
    /// The `PreToolUse` hook (`remargin claude pretool`) is absent from
    /// both user-scope and project-scope settings files. No CLI or
    /// native-tool enforcement is active for any managed path in the
    /// realm. All subsequent checks are skipped.
    HookMissing,
    /// Strict-mode realm whose resolved signing key does not point at an
    /// existing, readable file. Identity resolution admits a set-but-
    /// broken `key:`; the failure otherwise surfaces only inside a later
    /// sign/write op as a confusing I/O error.
    IdentityKeyUnresolvable,
    /// A static `permissions.deny` rule in a settings file is drift the
    /// hook has made redundant: either a path rule in
    /// [`hook_covered_rules`] for this realm (now enforced by the
    /// `PreToolUse` hook, so the static copy is a duplicate an older
    /// restrict left behind) or a stale `Bash(remargin *)` CLI deny the
    /// synchronizer no longer emits. Each finding names the file and the
    /// exact rule string.
    LeftoverProjectedRule,
    /// The `SessionStart` guard (`remargin claude session-guard`) is
    /// absent from both user-scope and project-scope settings files.
    /// Without it, a broken enforcement path (e.g. `remargin` fell off
    /// `PATH`) fails open silently — the guard is the fail-open backstop
    /// that surfaces such a failure into the session.
    SessionGuardMissing,
    /// A `trusted_roots` entry resolves outside the realm that declares
    /// it. Fail-closed at resolve time; doctor names the file, the entry,
    /// and the resolved anchor so it can be moved back inside the realm.
    TrustedRootEscape,
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
    /// Path rule the hook now covers — it is in [`hook_covered_rules`],
    /// so the static copy in settings is a duplicate an older restrict
    /// left behind.
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
    // Same file `install --local` writes, so a local install is visible.
    let project_settings_file = cwd.join(".claude/settings.json");

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

    // Lint containment before resolving: an out-of-realm entry makes
    // `resolve_permissions` (which the leftover check walks through)
    // fail closed, so doctor must name the misconfig here rather than
    // crash on the very error it exists to explain.
    let escapes = find_trusted_root_escapes(system, cwd)?;
    let has_escape = !escapes.is_empty();
    findings.extend(escapes.iter().map(trusted_root_escape_finding));

    // Schema lint reads configs via `lint_permissions_in_parents`, which
    // never resolves or bails, so it is safe on a malformed config — and it
    // must run before the resolve-dependent checks so a parse/schema fault
    // is named here rather than swallowed by their fail-closed `?`. Escapes
    // are filtered out (they own `TrustedRootEscape` above), so a non-empty
    // result means a fault that also makes `resolve_permissions` bail.
    let schema_lints = config_schema_lint_findings(system, cwd)?;
    let config_resolves = schema_lints.is_empty();
    findings.extend(schema_lints);

    // Leftover and identity both walk `resolve_permissions` /
    // `ResolvedConfig::resolve`, which fail closed on an out-of-realm root
    // and bail on any parse/schema fault. Run them only when the config is
    // clean on both counts, so doctor names the misconfig above instead of
    // crashing on the very error it exists to explain.
    if !has_escape && config_resolves {
        // Drift lives where the retired projection wrote: restrict emitted
        // rules into settings.local.json, so that file is scanned alongside
        // the hook-scope files.
        let settings_files = [
            user_settings_file.to_path_buf(),
            project_settings_file.clone(),
            cwd.join(".claude/settings.local.json"),
        ];
        findings.extend(leftover_projected_rule_findings(
            system,
            cwd,
            &settings_files,
        )?);
        findings.extend(identity_key_findings(system, cwd)?);
    }

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
/// The reference set is computed by reusing [`hook_covered_rules`] over
/// the realm's resolved `trusted_roots` (plus `allow_dot_folders`), so
/// the detector shares one rule-shape engine with the synchronizer
/// instead of re-deriving the shapes. Any on-disk deny rule that
/// (canonically) lands in that set is flagged as a leftover an older
/// restrict projected; a `Bash(remargin *)`-shaped deny — which the
/// synchronizer never emits — is flagged as stale.
fn leftover_projected_rule_findings(
    system: &dyn System,
    cwd: &Path,
    settings_files: &[PathBuf],
) -> Result<Vec<DoctorFinding>> {
    let resolved = resolve_permissions(system, cwd)?;
    let allow_dot_folders = resolved.allow_dot_folder_names();

    let mut projected = RuleSet::default();
    for entry in &resolved.trusted_roots {
        let rules = hook_covered_rules(entry, cwd, &allow_dot_folders);
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

fn trusted_root_escape_finding(escape: &TrustedRootEscape) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::TrustedRootEscape,
        message: escape.message(),
        remedy: format!(
            "Move the trusted_roots entry `{}` in {} so it resolves at or below {}, or run \
             `remargin restrict` from the folder that actually contains {}.",
            escape.entry,
            escape.source_file.display(),
            escape.realm_dir.display(),
            escape.anchor.display(),
        ),
        severity: Severity::Warning,
    }
}

/// One [`FindingKind::ConfigSchemaLint`] per permissions-schema fault in
/// the realm's parent walk, reusing [`lint_permissions_in_parents`]
/// verbatim so there is a single parse path. `trusted_roots` escapes are
/// dropped here: [`find_trusted_root_escapes`] feeds the dedicated
/// [`FindingKind::TrustedRootEscape`] and the lint arm emits the same
/// `escape.message()` string, so message-equality de-dup is exact — a
/// realm with an out-of-realm root shows one escape finding, not two.
fn config_schema_lint_findings(system: &dyn System, cwd: &Path) -> Result<Vec<DoctorFinding>> {
    let escape_messages: HashSet<String> = find_trusted_root_escapes(system, cwd)?
        .iter()
        .map(TrustedRootEscape::message)
        .collect();
    Ok(lint_permissions_in_parents(system, cwd)?
        .into_iter()
        .filter(|err| !escape_messages.contains(&err.message))
        .map(|err| DoctorFinding {
            kind: FindingKind::ConfigSchemaLint,
            message: schema_lint_message(&err),
            remedy: format!(
                "Fix the permissions schema in {}.",
                err.source_file.display()
            ),
            severity: Severity::Warning,
        })
        .collect())
}

/// The source file, the parser's location when it surfaced one, then the
/// raw diagnostic.
fn schema_lint_message(err: &PermissionsLintError) -> String {
    let location = match (err.line, err.column) {
        (Some(line), Some(col)) => format!(" (line {line}, col {col})"),
        (Some(line), None) => format!(" (line {line})"),
        (None, Some(col)) => format!(" (col {col})"),
        (None, None) => String::new(),
    };
    format!("{}{location}: {}", err.source_file.display(), err.message)
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

/// Read-only diagnostics over the realm's resolved signing identity.
///
/// Two failure modes that pass identity resolution but bite at op time:
/// a strict-mode `key:` that is set but points at no readable file, and an
/// agent identity whose key resolves under the user's `~/.ssh`. The
/// `key:`-is-`None` case is deliberately not reported — that is already a
/// hard `validate_identity` error, surfaced when the config resolves.
fn identity_key_findings(system: &dyn System, cwd: &Path) -> Result<Vec<DoctorFinding>> {
    let config = ResolvedConfig::resolve(system, cwd, &IdentityFlags::default(), None)?;
    let mut findings = Vec::new();
    let (Some(identity), Some(key_path)) = (config.identity.as_deref(), config.key_path.as_deref())
    else {
        return Ok(findings);
    };

    if config.mode == Mode::Strict && !key_is_readable(system, key_path) {
        findings.push(identity_key_unresolvable_finding(
            identity,
            key_path,
            config.source_path.as_deref(),
        ));
    }

    if identity_is_agent(&config, identity) && key_under_user_ssh(system, key_path) {
        findings.push(agent_key_under_ssh_finding(identity, key_path));
    }

    Ok(findings)
}

/// A key is usable only when it reads back as a file. Mirrors the
/// `read_to_string` probe the config loader uses, so a missing file and an
/// unreadable one collapse to the same "not a readable file" verdict.
fn key_is_readable(system: &dyn System, key_path: &Path) -> bool {
    system.read_to_string(key_path).is_ok()
}

/// The active identity is an agent. The registry participant's `type:` is
/// authoritative when the realm carries a registry; otherwise the config's
/// own `type:` decides (open realms need no registry).
fn identity_is_agent(config: &ResolvedConfig, identity: &str) -> bool {
    if let Some(registry) = &config.registry
        && let Some(participant) = registry.participants.get(identity)
    {
        return participant.author_type == "agent";
    }
    matches!(config.author_type, Some(AuthorType::Agent))
}

/// `key_path` lives at or below the user's primary SSH directory. `~/.ssh`
/// is derived from `HOME` the same way `key:` resolution derives it, so a
/// non-standard home resolves identically; a missing `HOME` means there is
/// no `~/.ssh` to compare against.
fn key_under_user_ssh(system: &dyn System, key_path: &Path) -> bool {
    let Ok(home) = system.env_var("HOME") else {
        return false;
    };
    key_path.starts_with(PathBuf::from(home).join(".ssh"))
}

fn identity_key_unresolvable_finding(
    identity: &str,
    key_path: &Path,
    source_path: Option<&Path>,
) -> DoctorFinding {
    let declared_in = source_path.map_or_else(
        || String::from("its `.remargin.yaml`"),
        |path| format!("declared in {}", path.display()),
    );
    let remedy_where = source_path.map_or_else(
        || String::from("the realm's `.remargin.yaml`"),
        |path| path.display().to_string(),
    );
    DoctorFinding {
        kind: FindingKind::IdentityKeyUnresolvable,
        message: format!(
            "Identity `{identity}` runs in strict mode but its signing key `{}` ({declared_in}) \
             is not a readable file. Signing and writes will fail.",
            key_path.display(),
        ),
        remedy: format!(
            "Fix the `key:` path in {remedy_where} or pass --key pointing at a readable key file."
        ),
        severity: Severity::Warning,
    }
}

fn agent_key_under_ssh_finding(identity: &str, key_path: &Path) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::AgentKeyUnderUserSsh,
        message: format!(
            "Agent identity `{identity}`'s key `{}` lives under your ~/.ssh — the agent can sign \
             with your personal keys.",
            key_path.display(),
        ),
        remedy: String::from(
            "Move the agent's key out of ~/.ssh and update the `key:` field in the realm's \
             `.remargin.yaml`.",
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
