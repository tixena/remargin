//! Unit tests for [`crate::permissions::doctor`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use os_shim::System;
use os_shim::mock::MemorySystem;
use serde_json::json;

use crate::permissions::doctor::{
    CheckName, DoctorFinding, DoctorReport, FindingKind, Severity, render_doctor_prompt,
    render_doctor_text,
};
use crate::permissions::pretool_install::{HOOK_MATCHER, HOOK_SUBCOMMAND};
use crate::permissions::session_guard_install::SESSION_HOOK_SUBCOMMAND;

/// The binary both hook commands name; present in every mock, so seeded entries read as live.
const EXE: &str = "/usr/local/bin/remargin";

/// The `PreToolUse` command a current install writes.
fn hook_command() -> String {
    format!("{EXE} {HOOK_SUBCOMMAND}")
}

/// The `SessionStart` command a current install writes.
fn guard_command() -> String {
    format!("{EXE} {SESSION_HOOK_SUBCOMMAND}")
}

/// [`run_doctor`](super::run_doctor) with every check selected.
fn run_doctor(
    system: &dyn System,
    cwd: &Path,
    user_settings_file: &Path,
) -> anyhow::Result<DoctorReport> {
    super::run_doctor(system, cwd, user_settings_file, &CheckName::all())
}

/// Settings carrying both enforcement hooks — the fully-configured, clean
/// state (`PreToolUse` enforcement + `SessionStart` guard).
fn hook_settings_json() -> String {
    let v = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": hook_command() }
                    ]
                }
            ],
            "SessionStart": [
                {
                    "hooks": [
                        { "type": "command", "command": guard_command() }
                    ]
                }
            ]
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

/// Settings carrying both hooks, each running exactly the command given —
/// the fixture for entries an older install wrote, or whose binary moved.
fn settings_json_with_commands(pretool: &str, guard: &str) -> String {
    let v = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": pretool }
                    ]
                }
            ],
            "SessionStart": [
                {
                    "hooks": [
                        { "type": "command", "command": guard }
                    ]
                }
            ]
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

/// Settings carrying only the `PreToolUse` hook — enforcement is wired but
/// the `SessionStart` guard is missing.
fn pretool_only_settings_json() -> String {
    let v = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": hook_command() }
                    ]
                }
            ]
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

/// Settings carrying only the `SessionStart` guard hook.
fn guard_only_settings_json() -> String {
    let v = json!({
        "hooks": {
            "SessionStart": [
                {
                    "hooks": [
                        { "type": "command", "command": guard_command() }
                    ]
                }
            ]
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

/// Settings carrying only a `permissions.deny` array — used to seed a
/// project-scope file with leftover drift while the enforcement hooks
/// live in the user-scope file.
fn deny_only_settings_json(deny: &[&str]) -> String {
    let v = json!({ "permissions": { "deny": deny } });
    serde_json::to_string_pretty(&v).unwrap()
}

fn mock_with_file(path: &str, body: &str) -> MemorySystem {
    mock_with_files(&[(path, body)])
}

fn mock_with_files(files: &[(&str, &str)]) -> MemorySystem {
    let mut system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap()
        .with_file(Path::new(EXE), b"binary")
        .unwrap();
    for (path, body) in files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    system
}

#[test]
fn hook_in_user_scope_is_clean() {
    let system = mock_with_file("/home/u/.claude/settings.json", &hook_settings_json());
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(report.hook_installed, "expected hook_installed=true");
    assert!(report.is_clean(), "expected no findings: {report:#?}");
    assert_eq!(report.findings, [] as [DoctorFinding; 0]);
}

/// Project scope is `.claude/settings.json`, the file `install --local` writes.
#[test]
fn hook_in_project_scope_is_clean() {
    let system = mock_with_file("/r/.claude/settings.json", &hook_settings_json());
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(report.hook_installed, "expected hook_installed=true");
    assert!(report.is_clean());
}

#[test]
fn hook_absent_from_both_scopes_reports_hook_missing() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap();
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(!report.hook_installed);
    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!(finding.kind, FindingKind::HookMissing);
    assert_eq!(finding.severity, Severity::Critical);
    assert!(
        finding.message.contains("PreToolUse"),
        "message should mention PreToolUse: {}",
        finding.message
    );
    assert!(
        finding.remedy.contains("pretool install"),
        "remedy should mention pretool install: {}",
        finding.remedy
    );
}

/// A command that cannot spawn fails open like a missing entry: the gate fails, naming the fault.
#[test]
fn stale_hook_binary_fails_the_gate_and_names_the_fault() {
    let settings = settings_json_with_commands(
        &format!("/gone/remargin {HOOK_SUBCOMMAND}"),
        &guard_command(),
    );
    let system = mock_with_file("/home/u/.claude/settings.json", &settings);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();

    assert!(!report.hook_installed, "stale binary is not enforcement");
    assert_eq!(kinds(&report), vec![FindingKind::HookMissing]);
    let finding = &report.findings[0];
    assert_eq!(finding.severity, Severity::Critical);
    assert!(
        finding.message.contains("/gone/remargin")
            && finding.message.contains("does not exist")
            && finding.message.contains("/home/u/.claude/settings.json"),
        "message should name the vanished binary and the file: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("pretool install"),
        "remedy should name the reinstall: {}",
        finding.remedy,
    );
}

/// The entry still enforces while `PATH` resolves it, so the gate passes with a warning.
#[test]
fn path_relative_hook_entry_passes_the_gate_with_a_warning() {
    let legacy = format!("remargin {HOOK_SUBCOMMAND}");
    let settings = settings_json_with_commands(&legacy, &guard_command());
    let system = mock_with_file("/home/u/.claude/settings.json", &settings);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();

    assert!(report.hook_installed, "a PATH-resolved entry still gates");
    assert_eq!(kinds(&report), vec![FindingKind::HookPathRelative]);
    let finding = &report.findings[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding.message.contains(&legacy)
            && finding.message.contains("/home/u/.claude/settings.json"),
        "message should name the command and the file: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("pretool install"),
        "remedy should name the reinstall: {}",
        finding.remedy,
    );
}

/// A `PATH`-relative guard entry is registered, so it warns instead of reporting missing.
#[test]
fn path_relative_session_guard_entry_warns_instead_of_missing() {
    let legacy = format!("remargin {SESSION_HOOK_SUBCOMMAND}");
    let settings = settings_json_with_commands(&hook_command(), &legacy);
    let system = mock_with_file("/home/u/.claude/settings.json", &settings);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();

    assert!(report.session_guard_installed);
    assert_eq!(kinds(&report), vec![FindingKind::HookPathRelative]);
    assert!(
        report.findings[0].remedy.contains("session-guard install"),
        "remedy should name the guard's install: {}",
        report.findings[0].remedy,
    );
}

#[test]
fn stale_session_guard_binary_reports_missing_with_the_fault() {
    let settings = settings_json_with_commands(
        &hook_command(),
        &format!("/gone/remargin {SESSION_HOOK_SUBCOMMAND}"),
    );
    let system = mock_with_file("/home/u/.claude/settings.json", &settings);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();

    assert!(report.hook_installed, "the PreToolUse entry is untouched");
    assert!(!report.session_guard_installed);
    assert_eq!(kinds(&report), vec![FindingKind::SessionGuardMissing]);
    let finding = &report.findings[0];
    assert!(
        finding.message.contains("/gone/remargin"),
        "message should name the vanished binary: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("session-guard install"),
        "remedy should name the reinstall: {}",
        finding.remedy,
    );
}

/// The warning belongs to the check that owns the entry.
#[test]
fn path_relative_findings_follow_their_check_selection() {
    let settings = settings_json_with_commands(
        &format!("remargin {HOOK_SUBCOMMAND}"),
        &format!("remargin {SESSION_HOOK_SUBCOMMAND}"),
    );
    let system = mock_with_file("/home/u/.claude/settings.json", &settings);
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &CheckName::parse_set("hook").unwrap(),
    )
    .unwrap();

    assert_eq!(kinds(&report), vec![FindingKind::HookPathRelative]);
    assert!(
        report.findings[0].remedy.contains("pretool install"),
        "only the PreToolUse entry's warning survives: {report:#?}",
    );
}

#[test]
fn hook_missing_finding_names_both_files() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap();
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    let finding = &report.findings[0];
    assert!(
        finding.message.contains("/home/u/.claude/settings.json"),
        "message should name user-scope file: {}",
        finding.message
    );
    assert!(
        finding.message.contains("/r/.claude/settings.json"),
        "message should name project-scope file: {}",
        finding.message
    );
}

#[test]
fn hook_missing_is_first_finding() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap();
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert_ne!(report.findings, [] as [DoctorFinding; 0]);
    assert_eq!(report.findings[0].kind, FindingKind::HookMissing);
}

