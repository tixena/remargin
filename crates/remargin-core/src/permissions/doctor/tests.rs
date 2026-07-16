//! Unit tests for [`crate::permissions::doctor`].

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;
use serde_json::json;

use crate::permissions::doctor::{
    DoctorFinding, DoctorReport, FindingKind, Severity, render_doctor_prompt, render_doctor_text,
    run_doctor,
};
use crate::permissions::pretool_install::{HOOK_COMMAND, HOOK_MATCHER};
use crate::permissions::session_guard_install::SESSION_HOOK_COMMAND;

/// Settings carrying both enforcement hooks — the fully-configured, clean
/// state (`PreToolUse` enforcement + `SessionStart` guard).
fn hook_settings_json() -> String {
    let v = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": HOOK_MATCHER,
                    "hooks": [
                        { "type": "command", "command": HOOK_COMMAND }
                    ]
                }
            ],
            "SessionStart": [
                {
                    "hooks": [
                        { "type": "command", "command": SESSION_HOOK_COMMAND }
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
                        { "type": "command", "command": HOOK_COMMAND }
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
                        { "type": "command", "command": SESSION_HOOK_COMMAND }
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

fn mock_with_file(path: &str, body: &str) -> MockSystem {
    mock_with_files(&[(path, body)])
}

fn mock_with_files(files: &[(&str, &str)]) -> MockSystem {
    let mut system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap();
    for (path, body) in files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    system
}

/// Hook present in user-scope → clean report.
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
    assert!(report.findings.is_empty());
}

/// Hook present in project-scope → clean report.
#[test]
fn hook_in_project_scope_is_clean() {
    let system = mock_with_file("/r/.claude/settings.local.json", &hook_settings_json());
    let report = run_doctor(
        &system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap();
    assert!(report.hook_installed, "expected hook_installed=true");
    assert!(report.is_clean());
}

/// Hook absent from both scopes → `HookMissing` finding (critical severity).
#[test]
fn hook_absent_from_both_scopes_reports_hook_missing() {
    let system = MockSystem::new()
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

/// `HookMissing` finding references both settings file paths.
#[test]
fn hook_missing_finding_names_both_files() {
    let system = MockSystem::new()
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
        finding.message.contains("/r/.claude/settings.local.json"),
        "message should name project-scope file: {}",
        finding.message
    );
}

/// Findings order: `HookMissing` comes first (it gates everything else).
#[test]
fn hook_missing_is_first_finding() {
    let system = MockSystem::new()
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
    assert!(!report.findings.is_empty());
    assert_eq!(report.findings[0].kind, FindingKind::HookMissing);
}

/// `DoctorReport` serializes to JSON without losing fields.
#[test]
fn doctor_report_json_round_trip() {
    let system = MockSystem::new()
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

/// Returns correct `project_settings_file` and `user_settings_file` paths.
#[test]
fn report_includes_correct_settings_file_paths() {
    let system = MockSystem::new()
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
        PathBuf::from("/r/.claude/settings.local.json")
    );
    assert_eq!(
        report.user_settings_file,
        PathBuf::from("/home/u/.claude/settings.json")
    );
}

// --- SessionStart guard (SessionGuardMissing) unit tests ---

/// Case 1: both hooks present in user-scope, guard installed, no
/// `SessionGuardMissing`, clean report.
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

/// Case 2: `PreToolUse` hook in user-scope, `SessionStart` guard in
/// project-scope only, both checks pass, no finding.
#[test]
fn guard_in_project_scope_only_is_clean() {
    let system = mock_with_files(&[
        (
            "/home/u/.claude/settings.json",
            &pretool_only_settings_json(),
        ),
        (
            "/r/.claude/settings.local.json",
            &guard_only_settings_json(),
        ),
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

/// Case 3: `PreToolUse` hook present but the guard is absent from both
/// scopes, exactly one `SessionGuardMissing` finding (Critical) naming
/// the install command.
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

// --- render_doctor_text unit tests ---

fn clean_report() -> DoctorReport {
    DoctorReport {
        findings: vec![],
        hook_installed: true,
        session_guard_installed: true,
        project_settings_file: PathBuf::from("/r/.claude/settings.local.json"),
        user_settings_file: PathBuf::from("/home/u/.claude/settings.json"),
    }
}

fn findings_report() -> DoctorReport {
    DoctorReport {
        findings: vec![DoctorFinding {
            kind: FindingKind::HookMissing,
            message: String::from("hook is missing"),
            remedy: String::from("run install"),
            severity: Severity::Critical,
        }],
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

// --- LeftoverProjectedRule (drift detection) unit tests ---

fn leftover_findings(report: &DoctorReport) -> Vec<&DoctorFinding> {
    report
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::LeftoverProjectedRule)
        .collect()
}

/// Case 1: a settings file carrying the stale `Bash(remargin *)` CLI
/// deny — a shape `rules_for` no longer emits — yields one
/// `LeftoverProjectedRule` (Warning) naming the file, the rule, and a
/// removal remedy.
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

/// Case 2: a settings file carrying a path deny that `rules_for` still
/// projects for the realm (`Edit(/r/**)` under a wildcard trusted
/// root) is flagged as leftover.
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

/// Case 3: a clean, hook-only settings tree (no projected or stale deny
/// rules) yields no `LeftoverProjectedRule` and a clean report.
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

fn leftover_finding_fixture(rule: &str, file: &str) -> DoctorFinding {
    DoctorFinding {
        kind: FindingKind::LeftoverProjectedRule,
        message: format!("The deny rule `{rule}` in {file} is drift."),
        remedy: format!("Remove the deny rule `{rule}` from the permissions.deny array in {file}."),
        severity: Severity::Warning,
    }
}

/// Case 4: `--prompt-mode` over two leftover findings emits one
/// imperative instruction per finding, naming both rules and both
/// files.
#[test]
fn render_prompt_names_each_finding_rule_and_file() {
    let report = DoctorReport {
        findings: vec![
            leftover_finding_fixture("Bash(remargin *)", "/r/.claude/settings.local.json"),
            leftover_finding_fixture("Edit(/r/**)", "/home/u/.claude/settings.json"),
        ],
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

/// Case 5: `--prompt-mode` over a clean report emits a "nothing to do"
/// prompt with no instructions.
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
