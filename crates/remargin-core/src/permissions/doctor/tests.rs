//! Unit tests for [`crate::permissions::doctor`].

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;
use serde_json::json;

use crate::permissions::doctor::{
    DoctorFinding, DoctorReport, FindingKind, Severity, render_doctor_text, run_doctor,
};
use crate::permissions::pretool_install::{HOOK_COMMAND, HOOK_MATCHER};

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
            ]
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

fn mock_with_file(path: &str, body: &str) -> MockSystem {
    MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap()
        .with_file(Path::new(path), body.as_bytes())
        .unwrap()
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

// --- render_doctor_text unit tests ---

fn clean_report() -> DoctorReport {
    DoctorReport {
        findings: vec![],
        hook_installed: true,
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