#[test]
fn doctor_report_json_round_trip() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap();
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    let json = serde_json::to_string(&report).unwrap();
    let parsed: DoctorReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, parsed);
}

#[test]
fn report_includes_correct_settings_file_paths() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap();
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert_eq!(
        report.project_settings_file,
        PathBuf::from("/r/.claude/settings.json")
    );
    assert_eq!(
        report.user_settings_file,
        PathBuf::from("/home/u/.claude/settings.json")
    );
}

#[test]
fn guard_in_user_scope_is_clean() {
    let system = mock_with_file("/home/u/.claude/settings.json", &hook_settings_json());
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(report.hook_installed);
    assert!(report.session_guard_installed, "expected guard installed");
    assert!(report.is_clean(), "expected no findings: {report:#?}");
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.kind != FindingKind::SessionGuardMissing),
        "no SessionGuardMissing expected: {report:#?}",
    );
}

#[test]
fn guard_in_project_scope_only_is_clean() {
    let system = mock_with_files(&[
        (
            "/home/u/.claude/settings.json",
            &pretool_only_settings_json(),
        ),
        ("/r/.claude/settings.json", &guard_only_settings_json()),
    ]);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(report.hook_installed);
    assert!(report.session_guard_installed);
    assert!(report.is_clean(), "expected no findings: {report:#?}");
}

#[test]
fn guard_absent_from_both_scopes_reports_session_guard_missing() {
    let system = mock_with_file(
        "/home/u/.claude/settings.json",
        &pretool_only_settings_json(),
    );
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(report.hook_installed, "PreToolUse hook should be detected");
    assert!(!report.session_guard_installed);
    assert_eq!(
        report.findings.len(),
        1,
        "expected one finding: {report:#?}"
    );
    let finding = &report.findings[0];
    assert_eq!(finding.kind, FindingKind::SessionGuardMissing);
    assert_eq!(finding.severity, Severity::Critical);
    assert!(
        finding.message.contains("SessionStart"),
        "message should mention SessionStart: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("session-guard install"),
        "remedy should name the install command: {}",
        finding.remedy,
    );
}

fn clean_report() -> DoctorReport {
    DoctorReport {
        elapsed_ms: None,
        findings: vec![],
        goose_guard_installed: None,
        goose_mcp_installed: None,
        goose_session_guard_installed: None,
        hook_installed: true,
        session_guard_installed: true,
        project_settings_file: PathBuf::from("/r/.claude/settings.local.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    }
}

fn findings_report() -> DoctorReport {
    DoctorReport {
        elapsed_ms: None,
        findings: vec![DoctorFinding {
            kind: FindingKind::HookMissing,
            message: String::from("hook is missing"),
            remedy: String::from("run install"),
            severity: Severity::Critical,
        }],
        goose_guard_installed: None,
        goose_mcp_installed: None,
        goose_session_guard_installed: None,
        hook_installed: false,
        session_guard_installed: false,
        project_settings_file: PathBuf::from("/r/.claude/settings.local.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    }
}

#[test]
fn render_doctor_clean_plain() {
    let out = render_doctor_text(&clean_report(), false);
    assert!(out.contains("all checks passed"), "unexpected: {out}");
    assert!(!out.contains("Checks:"), "verbose section in plain: {out}");
}

#[test]
fn render_doctor_clean_verbose() {
    let out = render_doctor_text(&clean_report(), true);
    assert!(out.contains("all checks passed"), "unexpected: {out}");
    assert!(out.contains("Checks:"), "missing Checks: in verbose: {out}");
    assert!(
        out.contains("hook-installed: ok"),
        "missing hook verdict: {out}"
    );
    assert!(
        out.contains("session-guard: ok"),
        "missing session-guard verdict: {out}"
    );
    assert!(
        out.contains("user-settings:"),
        "missing user-settings: {out}"
    );
    assert!(
        out.contains("project-settings:"),
        "missing project-settings: {out}"
    );
}

#[test]
fn render_doctor_findings_plain() {
    let out = render_doctor_text(&findings_report(), false);
    assert!(out.contains("[CRITICAL]"), "unexpected: {out}");
    assert!(out.contains("hook is missing"), "unexpected: {out}");
    assert!(out.contains("Remedy: run install"), "unexpected: {out}");
    assert!(!out.contains("Checks:"), "verbose section in plain: {out}");
}

#[test]
fn render_doctor_findings_verbose() {
    let out = render_doctor_text(&findings_report(), true);
    assert!(out.contains("[CRITICAL]"), "unexpected: {out}");
    assert!(out.contains("Checks:"), "missing Checks: in verbose: {out}");
    assert!(
        out.contains("hook-installed: missing"),
        "expected missing verdict: {out}"
    );
}

fn leftover_findings(report: &DoctorReport) -> Vec<&DoctorFinding> {
    report
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::LeftoverProjectedRule)
        .collect()
}

/// One warning naming the file, the rule and a removal remedy.
#[test]
fn leftover_flags_stale_remargin_cli_deny() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        (
            "/r/.claude/settings.local.json",
            &deny_only_settings_json(&["Bash(remargin *)"]),
        ),
    ]);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    let leftovers = leftover_findings(&report);
    assert_eq!(leftovers.len(), 1, "expected one leftover: {report:#?}");
    let finding = leftovers[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding.message.contains("Bash(remargin *)"),
        "message should name the rule: {}",
        finding.message,
    );
    assert!(
        finding.message.contains("/r/.claude/settings.local.json"),
        "message should name the file: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("Remove the deny rule")
            && finding.remedy.contains("Bash(remargin *)"),
        "remedy should name the removal + rule: {}",
        finding.remedy,
    );
}

