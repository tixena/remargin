//! `remargin doctor` core — health checks for the remargin permission stack.
//!
//! Runs a sequence of checks that surface drift and misconfiguration.
//! The hook-installed check runs first and is a gate: when the
//! `PreToolUse` hook is absent from both settings files, no other check
//! can provide meaningful signal (the hook is the single source of
//! truth for enforcement), so subsequent checks are skipped and the
//! report leads with `HookMissing` whenever that check is selected.
//!
//! Pure (no stdout/stdin): the CLI / MCP handlers own I/O.

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use serde::{Deserialize, Serialize};
use tixschema::model_schema;

use crate::config::identity::IdentityFlags;
use crate::config::permissions::resolve::{
    PermissionsLintError, ResolvedTrustedRoot, TrustedRootEscape, find_trusted_root_escapes,
    lint_permissions_in_parents, resolve_permissions, trusted_root_anchor,
};
use crate::config::{Mode, ResolvedConfig};
use crate::operations::sandbox;
use crate::parser::AuthorType;
use crate::permissions::claude_sync::{self, RuleSet, canonicalize_rule, hook_covered_rules};
use crate::permissions::goose_install::{self, TestOutcome as GooseTestOutcome};
use crate::permissions::goose_mcp_install::{self, TestOutcome as GooseMcpTestOutcome};
use crate::permissions::pretool_install::{self, TestOutcome};
use crate::permissions::session_guard_install::{self, TestOutcome as GuardTestOutcome};

/// Severity levels for doctor findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
#[model_schema]
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
#[model_schema]
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
    /// goose is installed and the `remargin-guard` plugin is present, but
    /// it does not describe a live guard — an unparseable `hooks.json`, no
    /// `PreToolUse` entry, or a command pointing at a binary that is gone.
    /// goose fails open on a hook it cannot run, so a broken plugin is
    /// indistinguishable from no plugin at all from inside a session.
    GooseGuardBroken,
    /// goose is installed but the `remargin-guard` plugin is absent from
    /// both the user-scope and project-scope plugin roots. goose sessions
    /// can shell and edit remargin-managed paths with no guard.
    GooseGuardMissing,
    /// goose is installed and its guard is wired, but remargin is not
    /// registered as a goose MCP extension in either config the CLI
    /// writes. The guard blocks native tools on managed paths and
    /// redirects the agent to remargin's ops — ops a session without the
    /// extension never received, which turns every redirect into a dead
    /// end rather than a detour.
    GooseMcpMissing,
    /// goose is installed but the guard plugin declares no live
    /// `SessionStart` entry (`remargin goose session-guard`) in either
    /// scope. goose fails open on a hook it cannot run, so a `PreToolUse`
    /// guard that breaks — its binary moved, its manifest corrupted — is
    /// indistinguishable from a live one from inside a session. The
    /// `SessionStart` entry is the backstop that says so out loud.
    GooseSessionGuardMissing,
    /// The `PreToolUse` hook (`remargin claude pretool`) is not live in
    /// either the user-scope or the project-scope settings file: absent
    /// from both, or declared with a command that cannot spawn because the
    /// binary it names is gone. No CLI or native-tool enforcement is active
    /// for any managed path in the realm. All subsequent checks are
    /// skipped.
    HookMissing,
    /// A Claude hook entry names the remargin binary by bare name rather
    /// than absolute path — the form installs wrote before they embedded
    /// it. Claude Code resolves it through `PATH` at spawn time and treats
    /// a command it cannot find as non-blocking, so the entry is one `PATH`
    /// change away from silently gating nothing. Reinstalling rewrites it;
    /// doctor only reports, because rewriting a user's settings is
    /// `install`'s job and not a diagnostic's.
    HookPathRelative,
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
    /// The `SessionStart` guard (`remargin claude session-guard`) is not
    /// live in either the user-scope or the project-scope settings file:
    /// absent from both, or declared with a command that cannot spawn.
    /// Without it, a broken enforcement path (e.g. the `PreToolUse` hook's
    /// binary moved) fails open silently — the guard is the fail-open
    /// backstop that surfaces such a failure into the session.
    SessionGuardMissing,
    /// A document's `sandbox:` frontmatter carries an `author@timestamp`
    /// entry whose author is not an active registry participant — a
    /// retired agent, a revoked identity, or an author dropped when a
    /// registry was rotated. Sandbox removal is per-identity and never
    /// garbage-collected, so the entry silently keeps the file "staged"
    /// for a participant who no longer exists. Names the file and the
    /// orphaned author.
    StaleSandboxEntry,
    /// A `trusted_roots` entry resolves outside the realm that declares
    /// it. Fail-closed at resolve time; doctor names the file, the entry,
    /// and the resolved anchor so it can be moved back inside the realm.
    TrustedRootEscape,
    /// A `trusted_roots` entry that stays inside its realm (so not a
    /// [`FindingKind::TrustedRootEscape`]) but resolves to a path absent
    /// on disk — a moved or deleted target. The root then protects
    /// nothing; doctor names the resolved anchor and the declaring
    /// `.remargin.yaml` so it can be repointed or dropped.
    TrustedRootMissing,
}

