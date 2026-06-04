//! Unit tests for [`crate::permissions::doctor`].

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;
use serde_json::json;

use crate::permissions::doctor::{DoctorReport, FindingKind, Severity, run_doctor};
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