#[test]
fn leftover_flags_projected_path_deny() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: \"*\"\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
        (
            "/r/.claude/settings.local.json",
            &deny_only_settings_json(&["Edit(/r/**)"]),
        ),
    ]);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    let leftovers = leftover_findings(&report);
    assert_eq!(leftovers.len(), 1, "expected one leftover: {report:#?}");
    assert!(
        leftovers[0].message.contains("Edit(/r/**)"),
        "message should name the projected rule: {}",
        leftovers[0].message,
    );
}

#[test]
fn leftover_clean_when_no_projected_or_stale_denies() {
    let system = mock_with_file("/home/u/.claude/settings.json", &hook_settings_json());
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(
        leftover_findings(&report).is_empty(),
        "no leftover expected: {report:#?}",
    );
    assert!(report.is_clean(), "expected clean report: {report:#?}");
}

/// Doctor names the entry and the resolved anchor instead of crashing on the resolve error.
#[test]
fn out_of_realm_trusted_root_emits_finding_without_crashing() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: /other/secret\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
    ]);
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    let escapes: Vec<&DoctorFinding> = report
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::TrustedRootEscape)
        .collect();
    assert_eq!(escapes.len(), 1, "expected one escape finding: {report:#?}");
    assert!(
        escapes[0].message.contains("/other/secret")
            && escapes[0].message.contains("/r/.remargin.yaml"),
        "message names entry and file: {}",
        escapes[0].message,
    );
    assert!(
        escapes[0].remedy.contains("restrict") || escapes[0].remedy.contains("Move"),
        "remedy offers a fix: {}",
        escapes[0].remedy,
    );
}

fn leftover_finding_fixture(rule: &str, file: &str) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::LeftoverProjectedRule,
        message: format!("The deny rule `{rule}` in {file} is drift."),
        remedy: format!("Remove the deny rule `{rule}` from the permissions.deny array in {file}."),
        severity: Severity::Warning,
    }
}

/// One imperative instruction per finding, naming its rule and its file.
#[test]
fn render_prompt_names_each_finding_rule_and_file() {
    let report = DoctorReport {
        elapsed_ms: None,
        findings: vec![
            leftover_finding_fixture("Bash(remargin *)", "/r/.claude/settings.local.json"),
            leftover_finding_fixture("Edit(/r/**)", "/home/u/.claude/settings.json"),
        ],
        goose_guard_installed: None,
        goose_mcp_installed: None,
        goose_session_guard_installed: None,
        hook_installed: true,
        session_guard_installed: true,
        project_settings_file: PathBuf::from("/r/.claude/settings.local.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    };
    let out = render_doctor_prompt(&report);
    assert!(out.contains("1."), "expected numbered instruction: {out}");
    assert!(out.contains("2."), "expected numbered instruction: {out}");
    assert!(
        out.contains("Bash(remargin *)") && out.contains("Edit(/r/**)"),
        "prompt must name both rules: {out}",
    );
    assert!(
        out.contains("/r/.claude/settings.local.json")
            && out.contains("/home/u/.claude/settings.json"),
        "prompt must name both files: {out}",
    );
}

#[test]
fn render_prompt_clean_says_nothing_to_do() {
    let out = render_doctor_prompt(&clean_report());
    assert!(
        out.to_lowercase().contains("nothing to do"),
        "expected nothing-to-do prompt: {out}",
    );
    assert!(
        !out.contains("1."),
        "clean prompt must list no steps: {out}"
    );
}

fn strict_agent_registry() -> &'static str {
    "participants:\n  agent1:\n    type: agent\n    status: active\n"
}

/// Hook installed in user-scope and `HOME` set, so `~/.ssh` derivation and
/// plain-name `key:` resolution behave as they do in a real run. Extra
/// realm files (`.remargin.yaml`, registry, key files) are layered on top.
fn identity_mock(files: &[(&str, &str)]) -> MemorySystem {
    let mut system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap()
        .with_env("HOME", "/home/u")
        .unwrap()
        .with_file(Path::new(EXE), b"binary")
        .unwrap()
        .with_file(
            Path::new("/home/u/.claude/settings.json"),
            hook_settings_json().as_bytes(),
        )
        .unwrap();
    for (path, body) in files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    system
}

fn findings_of_kind<'report>(
    report: &'report DoctorReport,
    kind: &FindingKind,
) -> Vec<&'report DoctorFinding> {
    report.findings.iter().filter(|f| &f.kind == kind).collect()
}

fn strict_agent_yaml(key: &str) -> String {
    format!("mode: strict\ntype: agent\nidentity: agent1\nkey: {key}\n")
}