/// A single diagnostic finding from a doctor check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[model_schema]
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
#[model_schema]
pub struct DoctorReport {
    /// Milliseconds the call took, written by the io layer on the way out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,

    /// Findings in report order. Empty = clean.
    pub findings: Vec<DoctorFinding>,

    /// Whether the goose guard plugin is wired in either plugin scope.
    /// `None` when no goose installation was found — a machine that does
    /// not run goose has no verdict, which is not the same as a `false`.
    /// The unset verdict is omitted from the wire rather than nulled: the
    /// generated contract renders `Option` as the absent form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goose_guard_installed: Option<bool>,

    /// Whether remargin is registered as a goose MCP extension in either
    /// config the CLI writes, `None` under the same no-installation
    /// condition as [`goose_guard_installed`](Self::goose_guard_installed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goose_mcp_installed: Option<bool>,

    /// Whether the goose `SessionStart` backstop is wired in either plugin
    /// scope, `None` under the same no-installation condition as
    /// [`goose_guard_installed`](Self::goose_guard_installed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goose_session_guard_installed: Option<bool>,

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

/// Both Claude settings scopes, probed once per run.
///
/// Every verdict about the two hook entries — whether they are live, the
/// fault when they are not, and the entries still naming the binary by bare
/// name — is read off this one snapshot, so no two answers in a report can
/// describe stacks observed at different moments.
struct ClaudeProbe {
    /// User scope first, then project scope.
    files: [PathBuf; 2],
    /// Per scope, in the order of `files`.
    hook: [TestOutcome; 2],
    /// Per scope, in the order of `files`.
    session_guard: [GuardTestOutcome; 2],
}

impl ClaudeProbe {
    /// The stale-binary fault behind a `PreToolUse` entry that cannot run.
    fn hook_fault(&self) -> Option<&str> {
        self.hook.iter().find_map(|outcome| {
            if let TestOutcome::Broken(reason) = outcome {
                Some(reason.as_str())
            } else {
                None
            }
        })
    }

    /// A `PATH`-relative entry still gates every tool call while `PATH`
    /// resolves it, so it counts as installed and earns its own warning
    /// rather than the critical "no enforcement at all" verdict.
    fn hook_installed(&self) -> bool {
        self.hook.iter().any(|outcome| {
            matches!(
                *outcome,
                TestOutcome::Installed | TestOutcome::PathRelative(_)
            )
        })
    }

