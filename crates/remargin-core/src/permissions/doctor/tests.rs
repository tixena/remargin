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

/// Hook present in project-scope → clean report. Project scope is
/// `.claude/settings.json` — the file `install --local` writes.
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
        finding.message.contains("/r/.claude/settings.json"),
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
        PathBuf::from("/r/.claude/settings.json")
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

// --- TrustedRootEscape (out-of-realm entry) unit tests ---

/// An out-of-realm `trusted_roots` entry makes `resolve_permissions` fail
/// closed. Doctor must still produce a finding — naming the entry and the
/// resolved anchor, with a move-it-back remedy — rather than crash on the
/// resolve error it exists to explain.
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

// --- identity / key resolvability (IdentityKeyUnresolvable,
//     AgentKeyUnderUserSsh) unit tests ---

fn strict_agent_registry() -> &'static str {
    "participants:\n  agent1:\n    type: agent\n    status: active\n"
}

/// Hook installed in user-scope and `HOME` set, so `~/.ssh` derivation and
/// plain-name `key:` resolution behave as they do in a real run. Extra
/// realm files (`.remargin.yaml`, registry, key files) are layered on top.
fn identity_mock(files: &[(&str, &str)]) -> MockSystem {
    let mut system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap()
        .with_env("HOME", "/home/u")
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

fn run_at_r(system: &MockSystem) -> DoctorReport {
    run_doctor(
        system,
        Path::new("/r"),
        Path::new("/home/u/.claude/settings.json"),
    )
    .unwrap()
}

/// Scenario 1: strict realm whose `key:` is set but points at no file →
/// one `IdentityKeyUnresolvable` naming the identity, key, and config.
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

/// Scenario 2: strict realm whose `key:` resolves to a path that exists
/// but does not read back as a file (a directory here) → still one
/// `IdentityKeyUnresolvable`. Proves the probe checks readability, not
/// mere existence.
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

/// Scenario 3: strict realm whose `key:` points at a readable file → no
/// identity finding.
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

/// Scenario 4: open mode with a missing key → the strict-only readability
/// check does not fire.
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

/// Scenario 5: an agent identity whose `key:` resolves under the user's
/// `~/.ssh` → one `AgentKeyUnderUserSsh` naming the identity and key.
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

/// Scenario 6: a human identity whose `key:` lives under `~/.ssh` → no
/// finding (`~/.ssh` is the expected home for a human key).
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

/// Scenario 7: the hook is absent → the report leads with `HookMissing`
/// and every later check, including the identity/key check, is skipped.
#[test]
fn hook_missing_skips_identity_key_check() {
    let system = MockSystem::new()
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

/// The new kinds serialize to their `snake_case` wire names, round-trip
/// through JSON, render as WARNING in text, and each contributes one
/// prompt-mode instruction.
#[test]
fn identity_findings_render_and_serialize() {
    let report = DoctorReport {
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

// --- ConfigSchemaLint (permissions-schema drift across the realm tree)
//     unit tests ---

fn schema_lint_findings(report: &DoctorReport) -> Vec<&DoctorFinding> {
    report
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::ConfigSchemaLint)
        .collect()
}

/// Scenario 1: a `.remargin.yaml` with a YAML syntax error in the parent
/// walk yields one `ConfigSchemaLint` (Warning) naming the file, with a
/// remedy pointing at the schema in that file.
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

/// Scenario 2: an unknown key under `permissions:` (serde rejects unknown
/// fields) yields one `ConfigSchemaLint` carrying the parser diagnostic
/// with the offending field name.
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

/// Scenario 3: a `deny_ops` entry carrying the legacy `to:` field yields a
/// `ConfigSchemaLint` with the migration hint. The same input also fails
/// serde (the entry rejects the unknown `to:`), so a second parse-error
/// schema lint accompanies it — the migration hint is asserted by
/// presence, matching the standalone lint's behavior.
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

/// Scenario 4: an out-of-realm `trusted_roots` entry is reported once — by
/// the dedicated `TrustedRootEscape` check — and NOT duplicated as a
/// `ConfigSchemaLint`, since both consult the same escape detector.
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

/// Scenario 5: a clean realm tree with valid configs produces zero
/// `ConfigSchemaLint` findings.
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

/// The new kind serializes to its `snake_case` wire name, round-trips
/// through JSON, renders as WARNING in text, and contributes one
/// prompt-mode instruction.
#[test]
fn config_schema_lint_serializes_and_renders() {
    let report = DoctorReport {
        findings: vec![DoctorFinding {
            kind: FindingKind::ConfigSchemaLint,
            message: String::from("/r/.remargin.yaml (line 2, col 3): unknown field `deny_op`"),
            remedy: String::from("Fix the permissions schema in /r/.remargin.yaml."),
            severity: Severity::Warning,
        }],
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

// --- StaleSandboxEntry (sandbox staging hygiene) unit tests ---

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

/// Scenario 1: a doc staged for an author absent from the registry yields
/// one `StaleSandboxEntry` (Warning) naming the file and the orphaned
/// author, with a removal remedy.
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

/// Scenario 2: a doc staged for an active participant yields no finding.
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

/// Scenario 3: a doc staged for a present-but-revoked participant is
/// flagged — a retired identity's staging is exactly the drift this check
/// exists to surface.
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

/// Scenario 4: a realm with no registry yields no findings — with no
/// registry there is no notion of an author "not backed" by a live
/// identity.
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

/// Scenario 5: a stale entry on file A and a live entry on file B produce
/// exactly one finding, naming file A only. A non-markdown file carrying
/// sandbox-looking text is skipped, proving the walk continues past it.
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

/// Scenario 6: the hook is absent → the report leads with `HookMissing`
/// and the resolve-dependent stale-sandbox check is skipped.
#[test]
fn stale_sandbox_skipped_when_hook_missing() {
    let system = MockSystem::new()
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

/// The new kind serializes to its `snake_case` wire name, round-trips
/// through JSON, renders as WARNING in text, and contributes one
/// prompt-mode instruction.
#[test]
fn stale_sandbox_serializes_and_renders() {
    let report = DoctorReport {
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

// --- TrustedRootMissing (contained but absent anchor) unit tests ---

/// A `trusted_roots` entry that stays inside its realm but resolves to a
/// path that does not exist yields one `TrustedRootMissing` naming the
/// resolved anchor and the declaring `.remargin.yaml`. It is not an escape.
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

/// The same entry resolving to a directory that exists yields no
/// `TrustedRootMissing` and a clean report.
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

/// A wildcard root anchors at the declaring realm's own directory, which
/// exists by construction, so it never reports missing.
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

/// An out-of-realm entry is reported once — as `TrustedRootEscape` — and
/// the existence pass is gated off, so no `TrustedRootMissing` is added for
/// the same misconfig.
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

/// The new kind serializes to its `snake_case` wire name, round-trips
/// through JSON, renders as WARNING in text, and contributes one
/// prompt-mode instruction.
#[test]
fn trusted_root_missing_serializes_and_renders() {
    let report = DoctorReport {
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