fn run_at_r(system: &MemorySystem) -> DoctorReport {
    run_doctor(
        system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap()
}

#[test]
fn strict_missing_key_reports_identity_key_unresolvable() {
    let system = identity_mock(&[
        ("/r/.remargin.yaml", &strict_agent_yaml("/r/keys/agent")),
        ("/r/.remargin-registry.yaml", strict_agent_registry()),
    ]);
    let report = run_at_r(&system);
    let found = findings_of_kind(&report, &FindingKind::IdentityKeyUnresolvable);
    assert_eq!(found.len(), 1, "expected one finding: {report:#?}");
    let finding = found[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding.message.contains("agent1")
            && finding.message.contains("/r/keys/agent")
            && finding.message.contains("/r/.remargin.yaml"),
        "message names identity, key, and config: {}",
        finding.message,
    );
    assert!(
        findings_of_kind(&report, &FindingKind::AgentKeyUnderUserSsh).is_empty(),
        "key is not under ~/.ssh: {report:#?}",
    );
}

/// A `key:` that exists but is a directory still fails: the probe checks readability.
#[test]
fn strict_present_but_unreadable_key_reports_identity_key_unresolvable() {
    let system = identity_mock(&[
        ("/r/.remargin.yaml", &strict_agent_yaml("/r/keys/agentdir")),
        ("/r/.remargin-registry.yaml", strict_agent_registry()),
    ])
    .with_dir(Path::new("/r/keys/agentdir"))
    .unwrap();
    let report = run_at_r(&system);
    assert_eq!(
        findings_of_kind(&report, &FindingKind::IdentityKeyUnresolvable).len(),
        1,
        "present-but-unreadable key must still flag: {report:#?}",
    );
}

#[test]
fn strict_readable_key_has_no_finding() {
    let system = identity_mock(&[
        ("/r/.remargin.yaml", &strict_agent_yaml("/r/keys/agent")),
        ("/r/.remargin-registry.yaml", strict_agent_registry()),
        ("/r/keys/agent", "PRIVATE KEY"),
    ]);
    let report = run_at_r(&system);
    assert!(
        findings_of_kind(&report, &FindingKind::IdentityKeyUnresolvable).is_empty()
            && findings_of_kind(&report, &FindingKind::AgentKeyUnderUserSsh).is_empty(),
        "readable key in-realm is clean: {report:#?}",
    );
}

#[test]
fn open_mode_missing_key_has_no_finding() {
    let yaml = "mode: open\ntype: agent\nidentity: agent1\nkey: /r/keys/missing\n";
    let system = identity_mock(&[("/r/.remargin.yaml", yaml)]);
    let report = run_at_r(&system);
    assert!(
        findings_of_kind(&report, &FindingKind::IdentityKeyUnresolvable).is_empty(),
        "strict-only check must not fire in open mode: {report:#?}",
    );
}

#[test]
fn agent_key_under_user_ssh_reports_finding() {
    let yaml = "mode: open\ntype: agent\nidentity: agent1\nkey: id_ed25519\n";
    let system = identity_mock(&[
        ("/r/.remargin.yaml", yaml),
        ("/home/u/.ssh/id_ed25519", "PRIVATE KEY"),
    ]);
    let report = run_at_r(&system);
    let found = findings_of_kind(&report, &FindingKind::AgentKeyUnderUserSsh);
    assert_eq!(found.len(), 1, "expected one finding: {report:#?}");
    let finding = found[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding.message.contains("agent1") && finding.message.contains("/home/u/.ssh/id_ed25519"),
        "message names identity and key: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("~/.ssh"),
        "remedy points out of ~/.ssh: {}",
        finding.remedy,
    );
    assert!(
        findings_of_kind(&report, &FindingKind::IdentityKeyUnresolvable).is_empty(),
        "open mode: no strict readability finding: {report:#?}",
    );
}

/// `~/.ssh` is the expected home for a human key.
#[test]
fn human_key_under_user_ssh_has_no_finding() {
    let yaml = "mode: open\ntype: human\nidentity: human1\nkey: id_ed25519\n";
    let system = identity_mock(&[
        ("/r/.remargin.yaml", yaml),
        ("/home/u/.ssh/id_ed25519", "PRIVATE KEY"),
    ]);
    let report = run_at_r(&system);
    assert!(
        findings_of_kind(&report, &FindingKind::AgentKeyUnderUserSsh).is_empty(),
        "~/.ssh is the expected home for a human key: {report:#?}",
    );
}

#[test]
fn hook_missing_skips_identity_key_check() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap()
        .with_env("HOME", "/home/u")
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            strict_agent_yaml("/r/keys/agent").as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/r/.remargin-registry.yaml"),
            strict_agent_registry().as_bytes(),
        )
        .unwrap();
    let report = run_at_r(&system);
    assert!(!report.hook_installed);
    assert_eq!(report.findings.len(), 1, "only HookMissing: {report:#?}");
    assert_eq!(report.findings[0].kind, FindingKind::HookMissing);
    assert!(
        findings_of_kind(&report, &FindingKind::IdentityKeyUnresolvable).is_empty(),
        "identity check must be skipped when the hook is missing: {report:#?}",
    );
}

#[test]
fn identity_findings_render_and_serialize() {
    let report = DoctorReport {
        elapsed_ms: None,
        findings: vec![
            DoctorFinding {
                kind: FindingKind::IdentityKeyUnresolvable,
                message: String::from("agent1 signing key is not a readable file"),
                remedy: String::from("Fix the key: path in /r/.remargin.yaml"),
                severity: Severity::Warning,
            },
            DoctorFinding {
                kind: FindingKind::AgentKeyUnderUserSsh,
                message: String::from("agent1 key lives under ~/.ssh"),
                remedy: String::from("Move the agent's key out of ~/.ssh"),
                severity: Severity::Warning,
            },
        ],
        goose_guard_installed: None,
        goose_mcp_installed: None,
        goose_session_guard_installed: None,
        hook_installed: true,
        session_guard_installed: true,
        project_settings_file: PathBuf::from("/r/.claude/settings.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    };

    let json = serde_json::to_string(&report).unwrap();
    assert!(
        json.contains("identity_key_unresolvable") && json.contains("agent_key_under_user_ssh"),
        "wire names present: {json}",
    );
    let parsed: DoctorReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, parsed);

    let text = render_doctor_text(&report, false);
    assert_eq!(
        text.matches("[WARNING]").count(),
        2,
        "both findings labelled WARNING: {text}",
    );

    let prompt = render_doctor_prompt(&report);
    assert!(
        prompt.contains("1.") && prompt.contains("2."),
        "prompt: {prompt}"
    );
    assert!(
        prompt.contains("Fix the key: path in /r/.remargin.yaml")
            && prompt.contains("Move the agent's key out of ~/.ssh"),
        "prompt names each remedy: {prompt}",
    );
}

fn schema_lint_findings(report: &DoctorReport) -> Vec<&DoctorFinding> {
    report
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::ConfigSchemaLint)
        .collect()
}

#[test]
fn schema_lint_flags_yaml_syntax_error() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", "permissions:\n  deny_ops: [oops\n"),
    ]);
    let report = run_at_r(&system);
    let lints = schema_lint_findings(&report);
    assert_eq!(lints.len(), 1, "expected one schema lint: {report:#?}");
    let finding = lints[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding.message.contains("/r/.remargin.yaml"),
        "message names the file: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("Fix the permissions schema")
            && finding.remedy.contains("/r/.remargin.yaml"),
        "remedy names the fix and file: {}",
        finding.remedy,
    );
}

#[test]
fn schema_lint_flags_unknown_permissions_key() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", "permissions:\n  deny_op: []\n"),
    ]);
    let report = run_at_r(&system);
    let lints = schema_lint_findings(&report);
    assert_eq!(lints.len(), 1, "expected one schema lint: {report:#?}");
    assert!(
        lints[0].message.contains("unknown field") && lints[0].message.contains("deny_op"),
        "message carries the parser diagnostic: {}",
        lints[0].message,
    );
}

/// The `to:` field also fails serde, so a parse-error lint accompanies the migration hint.
#[test]
fn schema_lint_flags_legacy_to_field() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        (
            "/r/.remargin.yaml",
            "permissions:\n  deny_ops:\n    - path: .\n      ops: [purge]\n      to: [eduardo-burgos]\n",
        ),
    ]);
    let report = run_at_r(&system);
    let lints = schema_lint_findings(&report);
    assert!(
        lints
            .iter()
            .any(|f| f.message.contains("legacy `to:`") && f.message.contains("exceptions")),
        "expected the migration-recipe schema lint: {report:#?}",
    );
}

/// The escape is reported once, by the dedicated check, not again as a schema lint.
#[test]
fn schema_lint_does_not_duplicate_trusted_root_escape() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: /other/secret\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
    ]);
    let report = run_at_r(&system);
    assert_eq!(
        findings_of_kind(&report, &FindingKind::TrustedRootEscape).len(),
        1,
        "exactly one escape finding: {report:#?}",
    );
    assert!(
        schema_lint_findings(&report).is_empty(),
        "escape must not be duplicated as a schema lint: {report:#?}",
    );
}