    /// One finding per scope whose `PreToolUse` entry still names the
    /// binary by bare name.
    fn hook_path_relative_findings(&self) -> Vec<DoctorFinding> {
        self.hook
            .iter()
            .zip(&self.files)
            .filter_map(|(outcome, file)| {
                if let TestOutcome::PathRelative(command) = outcome {
                    Some(path_relative_finding(
                        "PreToolUse hook",
                        command,
                        file,
                        "remargin claude pretool install",
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    /// # Errors
    ///
    /// I/O or JSON parse errors while reading either settings file.
    fn probe(system: &dyn System, user_file: &Path, project_file: &Path) -> Result<Self> {
        Ok(Self {
            files: [user_file.to_path_buf(), project_file.to_path_buf()],
            hook: [
                pretool_install::test(system, user_file)?,
                pretool_install::test(system, project_file)?,
            ],
            session_guard: [
                session_guard_install::test(system, user_file)?,
                session_guard_install::test(system, project_file)?,
            ],
        })
    }

    /// The stale-binary fault behind a `SessionStart` entry that cannot run.
    fn session_guard_fault(&self) -> Option<&str> {
        self.session_guard.iter().find_map(|outcome| {
            if let GuardTestOutcome::Broken(reason) = outcome {
                Some(reason.as_str())
            } else {
                None
            }
        })
    }

    fn session_guard_installed(&self) -> bool {
        self.session_guard.iter().any(|outcome| {
            matches!(
                *outcome,
                GuardTestOutcome::Installed | GuardTestOutcome::PathRelative(_)
            )
        })
    }

    /// One finding per scope whose `SessionStart` entry still names the
    /// binary by bare name.
    fn session_guard_path_relative_findings(&self) -> Vec<DoctorFinding> {
        self.session_guard
            .iter()
            .zip(&self.files)
            .filter_map(|(outcome, file)| {
                if let GuardTestOutcome::PathRelative(command) = outcome {
                    Some(path_relative_finding(
                        "SessionStart guard",
                        command,
                        file,
                        "remargin claude session-guard install",
                    ))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// The two goose plugin scopes, probed once per run.
///
/// Both goose checks read the same plugin roots, and the report's verdicts
/// sit beside the findings derived from them, so every consumer reads this
/// one snapshot: a second probe could observe a stack that changed between
/// them and report a verdict its own findings contradict.
struct GooseProbe {
    /// Per scope: user first, then project.
    guard: [GooseTestOutcome; 2],
    /// Per config file, in the order of the paths below.
    mcp: [GooseMcpTestOutcome; 2],
    mcp_local_file: PathBuf,
    mcp_user_file: PathBuf,
    project_dir: PathBuf,
    /// Per scope, in the order of `guard`.
    session_guard: [GooseTestOutcome; 2],
    user_dir: PathBuf,
}

impl GooseProbe {
    /// A guard entry counts as wired when either plugin scope reports it
    /// live — `install --local` is a supported wiring.
    fn any_installed(outcomes: &[GooseTestOutcome; 2]) -> bool {
        outcomes
            .iter()
            .any(|outcome| matches!(*outcome, GooseTestOutcome::Installed))
    }

    /// Findings for the guard plugin. A broken plugin in either scope is
    /// reported ahead of a plain absence because it names a different
    /// repair.
    fn guard_findings(&self) -> Vec<DoctorFinding> {
        if self.guard_installed() {
            return Vec::new();
        }
        for outcome in &self.guard {
            if let GooseTestOutcome::Broken(reason) = outcome {
                return vec![goose_guard_broken_finding(reason)];
            }
        }
        vec![goose_guard_missing_finding(
            &self.user_dir,
            &self.project_dir,
        )]
    }

    fn guard_installed(&self) -> bool {
        Self::any_installed(&self.guard)
    }

    /// Findings for the MCP extension. Silent until the guard is wired:
    /// an unguarded session is `GooseGuardMissing`'s finding and its
    /// repair comes first, and a session with neither guard nor extension
    /// has no redirect to be a dead end yet.
    fn mcp_findings(&self) -> Vec<DoctorFinding> {
        if !self.guard_installed() || self.mcp_installed() {
            return Vec::new();
        }
        let fault = self.mcp.iter().find_map(|outcome| {
            if let GooseMcpTestOutcome::Broken(reason) = outcome {
                Some(reason.as_str())
            } else {
                None
            }
        });
        vec![goose_mcp_missing_finding(
            &self.mcp_user_file,
            &self.mcp_local_file,
            fault,
        )]
    }

    fn mcp_installed(&self) -> bool {
        self.mcp
            .iter()
            .any(|outcome| matches!(*outcome, GooseMcpTestOutcome::Installed))
    }

    /// Findings for the `SessionStart` backstop. A `Broken` verdict
    /// contributes its fault to the message rather than a second finding:
    /// the entry is not live either way, and one repair — reinstalling it —
    /// covers both.
    fn session_guard_findings(&self) -> Vec<DoctorFinding> {
        if self.session_guard_installed() {
            return Vec::new();
        }
        let fault = self.session_guard.iter().find_map(|outcome| {
            if let GooseTestOutcome::Broken(reason) = outcome {
                Some(reason.as_str())
            } else {
                None
            }
        });
        vec![goose_session_guard_missing_finding(
            &self.user_dir,
            &self.project_dir,
            fault,
        )]
    }

    fn session_guard_installed(&self) -> bool {
        Self::any_installed(&self.session_guard)
    }
}

/// Why a `permissions.deny` rule is flagged as leftover drift.
enum LeftoverReason {
    /// A path rule in [`hook_covered_rules`]: the hook enforces it, so the settings copy is
    /// redundant.
    Projected,
    /// A `Bash(remargin *)` deny: CLI denial is the hook's job through `cli_allowed`.
    StaleCli,
}

/// A named check `run_doctor` can be asked to run.
///
/// The `--check` flag / MCP `check` param select a subset by slug; an
/// unknown slug is an error, never a silent no-op. `Hook` is special: the
/// hook-installed gate runs regardless of selection because it is
/// enforcement's source of truth, so including or excluding `Hook` changes
/// only whether the leading `HookMissing` finding is *reported*, never
/// whether the gate runs and short-circuits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CheckName {
    /// Permissions-schema faults in the realm's parent walk.
    ConfigSchemaLint,
    /// The goose guard plugin, when a goose installation is present.
    GooseGuard,
    /// remargin's registration as a goose MCP extension, the guard's redirect target.
    GooseMcp,
    /// The goose `SessionStart` backstop.
    GooseSessionGuard,
    /// The `PreToolUse` enforcement hook — always gates the run.
    Hook,
    /// Strict-mode signing-key resolution and agent-key-under-`~/.ssh`.
    IdentityKey,
    /// `permissions.deny` rules the hook has made redundant.
    LeftoverRules,
    /// The `SessionStart` fail-open backstop guard.
    SessionGuard,
    /// `sandbox:` entries whose author is no longer an active participant.
    StaleSandbox,
    /// A `trusted_roots` entry resolving outside its declaring realm.
    TrustedRootEscape,
    /// A `trusted_roots` entry resolving to a path absent on disk.
    TrustedRootMissing,
}

impl CheckName {
    /// In a stable order.
    const ALL: [Self; 11] = [
        Self::ConfigSchemaLint,
        Self::GooseGuard,
        Self::GooseMcp,
        Self::GooseSessionGuard,
        Self::Hook,
        Self::IdentityKey,
        Self::LeftoverRules,
        Self::SessionGuard,
        Self::StaleSandbox,
        Self::TrustedRootEscape,
        Self::TrustedRootMissing,
    ];

    /// The full set — the default when no selection is given.
    #[must_use]
    pub fn all() -> HashSet<Self> {
        Self::ALL.into_iter().collect()
    }

    fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|check| check.slug() == slug)
    }

    /// Parse a comma-separated slug list into a set. Whitespace around each
    /// slug is trimmed and empty tokens are skipped.
    ///
    /// # Errors
    ///
    /// Any token that is not a known check slug — the message names the bad
    /// slug and lists the valid ones, so a typo is a hard error rather than
    /// a silent no-op run.
    pub fn parse_set(csv: &str) -> Result<HashSet<Self>> {
        let mut set = HashSet::new();
        for raw in csv.split(',') {
            let slug = raw.trim();
            if slug.is_empty() {
                continue;
            }
            match Self::from_slug(slug) {
                Some(check) => {
                    set.insert(check);
                }
                None => anyhow::bail!("unknown check `{slug}`; valid: {}", Self::valid_slugs()),
            }
        }
        Ok(set)
    }

    /// This check's stable kebab-case slug for the `--check` flag.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::ConfigSchemaLint => "config-schema-lint",
            Self::GooseGuard => "goose-guard",
            Self::GooseMcp => "goose-mcp",
            Self::GooseSessionGuard => "goose-session-guard",
            Self::Hook => "hook",
            Self::IdentityKey => "identity-key",
            Self::LeftoverRules => "leftover-rules",
            Self::SessionGuard => "session-guard",
            Self::StaleSandbox => "stale-sandbox",
            Self::TrustedRootEscape => "trusted-root-escape",
            Self::TrustedRootMissing => "trusted-root-missing",
        }
    }

    fn valid_slugs() -> String {
        Self::ALL
            .iter()
            .map(|check| check.slug())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Run the checks in `checks` against the realm at `cwd`.
///
/// `checks` selects which checks contribute findings ([`CheckName::all`]
/// runs every one — the default). The hook-installed gate itself is exempt:
/// it runs regardless of selection, and when the `PreToolUse` hook is
/// absent from both `user_settings_file` and `project_settings_file` it
/// short-circuits so no later check can report a false-clean. Its
/// `HookMissing` finding follows the selection like every other check, so a
/// run that did not select `Hook` short-circuits silently — callers
/// scripting the gate must select it.
///
/// Two config-safety preconditions are always computed even when their own
/// findings are deselected: `find_trusted_root_escapes` (an out-of-realm
/// root) and `config_schema_lint_findings` (a parse/schema fault). The
/// resolve-dependent checks below walk `resolve_permissions` /
/// `ResolvedConfig::resolve`, which fail closed on either fault, so they run
/// only when both preconditions hold — independent of whether the escape /
/// schema-lint findings were themselves selected for reporting.
///
/// # Errors
///
/// I/O or JSON parse errors while reading settings files.
pub fn run_doctor(
    system: &dyn System,
    cwd: &Path,
    user_settings_file: &Path,
    checks: &HashSet<CheckName>,
) -> Result<DoctorReport> {
    // Same file `install --local` writes, so a local install is visible.
    let project_settings_file = cwd.join(".claude/settings.json");

    let claude = ClaudeProbe::probe(system, user_settings_file, &project_settings_file)?;
    let hook_installed = claude.hook_installed();
    let session_guard_installed = claude.session_guard_installed();

    // Probed before the hook gate so the verdicts survive its short-circuit; without them the
    // report would read as no goose installation at all.
    let goose = probe_goose(system, cwd)?;
    let goose_guard_installed = goose.as_ref().map(GooseProbe::guard_installed);
    let goose_mcp_installed = goose.as_ref().map(GooseProbe::mcp_installed);
    let goose_session_guard_installed = goose.as_ref().map(GooseProbe::session_guard_installed);

    let mut findings: Vec<DoctorFinding> = Vec::new();

    if !hook_installed {
        if checks.contains(&CheckName::Hook) {
            findings.push(hook_missing_finding(
                user_settings_file,
                &project_settings_file,
                claude.hook_fault(),
            ));
        }
        // Short-circuit: no further checks are meaningful without the hook.
        return Ok(DoctorReport {
            elapsed_ms: None,
            findings,
            goose_guard_installed,
            goose_mcp_installed,
            goose_session_guard_installed,
            hook_installed,
            session_guard_installed,
            project_settings_file,
            user_settings_file: user_settings_file.to_path_buf(),
        });
    }

    if checks.contains(&CheckName::Hook) {
        findings.extend(claude.hook_path_relative_findings());
    }

    if checks.contains(&CheckName::SessionGuard) {
        if session_guard_installed {
            findings.extend(claude.session_guard_path_relative_findings());
        } else {
            findings.push(session_guard_missing_finding(
                user_settings_file,
                &project_settings_file,
                claude.session_guard_fault(),
            ));
        }
    }

    findings.extend(goose_findings(goose.as_ref(), checks));

    // An out-of-realm entry makes `resolve_permissions` fail closed, so the escape scan runs even
    // when its finding is deselected: `has_escape` gates the resolve-dependent block below.
    let escapes = find_trusted_root_escapes(system, cwd)?;
    let has_escape = !escapes.is_empty();
    if checks.contains(&CheckName::TrustedRootEscape) {
        findings.extend(escapes.iter().map(trusted_root_escape_finding));
    }

    // The schema lint never resolves, so it is safe on a malformed config, and it runs before the
    // resolve-dependent checks so a parse fault is named here. Only its findings follow selection.
    let schema_lints = config_schema_lint_findings(system, cwd)?;
    let config_resolves = schema_lints.is_empty();
    if checks.contains(&CheckName::ConfigSchemaLint) {
        findings.extend(schema_lints);
    }

    if !has_escape && config_resolves {
        // `settings.local.json` is where projected rules were written, so it is scanned as well.
        let settings_files = [
            user_settings_file.to_path_buf(),
            project_settings_file.clone(),
            cwd.join(".claude/settings.local.json"),
        ];
        findings.extend(resolve_dependent_findings(
            system,
            cwd,
            checks,
            &settings_files,
        )?);
    }

    Ok(DoctorReport {
        elapsed_ms: None,
        findings,
        goose_guard_installed,
        goose_mcp_installed,
        goose_session_guard_installed,
        hook_installed,
        session_guard_installed,
        project_settings_file,
        user_settings_file: user_settings_file.to_path_buf(),
    })
}

/// Findings from the checks that walk `resolve_permissions` /
/// `ResolvedConfig::resolve`: leftover rules, identity keys, stale sandbox
/// entries, and trusted roots pointing at nothing.
///
/// Every check here fails closed on an out-of-realm trusted root and bails
/// on a parse/schema fault, so the caller runs this only once it has
/// established that the config resolves — otherwise doctor would crash on
/// the very misconfiguration it exists to explain. Each check additionally
/// contributes only when it is selected.
///
/// # Errors
///
/// I/O or parse errors surfaced by the resolvers these checks walk.
fn resolve_dependent_findings(
    system: &dyn System,
    cwd: &Path,
    checks: &HashSet<CheckName>,
    settings_files: &[PathBuf],
) -> Result<Vec<DoctorFinding>> {
    let mut findings = Vec::new();
    if checks.contains(&CheckName::LeftoverRules) {
        findings.extend(leftover_projected_rule_findings(
            system,
            cwd,
            settings_files,
        )?);
    }
    if checks.contains(&CheckName::IdentityKey) {
        findings.extend(identity_key_findings(system, cwd)?);
    }
    if checks.contains(&CheckName::StaleSandbox) {
        findings.extend(stale_sandbox_findings(system, cwd)?);
    }
    if checks.contains(&CheckName::TrustedRootMissing) {
        findings.extend(trusted_root_missing_findings(system, cwd)?);
    }
    Ok(findings)
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

    // The synchronizer's simulator does the file read and the on-disk deny extraction.
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

/// The goose stack as one doctor run observed it, or `None` when there is
/// nothing to observe: `~/.agents` is the root goose discovers user-scope
/// plugins from, so its absence means there is no goose session to guard
/// and the checks have nothing to say.
///
/// # Errors
///
/// I/O errors from the plugin-directory probes.
fn probe_goose(system: &dyn System, cwd: &Path) -> Result<Option<GooseProbe>> {
    let Ok(home_var) = system.env_var("HOME") else {
        return Ok(None);
    };
    let home = PathBuf::from(home_var);
    if !system.exists(&goose_install::agents_dir(&home))? {
        return Ok(None);
    }

    let user_dir = goose_install::plugin_dir(&home);
    let project_dir = goose_install::plugin_dir(cwd);
    let guard = [
        goose_install::test(system, &user_dir)?,
        goose_install::test(system, &project_dir)?,
    ];
    let session_guard = [
        goose_install::test_session_guard(system, &user_dir)?,
        goose_install::test_session_guard(system, &project_dir)?,
    ];
    let mcp_user_file = goose_mcp_install::user_config_file(system, &home);
    let mcp_local_file = goose_mcp_install::local_config_file(cwd);
    let mcp = [
        goose_mcp_install::test(system, &mcp_user_file)?,
        goose_mcp_install::test(system, &mcp_local_file)?,
    ];
    Ok(Some(GooseProbe {
        guard,
        mcp,
        mcp_local_file,
        mcp_user_file,
        project_dir,
        session_guard,
        user_dir,
    }))
}

/// The leading gate finding. A `fault` is the stale-binary case — the entry
/// is registered but its command names a binary that is gone — which fails
/// open exactly as an absent entry does, so it shares the kind and names a
/// different repair.
fn hook_missing_finding(
    user_settings_file: &Path,
    project_settings_file: &Path,
    fault: Option<&str>,
) -> DoctorFinding {
    let (message, remedy) = fault.map_or_else(
        || {
            (
                format!(
                    "The PreToolUse hook (`remargin claude pretool`) is not registered in either \
                     the user-scope settings ({}) or the project-scope settings ({}). No \
                     enforcement is active — agents can invoke the remargin CLI and bypass path \
                     restrictions without restriction.",
                    user_settings_file.display(),
                    project_settings_file.display()
                ),
                String::from("Run `remargin claude pretool install` to register the hook."),
            )
        },
        |reason| {
            (
                format!(
                    "The PreToolUse hook (`remargin claude pretool`) is registered but cannot \
                     run: {reason}. Claude Code treats a hook command it cannot spawn as a \
                     non-blocking failure, so every gated tool call proceeds unprotected with no \
                     signal — the same exposure as no hook at all."
                ),
                String::from(
                    "Run `remargin claude pretool install` to rewrite the entry with the current \
                     binary path.",
                ),
            )
        },
    );
    DoctorFinding {
        kind: FindingKind::HookMissing,
        message,
        remedy,
        severity: Severity::Critical,
    }
}

/// The `SessionStart` backstop's finding. As with the hook gate, a `fault`
/// is the stale-binary case: the entry is registered but cannot spawn, so
/// no backstop runs and the repair is a reinstall rather than a first
/// install.
fn session_guard_missing_finding(
    user_settings_file: &Path,
    project_settings_file: &Path,
    fault: Option<&str>,
) -> DoctorFinding {
    let (message, remedy) = fault.map_or_else(
        || {
            (
                format!(
                    "The SessionStart guard (`remargin claude session-guard`) is not registered \
                     in either the user-scope settings ({}) or the project-scope settings ({}). \
                     The PreToolUse hook fails open — if `remargin` falls off PATH it exits 127 \
                     (non-blocking) and gated tool calls proceed unprotected with no signal. The \
                     guard is the backstop that surfaces that failure into the session.",
                    user_settings_file.display(),
                    project_settings_file.display()
                ),
                String::from("Run `remargin claude session-guard install` to register the guard."),
            )
        },
        |reason| {
            (
                format!(
                    "The SessionStart guard (`remargin claude session-guard`) is registered but \
                     cannot run: {reason}. The backstop that exists to make a fail-open \
                     enforcement path loud is itself silently absent from every session."
                ),
                String::from(
                    "Run `remargin claude session-guard install` to rewrite the entry with the \
                     current binary path.",
                ),
            )
        },
    );
    DoctorFinding {
        kind: FindingKind::SessionGuardMissing,
        message,
        remedy,
        severity: Severity::Critical,
    }
}

/// One entry still naming the binary by bare name. `subject` names which
/// hook, since both Claude entries can carry the legacy form.
fn path_relative_finding(
    subject: &str,
    command: &str,
    settings_file: &Path,
    install_command: &str,
) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::HookPathRelative,
        message: format!(
            "The {subject} entry in {} runs `{command}`, naming the remargin binary by bare name \
             rather than absolute path — the form installs wrote before they embedded it. Claude \
             Code resolves it through PATH at spawn time and treats a command it cannot find as \
             non-blocking, so a PATH change disarms it silently.",
            settings_file.display(),
        ),
        remedy: format!(
            "Run `{install_command}` to rewrite the entry with the absolute binary path."
        ),
        severity: Severity::Warning,
    }
}

fn goose_session_guard_missing_finding(
    user_dir: &Path,
    project_dir: &Path,
    fault: Option<&str>,
) -> DoctorFinding {
    let detail = fault.map_or_else(
        || {
            format!(
                "it is absent from both the user-scope plugin root ({}) and the project-scope one \
                 ({})",
                user_dir.display(),
                project_dir.display(),
            )
        },
        |reason| format!("it is not live: {reason}"),
    );
    DoctorFinding {
        kind: FindingKind::GooseSessionGuardMissing,
        message: format!(
            "goose is installed but the remargin guard plugin declares no live SessionStart entry \
             (`remargin goose session-guard`) — {detail}. goose fails open on a hook it cannot \
             run, so a PreToolUse guard that breaks leaves the session unguarded with no signal; \
             the SessionStart entry is the backstop that reports it.",
        ),
        remedy: String::from(
            "Run `remargin goose session-guard install` to register the SessionStart entry.",
        ),
        severity: Severity::Critical,
    }
}

/// Every selected goose check's findings. A `None` probe is a machine with
/// no goose installation, which has nothing to report — not a failure.
fn goose_findings(goose: Option<&GooseProbe>, checks: &HashSet<CheckName>) -> Vec<DoctorFinding> {
    let Some(probe) = goose else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    if checks.contains(&CheckName::GooseGuard) {
        findings.extend(probe.guard_findings());
    }
    if checks.contains(&CheckName::GooseMcp) {
        findings.extend(probe.mcp_findings());
    }
    if checks.contains(&CheckName::GooseSessionGuard) {
        findings.extend(probe.session_guard_findings());
    }
    findings
}

fn goose_mcp_missing_finding(
    user_file: &Path,
    local_file: &Path,
    fault: Option<&str>,
) -> DoctorFinding {
    let detail = fault.map_or_else(
        || {
            format!(
                "no entry is registered in the user-scope config ({}) or the project one ({})",
                user_file.display(),
                local_file.display(),
            )
        },
        |reason| format!("its entry is not live: {reason}"),
    );
    DoctorFinding {
        kind: FindingKind::GooseMcpMissing,
        message: format!(
            "The goose guard is wired but remargin is not registered as a goose MCP extension — \
             {detail}. The guard blocks native tools on managed paths and tells the agent to use \
             the remargin ops instead, so without the extension every block names tools the \
             session does not have: a dead end rather than a redirect.",
        ),
        remedy: String::from(
            "Run `remargin goose mcp install` to register the extension, then start a new goose \
             session.",
        ),
        severity: Severity::Critical,
    }
}

fn goose_guard_missing_finding(user_dir: &Path, project_dir: &Path) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::GooseGuardMissing,
        message: format!(
            "goose is installed but the remargin guard plugin is absent from both the user-scope \
             plugin root ({}) and the project-scope one ({}). A goose session can shell into and \
             edit remargin-managed paths with no guard at all.",
            user_dir.display(),
            project_dir.display(),
        ),
        remedy: String::from("Run `remargin goose pretool install` to install the guard plugin."),
        severity: Severity::Critical,
    }
}

fn goose_guard_broken_finding(reason: &str) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::GooseGuardBroken,
        message: format!(
            "The remargin guard plugin for goose is installed but does not describe a live hook: \
             {reason}. goose fails open on a hook it cannot run, so the session is unguarded with \
             no signal."
        ),
        remedy: String::from(
            "Run `remargin goose pretool install` to rewrite the guard plugin in place.",
        ),
        severity: Severity::Critical,
    }
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

/// One [`FindingKind::TrustedRootMissing`] per resolved trusted root whose
/// anchor does not exist on disk.
///
/// Reuses the resolver the escape and leftover checks already walk, so the
/// resolved anchor set has a single source of truth. Runs only behind the
/// escape gate (see the caller), so every anchor here is already inside its
/// realm — an absent anchor is a moved or deleted target, never a
/// containment escape. A wildcard root anchors at the declaring realm's own
/// directory, which exists by construction, so it never fires; only an
/// explicit path pointing at a vanished target does.
fn trusted_root_missing_findings(system: &dyn System, cwd: &Path) -> Result<Vec<DoctorFinding>> {
    let resolved = resolve_permissions(system, cwd)?;
    let mut findings = Vec::new();
    for root in &resolved.trusted_roots {
        let anchor = trusted_root_anchor(root);
        if !system.exists(anchor)? {
            findings.push(trusted_root_missing_finding(root, anchor));
        }
    }
    Ok(findings)
}

fn trusted_root_missing_finding(root: &ResolvedTrustedRoot, anchor: &Path) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::TrustedRootMissing,
        message: format!(
            "trusted_roots entry in {} resolves to {}, which does not exist. It protects nothing.",
            root.source_file.display(),
            anchor.display(),
        ),
        remedy: format!(
            "Point the entry at an existing path, or drop it from {}.",
            root.source_file.display(),
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

/// One [`FindingKind::StaleSandboxEntry`] per realm sandbox entry whose
/// author is not an active registry participant.
///
/// A realm with no registry has no notion of a "live identity" backing an
/// entry, so it yields nothing. Otherwise every `author@timestamp` across
/// all identities is scanned; an author that is absent or revoked in the
/// resolved registry is stale staging that never clears on its own. The
/// scan reuses the sandbox walk's visibility/parse filtering and skips
/// unreadable or unparseable files without aborting.
fn stale_sandbox_findings(system: &dyn System, cwd: &Path) -> Result<Vec<DoctorFinding>> {
    let config = ResolvedConfig::resolve(system, cwd, &IdentityFlags::default(), None)?;
    let Some(registry) = config.registry.as_ref() else {
        return Ok(Vec::new());
    };

    let mut findings = Vec::new();
    for entry in sandbox::scan_all_entries(system, cwd)? {
        if !registry.is_active(&entry.author) {
            findings.push(stale_sandbox_finding(&entry.path, &entry.author));
        }
    }
    Ok(findings)
}

fn stale_sandbox_finding(path: &Path, author: &str) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::StaleSandboxEntry,
        message: format!(
            "`{}` carries a sandbox entry for `{author}`, who is not an active registry \
             participant. The stale staging never clears on its own.",
            path.display(),
        ),
        remedy: format!(
            "Re-stage as a live identity, or remove the stale `sandbox:` entry for `{author}` \
             from {}.",
            path.display(),
        ),
        severity: Severity::Warning,
    }
}

/// Render a [`DoctorReport`] as human-readable text.
///
/// When `verbose` is `true`, a `Checks:` section is appended after the
/// findings block (or after the clean message) listing one verdict per
/// check and the paths of both settings files that were inspected. This
/// section appears in both the clean and findings cases. The goose lines
/// are rendered only when the report carries a goose verdict, so a machine
/// without goose says nothing about it.
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
        let _ = writeln!(out, "Checks:");
        let _ = writeln!(
            out,
            "  hook-installed: {}",
            check_verdict(report.hook_installed)
        );
        let _ = writeln!(
            out,
            "  session-guard: {}",
            check_verdict(report.session_guard_installed)
        );
        if let Some(installed) = report.goose_guard_installed {
            let _ = writeln!(out, "  goose-guard: {}", check_verdict(installed));
        }
        if let Some(installed) = report.goose_mcp_installed {
            let _ = writeln!(out, "  goose-mcp: {}", check_verdict(installed));
        }
        if let Some(installed) = report.goose_session_guard_installed {
            let _ = writeln!(out, "  goose-session-guard: {}", check_verdict(installed));
        }
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

/// The verbose `Checks:` wording for one verdict, shared by every line so
/// the vocabulary cannot drift between checks.
const fn check_verdict(installed: bool) -> &'static str {
    if installed { "ok" } else { "missing" }
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