#[test]
fn schema_lint_clean_tree_has_no_findings() {
    let yaml = "permissions:\n  deny_ops:\n    - path: src/secret\n      ops: [purge, delete]\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
    ]);
    let report = run_at_r(&system);
    assert!(
        schema_lint_findings(&report).is_empty(),
        "valid config yields no schema lint: {report:#?}",
    );
}

#[test]
fn config_schema_lint_serializes_and_renders() {
    let report = DoctorReport {
        elapsed_ms: None,
        findings: vec![DoctorFinding {
            kind: FindingKind::ConfigSchemaLint,
            message: String::from("/r/.remargin.yaml (line 2, col 3): unknown field `deny_op`"),
            remedy: String::from("Fix the permissions schema in /r/.remargin.yaml."),
            severity: Severity::Warning,
        }],
        goose_guard_installed: None,
        goose_mcp_installed: None,
        goose_session_guard_installed: None,
        hook_installed: true,
        session_guard_installed: true,
        project_settings_file: PathBuf::from("/r/.claude/settings.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    };
    let json = serde_json::to_string(&report).unwrap();
    assert!(
        json.contains("config_schema_lint"),
        "wire name present: {json}",
    );
    let parsed: DoctorReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, parsed);

    let text = render_doctor_text(&report, false);
    assert!(
        text.contains("[WARNING]") && text.contains("unknown field"),
        "text renders the schema lint as WARNING: {text}",
    );

    let prompt = render_doctor_prompt(&report);
    assert!(
        prompt.contains("Fix the permissions schema"),
        "prompt names the remedy: {prompt}",
    );
}

/// A registry with one active human and one revoked agent. Any sandbox
/// author outside this active set (absent or revoked) is stale.
fn sandbox_registry() -> &'static str {
    "participants:\n  \
     eduardo-burgos:\n    type: human\n    status: active\n  \
     retired-agent:\n    type: agent\n    status: revoked\n"
}

/// A markdown document carrying a single `sandbox:` entry for `entry`
/// (an `author@timestamp` string).
fn sandbox_doc(entry: &str) -> String {
    format!("---\ntitle: Roster\nsandbox:\n- {entry}\n---\n\n# Roster\n\nBody.\n")
}

#[test]
fn stale_sandbox_flags_orphaned_author() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin-registry.yaml", sandbox_registry()),
        (
            "/r/notes/roster.md",
            &sandbox_doc("ghost_agent@2026-01-01T00:00:00+00:00"),
        ),
    ]);
    let report = run_at_r(&system);
    let stale = findings_of_kind(&report, &FindingKind::StaleSandboxEntry);
    assert_eq!(stale.len(), 1, "expected one stale finding: {report:#?}");
    let finding = stale[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding.message.contains("ghost_agent") && finding.message.contains("/r/notes/roster.md"),
        "message names author and file: {}",
        finding.message,
    );
    assert!(
        finding.remedy.contains("sandbox:") && finding.remedy.contains("/r/notes/roster.md"),
        "remedy names removal and file: {}",
        finding.remedy,
    );
}

#[test]
fn stale_sandbox_active_author_is_clean() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin-registry.yaml", sandbox_registry()),
        (
            "/r/roster.md",
            &sandbox_doc("eduardo-burgos@2026-01-01T00:00:00+00:00"),
        ),
    ]);
    let report = run_at_r(&system);
    assert!(
        findings_of_kind(&report, &FindingKind::StaleSandboxEntry).is_empty(),
        "active author must not be flagged: {report:#?}",
    );
    assert!(report.is_clean(), "expected clean report: {report:#?}");
}

#[test]
fn stale_sandbox_revoked_participant_is_flagged() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin-registry.yaml", sandbox_registry()),
        (
            "/r/roster.md",
            &sandbox_doc("retired-agent@2026-01-01T00:00:00+00:00"),
        ),
    ]);
    let report = run_at_r(&system);
    let stale = findings_of_kind(&report, &FindingKind::StaleSandboxEntry);
    assert_eq!(
        stale.len(),
        1,
        "revoked author must be flagged: {report:#?}"
    );
    assert!(
        stale[0].message.contains("retired-agent"),
        "message names the revoked author: {}",
        stale[0].message,
    );
}

#[test]
fn stale_sandbox_no_registry_has_no_findings() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        (
            "/r/roster.md",
            &sandbox_doc("ghost_agent@2026-01-01T00:00:00+00:00"),
        ),
    ]);
    let report = run_at_r(&system);
    assert!(
        findings_of_kind(&report, &FindingKind::StaleSandboxEntry).is_empty(),
        "no registry means no stale findings: {report:#?}",
    );
    assert!(report.is_clean(), "expected clean report: {report:#?}");
}

/// A non-markdown file with sandbox-looking text is skipped and the walk continues.
#[test]
fn stale_sandbox_mixed_files_flags_only_the_stale_one() {
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin-registry.yaml", sandbox_registry()),
        (
            "/r/a.md",
            &sandbox_doc("ghost_agent@2026-01-01T00:00:00+00:00"),
        ),
        (
            "/r/b.md",
            &sandbox_doc("eduardo-burgos@2026-01-01T00:00:00+00:00"),
        ),
        (
            "/r/junk.txt",
            "sandbox: ghost_agent@2026-01-01T00:00:00+00:00",
        ),
    ]);
    let report = run_at_r(&system);
    let stale = findings_of_kind(&report, &FindingKind::StaleSandboxEntry);
    assert_eq!(stale.len(), 1, "expected exactly one finding: {report:#?}");
    assert!(
        stale[0].message.contains("/r/a.md") && !stale[0].message.contains("/r/b.md"),
        "only the stale file is named: {}",
        stale[0].message,
    );
}

#[test]
fn stale_sandbox_skipped_when_hook_missing() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap()
        .with_file(
            Path::new("/r/.remargin-registry.yaml"),
            sandbox_registry().as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/r/roster.md"),
            sandbox_doc("ghost_agent@2026-01-01T00:00:00+00:00").as_bytes(),
        )
        .unwrap();
    let report = run_at_r(&system);
    assert!(!report.hook_installed);
    assert_eq!(report.findings.len(), 1, "only HookMissing: {report:#?}");
    assert_eq!(report.findings[0].kind, FindingKind::HookMissing);
}

#[test]
fn stale_sandbox_serializes_and_renders() {
    let report = DoctorReport {
        elapsed_ms: None,
        findings: vec![DoctorFinding {
            kind: FindingKind::StaleSandboxEntry,
            message: String::from(
                "`notes/roster.md` carries a sandbox entry for `ghost_agent`, who is not active.",
            ),
            remedy: String::from(
                "Re-stage as a live identity, or remove the stale `sandbox:` entry from \
                 notes/roster.md.",
            ),
            severity: Severity::Warning,
        }],
        goose_guard_installed: None,
        goose_mcp_installed: None,
        goose_session_guard_installed: None,
        hook_installed: true,
        session_guard_installed: true,
        project_settings_file: PathBuf::from("/r/.claude/settings.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    };
    let json = serde_json::to_string(&report).unwrap();
    assert!(
        json.contains("stale_sandbox_entry"),
        "wire name present: {json}",
    );
    let parsed: DoctorReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, parsed);

    let text = render_doctor_text(&report, false);
    assert!(
        text.contains("[WARNING]") && text.contains("ghost_agent"),
        "text renders the stale entry as WARNING: {text}",
    );

    let prompt = render_doctor_prompt(&report);
    assert!(
        prompt.contains("Re-stage as a live identity"),
        "prompt names the remedy: {prompt}",
    );
}

/// An absent anchor inside the realm is not an escape.
#[test]
fn trusted_root_missing_flags_contained_but_absent_anchor() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
    ]);
    let report = run_at_r(&system);
    let missing = findings_of_kind(&report, &FindingKind::TrustedRootMissing);
    assert_eq!(
        missing.len(),
        1,
        "expected one missing finding: {report:#?}"
    );
    assert_eq!(missing[0].severity, Severity::Warning);
    assert!(
        missing[0].message.contains("/r/src/secret")
            && missing[0].message.contains("/r/.remargin.yaml")
            && missing[0].message.contains("does not exist"),
        "message names the anchor and declaring file: {}",
        missing[0].message,
    );
    assert!(
        findings_of_kind(&report, &FindingKind::TrustedRootEscape).is_empty(),
        "a contained entry is not an escape: {report:#?}",
    );
}

#[test]
fn trusted_root_existing_anchor_has_no_finding() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
    ])
    .with_dir(Path::new("/r/src/secret"))
    .unwrap();
    let report = run_at_r(&system);
    assert!(
        findings_of_kind(&report, &FindingKind::TrustedRootMissing).is_empty(),
        "an existing anchor must not fire: {report:#?}",
    );
    assert!(report.is_clean(), "expected clean report: {report:#?}");
}

/// A wildcard root anchors at the realm's own directory, which exists by construction.
#[test]
fn trusted_root_wildcard_never_reports_missing() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: \"*\"\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
    ]);
    let report = run_at_r(&system);
    assert!(
        findings_of_kind(&report, &FindingKind::TrustedRootMissing).is_empty(),
        "wildcard anchors at the extant realm root: {report:#?}",
    );
    assert!(report.is_clean(), "expected clean report: {report:#?}");
}

#[test]
fn trusted_root_missing_skipped_when_escape_present() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: /other/secret\n";
    let system = mock_with_files(&[
        ("/home/u/.claude/settings.json", &hook_settings_json()),
        ("/r/.remargin.yaml", yaml),
    ]);
    let report = run_at_r(&system);
    assert_eq!(
        findings_of_kind(&report, &FindingKind::TrustedRootEscape).len(),
        1,
        "escape is reported: {report:#?}",
    );
    assert!(
        findings_of_kind(&report, &FindingKind::TrustedRootMissing).is_empty(),
        "existence pass is gated behind !has_escape: {report:#?}",
    );
}

#[test]
fn trusted_root_missing_serializes_and_renders() {
    let report = DoctorReport {
        elapsed_ms: None,
        findings: vec![DoctorFinding {
            kind: FindingKind::TrustedRootMissing,
            message: String::from(
                "trusted_roots entry in /r/.remargin.yaml resolves to /r/src/secret, which does \
                 not exist. It protects nothing.",
            ),
            remedy: String::from(
                "Point the entry at an existing path, or drop it from /r/.remargin.yaml.",
            ),
            severity: Severity::Warning,
        }],
        goose_guard_installed: None,
        goose_mcp_installed: None,
        goose_session_guard_installed: None,
        hook_installed: true,
        session_guard_installed: true,
        project_settings_file: PathBuf::from("/r/.claude/settings.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    };
    let json = serde_json::to_string(&report).unwrap();
    assert!(
        json.contains("trusted_root_missing"),
        "wire name present: {json}",
    );
    let parsed: DoctorReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, parsed);

    let text = render_doctor_text(&report, false);
    assert!(
        text.contains("[WARNING]") && text.contains("/r/src/secret"),
        "text renders the missing root as WARNING: {text}",
    );

    let prompt = render_doctor_prompt(&report);
    assert!(
        prompt.contains("Point the entry at an existing path"),
        "prompt names the remedy: {prompt}",
    );
}

/// A realm that trips two independent checks at once: the `SessionStart`
/// guard is absent (`SessionGuardMissing`) and a stale `Bash(remargin *)`
/// deny sits in `settings.local.json` (`LeftoverProjectedRule`). The
/// `PreToolUse` hook is present, so the run does not short-circuit.
fn guard_missing_and_leftover_mock() -> MemorySystem {
    mock_with_files(&[
        (
            "/home/u/.claude/settings.json",
            &pretool_only_settings_json(),
        ),
        (
            "/r/.claude/settings.local.json",
            &deny_only_settings_json(&["Bash(remargin *)"]),
        ),
    ])
}

fn kinds(report: &DoctorReport) -> Vec<FindingKind> {
    report.findings.iter().map(|f| f.kind.clone()).collect()
}

#[test]
fn default_selection_runs_every_check() {
    let system = guard_missing_and_leftover_mock();
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &CheckName::all(),
    )
    .unwrap();
    let found = kinds(&report);
    assert!(
        found.contains(&FindingKind::SessionGuardMissing)
            && found.contains(&FindingKind::LeftoverProjectedRule),
        "default run surfaces both findings: {report:#?}",
    );
}

#[test]
fn single_check_selection_suppresses_other_findings() {
    let system = guard_missing_and_leftover_mock();
    let mut checks = HashSet::new();
    checks.insert(CheckName::LeftoverRules);
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &checks,
    )
    .unwrap();
    assert_eq!(
        kinds(&report),
        vec![FindingKind::LeftoverProjectedRule],
        "only the selected check's finding is present: {report:#?}",
    );
}

#[test]
fn multiple_check_selection_runs_exactly_those() {
    let system = guard_missing_and_leftover_mock();
    let checks: HashSet<CheckName> = [CheckName::Hook, CheckName::SessionGuard]
        .into_iter()
        .collect();
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &checks,
    )
    .unwrap();
    assert_eq!(
        kinds(&report),
        vec![FindingKind::SessionGuardMissing],
        "only session-guard runs, not leftover: {report:#?}",
    );
}

#[test]
fn unknown_check_name_errors_and_lists_valid() {
    let err = CheckName::parse_set("bogus").unwrap_err().to_string();
    assert!(
        err.contains("unknown check `bogus`"),
        "message names the bad slug: {err}",
    );
    assert!(
        err.contains("hook") && err.contains("session-guard"),
        "message lists valid slugs: {err}",
    );
}

/// The gate still short-circuits, but reports no `HookMissing` when `hook` is not selected.
#[test]
fn hook_gate_short_circuits_silently_when_deselected() {
    let system = mock_with_files(&[]);
    let mut checks = HashSet::new();
    checks.insert(CheckName::SessionGuard);
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &checks,
    )
    .unwrap();
    assert!(!report.hook_installed, "hook is absent: {report:#?}");
    assert_eq!(
        kinds(&report),
        Vec::new(),
        "a deselected hook reports nothing, and the gate still skips every later check: \
         {report:#?}",
    );
}

#[test]
fn hook_gate_reports_when_selected() {
    let system = mock_with_files(&[]);
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &CheckName::parse_set("hook,session-guard").unwrap(),
    )
    .unwrap();
    assert!(!report.hook_installed, "hook is absent: {report:#?}");
    assert_eq!(
        kinds(&report),
        vec![FindingKind::HookMissing],
        "the selected gate finding leads and short-circuits: {report:#?}",
    );
}

#[test]
fn parse_set_trims_and_all_is_complete() {
    let parsed = CheckName::parse_set(" hook , session-guard ,").unwrap();
    let expected: HashSet<CheckName> = [CheckName::Hook, CheckName::SessionGuard]
        .into_iter()
        .collect();
    assert_eq!(parsed, expected);
    assert_eq!(
        CheckName::all().len(),
        11,
        "every check has exactly one slug",
    );
    assert!(CheckName::parse_set("").unwrap().is_empty());
}

/// A mock carrying both Claude hooks (so the gate does not short-circuit),
/// a `HOME` env var, and whichever extra files the case needs.
fn goose_mock(extra: &[(&str, &str)]) -> MemorySystem {
    let mut files = vec![("/home/u/.claude/settings.json", hook_settings_json())];
    files.extend(extra.iter().map(|(p, b)| ((*p), (*b).to_owned())));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (*path, body.as_str()))
        .collect();
    let system = mock_with_files(&borrowed);
    system.set_env_var("HOME", "/home/u");
    system
}

fn goose_hooks_json(command: &str) -> String {
    let v = json!({
        "hooks": { "PreToolUse": [{ "hooks": [
            { "type": "command", "command": command },
        ] }] },
    });
    serde_json::to_string_pretty(&v).unwrap()
}

fn run_goose_doctor(system: &dyn System) -> DoctorReport {
    super::run_doctor(
        system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &CheckName::all(),
    )
    .unwrap()
}

/// No `~/.agents` means no goose installation, so the check is silent.
#[test]
fn goose_absent_produces_no_finding() {
    let report = run_goose_doctor(&goose_mock(&[]));
    assert!(
        !kinds(&report).iter().any(|k| matches!(
            *k,
            FindingKind::GooseGuardMissing | FindingKind::GooseGuardBroken
        )),
        "no goose installed, so no goose finding: {report:#?}",
    );
}

#[test]
fn goose_present_without_guard_is_flagged() {
    let system = goose_mock(&[("/home/u/.agents/plugins/other/plugin.json", "{}")]);
    let report = run_goose_doctor(&system);
    assert!(
        kinds(&report).contains(&FindingKind::GooseGuardMissing),
        "expected GooseGuardMissing: {report:#?}",
    );
    let finding = report
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::GooseGuardMissing)
        .unwrap();
    assert_eq!(finding.severity, Severity::Critical);
    assert!(
        finding.remedy.contains("remargin goose pretool install"),
        "remedy should name the install command: {finding:#?}",
    );
}

/// An unparseable hook manifest is a different repair from an absent plugin.
#[test]
fn goose_present_with_broken_guard_is_flagged_as_broken() {
    let system = goose_mock(&[(
        "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
        "{ not json",
    )]);
    let report = run_goose_doctor(&system);
    assert!(
        kinds(&report).contains(&FindingKind::GooseGuardBroken),
        "expected GooseGuardBroken: {report:#?}",
    );
}

#[test]
fn goose_present_with_wired_guard_is_clean() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        (
            "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json("/opt/bin/remargin goose pretool"),
        ),
    ]);
    let report = run_goose_doctor(&system);
    assert!(
        !kinds(&report).iter().any(|k| matches!(
            *k,
            FindingKind::GooseGuardMissing | FindingKind::GooseGuardBroken
        )),
        "wired guard should be clean: {report:#?}",
    );
}

#[test]
fn goose_project_scope_guard_satisfies_the_check() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        ("/home/u/.agents/marker", "x"),
        (
            "/r/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json("/opt/bin/remargin goose pretool"),
        ),
    ]);
    let report = run_goose_doctor(&system);
    assert!(
        !kinds(&report).iter().any(|k| matches!(
            *k,
            FindingKind::GooseGuardMissing | FindingKind::GooseGuardBroken
        )),
        "project-scope guard should be clean: {report:#?}",
    );
}

#[test]
fn goose_guard_check_is_selectable_by_slug() {
    let system = goose_mock(&[("/home/u/.agents/marker", "x")]);

    let selected = CheckName::parse_set("goose-guard").unwrap();
    assert!(selected.contains(&CheckName::GooseGuard));
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &selected,
    )
    .unwrap();
    assert_eq!(kinds(&report), vec![FindingKind::GooseGuardMissing]);

    let other = CheckName::parse_set("session-guard").unwrap();
    let deselected = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &other,
    )
    .unwrap();
    assert!(
        !kinds(&deselected).contains(&FindingKind::GooseGuardMissing),
        "deselected check must not report: {deselected:#?}",
    );
}

/// A manifest carrying the `PreToolUse` entry and, optionally, the
/// `SessionStart` backstop beside it.
fn goose_hooks_json_with_session(binary: &str) -> String {
    let v = json!({
        "hooks": {
            "PreToolUse": [{ "hooks": [
                { "type": "command", "command": format!("{binary} goose pretool") },
            ] }],
            "SessionStart": [{ "hooks": [
                { "type": "command", "command": format!("{binary} goose session-guard") },
            ] }],
        },
    });
    serde_json::to_string_pretty(&v).unwrap()
}

#[test]
fn goose_absent_produces_no_session_guard_finding() {
    let report = run_goose_doctor(&goose_mock(&[]));
    assert!(
        !kinds(&report).contains(&FindingKind::GooseSessionGuardMissing),
        "no goose installed, so no backstop finding: {report:#?}",
    );
}

/// The blocking guard is live, but nothing reports it when it breaks.
#[test]
fn goose_guard_without_the_session_entry_is_flagged() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        (
            "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json("/opt/bin/remargin goose pretool"),
        ),
    ]);
    let report = run_goose_doctor(&system);
    assert!(
        !kinds(&report).contains(&FindingKind::GooseGuardMissing),
        "the PreToolUse guard is wired: {report:#?}",
    );
    let finding = report
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::GooseSessionGuardMissing)
        .unwrap();
    assert_eq!(finding.severity, Severity::Critical);
    assert!(
        finding
            .remedy
            .contains("remargin goose session-guard install"),
        "remedy should name the install command: {finding:#?}",
    );
}

#[test]
fn goose_with_the_session_entry_is_clean() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        (
            "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json_with_session("/opt/bin/remargin"),
        ),
    ]);
    let report = run_goose_doctor(&system);
    assert!(
        !kinds(&report).iter().any(|k| matches!(
            *k,
            FindingKind::GooseGuardBroken
                | FindingKind::GooseGuardMissing
                | FindingKind::GooseSessionGuardMissing
        )),
        "a fully wired goose stack should be clean: {report:#?}",
    );
}

#[test]
fn goose_project_scope_session_entry_satisfies_the_check() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        ("/home/u/.agents/marker", "x"),
        (
            "/r/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json_with_session("/opt/bin/remargin"),
        ),
    ]);
    let report = run_goose_doctor(&system);
    assert!(
        !kinds(&report).contains(&FindingKind::GooseSessionGuardMissing),
        "project-scope backstop should be clean: {report:#?}",
    );
}

#[test]
fn goose_session_entry_pointing_at_a_missing_binary_names_the_fault() {
    let system = goose_mock(&[(
        "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
        &goose_hooks_json_with_session("/opt/bin/remargin"),
    )]);
    let report = run_goose_doctor(&system);
    let finding = report
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::GooseSessionGuardMissing)
        .unwrap();
    assert!(
        finding.message.contains("/opt/bin/remargin"),
        "message should name the missing binary: {finding:#?}",
    );
}

#[test]
fn goose_session_guard_check_is_selectable_by_slug() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        (
            "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json("/opt/bin/remargin goose pretool"),
        ),
    ]);

    let selected = CheckName::parse_set("goose-session-guard").unwrap();
    assert!(selected.contains(&CheckName::GooseSessionGuard));
    let report = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &selected,
    )
    .unwrap();
    assert_eq!(kinds(&report), vec![FindingKind::GooseSessionGuardMissing]);

    let other = CheckName::parse_set("goose-guard").unwrap();
    let deselected = super::run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
        &other,
    )
    .unwrap();
    assert!(
        !kinds(&deselected).contains(&FindingKind::GooseSessionGuardMissing),
        "deselected check must not report: {deselected:#?}",
    );
}

#[test]
fn goose_absent_leaves_both_verdicts_unset() {
    let report = run_goose_doctor(&goose_mock(&[]));
    assert_eq!(report.goose_guard_installed, None, "{report:#?}");
    assert_eq!(report.goose_session_guard_installed, None, "{report:#?}");
}

/// An unset verdict is omitted from the wire, never `null`, and reads back as `None`.
#[test]
fn goose_absent_omits_both_verdict_keys_from_the_wire() {
    let report = run_goose_doctor(&goose_mock(&[]));
    let json = serde_json::to_string(&report).unwrap();
    assert!(
        !json.contains("goose_guard_installed"),
        "unset verdict must be absent, not null: {json}",
    );
    assert!(
        !json.contains("goose_session_guard_installed"),
        "unset verdict must be absent, not null: {json}",
    );

    let parsed: DoctorReport = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.goose_guard_installed, None, "{parsed:#?}");
    assert_eq!(parsed.goose_session_guard_installed, None, "{parsed:#?}");
    assert_eq!(report, parsed);
}

#[test]
fn goose_without_the_plugin_reports_false_verdicts() {
    let system = goose_mock(&[("/home/u/.agents/marker", "x")]);
    let report = run_goose_doctor(&system);
    assert_eq!(report.goose_guard_installed, Some(false), "{report:#?}");
    assert_eq!(
        report.goose_session_guard_installed,
        Some(false),
        "{report:#?}",
    );
}

#[test]
fn goose_wired_stack_reports_true_verdicts() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        (
            "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json_with_session("/opt/bin/remargin"),
        ),
    ]);
    let report = run_goose_doctor(&system);
    assert_eq!(report.goose_guard_installed, Some(true), "{report:#?}");
    assert_eq!(
        report.goose_session_guard_installed,
        Some(true),
        "{report:#?}",
    );

    let json = serde_json::to_string(&report).unwrap();
    assert!(
        json.contains("\"goose_guard_installed\":true")
            && json.contains("\"goose_session_guard_installed\":true"),
        "wire shape carries both verdicts: {json}",
    );
    let parsed: DoctorReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, parsed);
}

#[test]
fn goose_pretool_only_install_splits_the_verdicts() {
    let system = goose_mock(&[
        ("/opt/bin/remargin", "binary"),
        (
            "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
            &goose_hooks_json("/opt/bin/remargin goose pretool"),
        ),
    ]);
    let report = run_goose_doctor(&system);
    assert_eq!(report.goose_guard_installed, Some(true), "{report:#?}");
    assert_eq!(
        report.goose_session_guard_installed,
        Some(false),
        "{report:#?}",
    );
}

/// The plugin directory's presence is not a pass.
#[test]
fn goose_broken_plugin_reports_a_false_guard_verdict() {
    let system = goose_mock(&[(
        "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
        "{ not json",
    )]);
    let report = run_goose_doctor(&system);
    assert!(kinds(&report).contains(&FindingKind::GooseGuardBroken));
    assert_eq!(report.goose_guard_installed, Some(false), "{report:#?}");
}

/// Dropping the verdicts at the short-circuit would claim there is no goose installation.
#[test]
fn goose_verdicts_survive_the_hook_missing_short_circuit() {
    let system = mock_with_files(&[(
        "/home/u/.agents/plugins/remargin-guard/hooks/hooks.json",
        "{ not json",
    )]);
    system.set_env_var("HOME", "/home/u");
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert_eq!(kinds(&report), vec![FindingKind::HookMissing]);
    assert_eq!(report.goose_guard_installed, Some(false), "{report:#?}");
    assert_eq!(
        report.goose_session_guard_installed,
        Some(false),
        "{report:#?}",
    );
}

#[test]
fn render_verbose_omits_goose_lines_without_goose() {
    let out = render_doctor_text(&clean_report(), true);
    assert!(out.contains("Checks:"), "missing Checks: {out}");
    assert!(
        !out.contains("goose-guard:"),
        "unexpected goose line: {out}"
    );
    assert!(
        !out.contains("goose-session-guard:"),
        "unexpected goose line: {out}",
    );
}

#[test]
fn render_verbose_names_both_goose_verdicts() {
    let mut report = clean_report();
    report.goose_guard_installed = Some(true);
    report.goose_session_guard_installed = Some(false);
    let out = render_doctor_text(&report, true);
    assert!(
        out.contains("goose-guard: ok"),
        "missing goose guard verdict: {out}",
    );
    assert!(
        out.contains("goose-session-guard: missing"),
        "missing goose backstop verdict: {out}",
    );
}

#[test]
fn render_plain_omits_goose_lines() {
    let mut report = clean_report();
    report.goose_guard_installed = Some(true);
    report.goose_session_guard_installed = Some(true);
    let out = render_doctor_text(&report, false);
    assert!(!out.contains("goose-guard"), "goose line in plain: {out}");
}
