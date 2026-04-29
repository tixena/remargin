//! Unit tests for [`crate::operations::projections::unprotect::project_unprotect`]
//! (rem-6eop / T43).

use std::io;
use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::operations::plan::{
    UnprotectConfigDiff, UnprotectConflict, UnprotectEntryAction, UnprotectSidecarDiff,
    UnprotectYamlDiff,
};
use crate::operations::projections::unprotect::{UnprotectProjection, project_unprotect};
use crate::permissions::restrict::{RestrictArgs, restrict};
use crate::permissions::unprotect::UnprotectArgs;

fn snapshot(system: &MockSystem, paths: &[&Path]) -> Vec<(PathBuf, Result<String, io::Error>)> {
    paths
        .iter()
        .map(|p| (p.to_path_buf(), system.read_to_string(p)))
        .collect()
}

/// Realm fixture: `<r>/.claude/` exists, no `.remargin.yaml`, no
/// settings files. Anchor is `<r>`. Returns `(system, realm_root,
/// project_settings, user_settings)`.
fn fresh_realm() -> (MockSystem, PathBuf, PathBuf, PathBuf) {
    let realm = PathBuf::from("/realm");
    let system = MockSystem::new()
        .with_dir(&realm)
        .unwrap()
        .with_dir(realm.join(".claude"))
        .unwrap();
    let project = realm.join(".claude/settings.local.json");
    let user = PathBuf::from("/home/u/.claude/settings.json");
    (system, realm, project, user)
}

#[track_caller]
fn diff_or_fail(projection: UnprotectProjection) -> Box<UnprotectConfigDiff> {
    match projection {
        UnprotectProjection::Diff(diff) => diff,
        UnprotectProjection::Reject(reason) => {
            assert_eq!(reason, "<expected UnprotectProjection::Diff>");
            Box::new(UnprotectConfigDiff {
                absolute_path: PathBuf::new(),
                anchor: PathBuf::new(),
                conflicts: Vec::new(),
                remargin_yaml: UnprotectYamlDiff {
                    entry_action: UnprotectEntryAction::Absent,
                    path: PathBuf::new(),
                    previous_entry: None,
                },
                settings_files: Vec::new(),
                sidecar: UnprotectSidecarDiff {
                    entry_action: UnprotectEntryAction::Absent,
                    path: PathBuf::new(),
                },
            })
        }
    }
}

#[track_caller]
fn reject_or_fail(projection: UnprotectProjection) -> String {
    match projection {
        UnprotectProjection::Reject(reason) => reason,
        UnprotectProjection::Diff(_diff) => {
            assert_eq!(String::new(), "<expected UnprotectProjection::Reject>");
            String::new()
        }
    }
}

fn unprotect_args(path: &str) -> UnprotectArgs {
    UnprotectArgs::new(String::from(path))
}

fn restrict_args(path: &str) -> RestrictArgs {
    RestrictArgs::new(String::from(path), Vec::new(), false)
}

/// After a clean `restrict src/secret`, projecting `unprotect src/secret`
/// reports `WouldBeRemoved` for both the YAML entry and the sidecar
/// entry, with non-empty `rules_to_remove` for each tracked file and
/// no conflicts.
#[test]
fn clean_projection_after_restrict() {
    let (system, realm, project, user) = fresh_realm();
    let settings = vec![project, user];
    restrict(&system, &realm, &restrict_args("src/secret"), &settings).unwrap();

    let projection = project_unprotect(&system, &realm, &unprotect_args("src/secret")).unwrap();
    let diff = diff_or_fail(projection);

    assert_eq!(diff.anchor, realm);
    assert_eq!(diff.absolute_path, realm.join("src/secret"));
    assert!(matches!(
        diff.remargin_yaml.entry_action,
        UnprotectEntryAction::WouldBeRemoved
    ));
    assert!(matches!(
        diff.sidecar.entry_action,
        UnprotectEntryAction::WouldBeRemoved
    ));
    assert_eq!(diff.settings_files.len(), 2);
    for sf in &diff.settings_files {
        assert!(
            !sf.rules_to_remove.is_empty(),
            "expected non-empty rules_to_remove for {}",
            sf.path.display()
        );
        assert!(sf.rules_already_absent.is_empty());
    }
    assert!(
        diff.conflicts.is_empty(),
        "expected no conflicts on the clean path: {:?}",
        diff.conflicts
    );
}

/// Path was never restricted: noop signals via both `Absent` entry
/// actions, and both `YamlEntryMissing` + `SidecarEntryMissing`
/// surface as conflicts.
#[test]
fn never_restricted_yields_both_missing_conflicts() {
    let (system, realm, _project, _user) = fresh_realm();

    let projection = project_unprotect(&system, &realm, &unprotect_args("src/secret")).unwrap();
    let diff = diff_or_fail(projection);

    assert!(matches!(
        diff.remargin_yaml.entry_action,
        UnprotectEntryAction::Absent
    ));
    assert!(matches!(
        diff.sidecar.entry_action,
        UnprotectEntryAction::Absent
    ));
    assert!(diff.settings_files.is_empty());
    let saw_yaml_missing = diff
        .conflicts
        .iter()
        .any(|c| matches!(c, UnprotectConflict::YamlEntryMissing { .. }));
    let saw_sidecar_missing = diff
        .conflicts
        .iter()
        .any(|c| matches!(c, UnprotectConflict::SidecarEntryMissing { .. }));
    assert!(saw_yaml_missing, "expected YamlEntryMissing conflict");
    assert!(saw_sidecar_missing, "expected SidecarEntryMissing conflict");
}

/// YAML present, sidecar missing: `yaml.WouldBeRemoved`;
/// `sidecar.Absent`; `settings_files` empty (no rules to look up).
#[test]
fn yaml_present_sidecar_missing() {
    let (system, realm, project, user) = fresh_realm();
    let settings = vec![project, user];
    restrict(&system, &realm, &restrict_args("src/secret"), &settings).unwrap();

    // Manually wipe the sidecar by writing an empty entries map.
    let sidecar_path = realm.join(".claude/.remargin-restrictions.json");
    let empty_sidecar = serde_json::json!({"version": 1_u32, "entries": {}}).to_string();
    let with_empty_sidecar = system
        .with_file(&sidecar_path, empty_sidecar.as_bytes())
        .unwrap();

    let projection =
        project_unprotect(&with_empty_sidecar, &realm, &unprotect_args("src/secret")).unwrap();
    let diff = diff_or_fail(projection);

    assert!(matches!(
        diff.remargin_yaml.entry_action,
        UnprotectEntryAction::WouldBeRemoved
    ));
    assert!(matches!(
        diff.sidecar.entry_action,
        UnprotectEntryAction::Absent
    ));
    assert!(diff.settings_files.is_empty());
    let saw_sidecar_missing = diff
        .conflicts
        .iter()
        .any(|c| matches!(c, UnprotectConflict::SidecarEntryMissing { .. }));
    assert!(saw_sidecar_missing);
}

/// YAML missing, sidecar present: `yaml.Absent`;
/// `sidecar.WouldBeRemoved`; settings `rules_to_remove` non-empty;
/// conflict `YamlEntryMissing`.
#[test]
fn sidecar_present_yaml_missing() {
    let (system, realm, project, user) = fresh_realm();
    let settings = vec![project, user];
    restrict(&system, &realm, &restrict_args("src/secret"), &settings).unwrap();

    // Wipe the YAML — the sidecar still tracks the rules, but the
    // YAML no longer references the path.
    let yaml_path = realm.join(".remargin.yaml");
    let stripped = "permissions: {}\n";
    let with_stripped_yaml = system.with_file(&yaml_path, stripped.as_bytes()).unwrap();

    let projection =
        project_unprotect(&with_stripped_yaml, &realm, &unprotect_args("src/secret")).unwrap();
    let diff = diff_or_fail(projection);

    assert!(matches!(
        diff.remargin_yaml.entry_action,
        UnprotectEntryAction::Absent
    ));
    assert!(matches!(
        diff.sidecar.entry_action,
        UnprotectEntryAction::WouldBeRemoved
    ));
    assert!(!diff.settings_files.is_empty());
    for sf in &diff.settings_files {
        assert!(!sf.rules_to_remove.is_empty());
    }
    let saw_yaml_missing = diff
        .conflicts
        .iter()
        .any(|c| matches!(c, UnprotectConflict::YamlEntryMissing { .. }));
    assert!(saw_yaml_missing);
}

/// One of the tracked rules has been manually deleted from the
/// project-scope settings file. The plan reports it under
/// `rules_already_absent` and surfaces a `RuleAlreadyAbsent` conflict.
#[test]
fn rule_already_absent_drift_surfaces_conflict() {
    let (system, realm, project, user) = fresh_realm();
    let settings = vec![project.clone(), user];
    restrict(&system, &realm, &restrict_args("src/secret"), &settings).unwrap();

    // Read project settings, drop the first deny rule, write back.
    let body = system.read_to_string(&project).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let removed_rule = {
        let deny = value
            .get_mut("permissions")
            .and_then(|v| v.get_mut("deny"))
            .and_then(serde_json::Value::as_array_mut)
            .unwrap();
        let popped: serde_json::Value = deny.remove(0);
        String::from(popped.as_str().unwrap())
    };
    let updated = serde_json::to_string_pretty(&value).unwrap();
    let with_drift = system.with_file(&project, updated.as_bytes()).unwrap();

    let projection = project_unprotect(&with_drift, &realm, &unprotect_args("src/secret")).unwrap();
    let diff = diff_or_fail(projection);

    let project_diff = diff
        .settings_files
        .iter()
        .find(|sf| sf.path == project)
        .unwrap();
    assert!(
        project_diff
            .rules_already_absent
            .iter()
            .any(|r| r == &removed_rule),
        "expected {removed_rule} in rules_already_absent, got {:?}",
        project_diff.rules_already_absent
    );
    let saw = diff.conflicts.iter().any(|c| {
        matches!(
            c,
            UnprotectConflict::RuleAlreadyAbsent { rule, settings_file }
                if rule == &removed_rule && settings_file == &project
        )
    });
    assert!(
        saw,
        "expected RuleAlreadyAbsent for {removed_rule} on {}, conflicts: {:?}",
        project.display(),
        diff.conflicts
    );
}

/// Wildcard plan after `restrict *` works symmetrically: same shape as
/// the named-path projection.
#[test]
fn wildcard_projection_after_wildcard_restrict() {
    let (system, realm, project, user) = fresh_realm();
    let settings = vec![project, user];
    restrict(&system, &realm, &restrict_args("*"), &settings).unwrap();

    let projection = project_unprotect(&system, &realm, &unprotect_args("*")).unwrap();
    let diff = diff_or_fail(projection);

    assert_eq!(diff.absolute_path, realm);
    assert!(matches!(
        diff.remargin_yaml.entry_action,
        UnprotectEntryAction::WouldBeRemoved
    ));
    assert!(matches!(
        diff.sidecar.entry_action,
        UnprotectEntryAction::WouldBeRemoved
    ));
}

/// No `.claude/` ancestor returns a Reject with a clear message.
#[test]
fn no_anchor_returns_reject() {
    let cwd = PathBuf::from("/orphan");
    let system = MockSystem::new().with_dir(&cwd).unwrap();
    let projection = project_unprotect(&system, &cwd, &unprotect_args("foo")).unwrap();
    let reason = reject_or_fail(projection);
    assert!(
        reason.contains("/orphan") || reason.contains(".claude"),
        "reject reason should reference cwd / anchor: {reason}"
    );
}

/// Plan is read-only: every disk byte is identical before and after.
#[test]
fn plan_is_read_only() {
    let (system, realm, project, user) = fresh_realm();
    let settings = vec![project.clone(), user.clone()];
    restrict(&system, &realm, &restrict_args("src/secret"), &settings).unwrap();

    let yaml = realm.join(".remargin.yaml");
    let sidecar = realm.join(".claude/.remargin-restrictions.json");
    let watched: &[&Path] = &[&yaml, &project, &user, &sidecar];

    let before = snapshot(&system, watched);
    let _result: UnprotectProjection =
        project_unprotect(&system, &realm, &unprotect_args("src/secret")).unwrap();
    let after = snapshot(&system, watched);

    for (idx, (path_b, body_b)) in before.iter().enumerate() {
        let (path_a, body_a) = &after[idx];
        assert_eq!(path_b, path_a);
        match (body_b, body_a) {
            (Ok(b), Ok(a)) => assert_eq!(b, a, "{} changed", path_b.display()),
            (Err(_), Err(_)) => {}
            (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
                let _: () = unreachable_existence_flip(path_b);
            }
        }
    }
}

#[track_caller]
fn unreachable_existence_flip(path: &Path) {
    assert_eq!(path.display().to_string(), "<existence flipped>");
}

/// Idempotent observation: running plan twice in a row produces the
/// same conflict set, entry actions, and rule lists.
#[test]
fn plan_twice_in_a_row_is_idempotent() {
    let (system, realm, project, user) = fresh_realm();
    let settings = vec![project, user];
    restrict(&system, &realm, &restrict_args("src/secret"), &settings).unwrap();

    let first =
        diff_or_fail(project_unprotect(&system, &realm, &unprotect_args("src/secret")).unwrap());
    let second =
        diff_or_fail(project_unprotect(&system, &realm, &unprotect_args("src/secret")).unwrap());

    assert_eq!(first.absolute_path, second.absolute_path);
    assert_eq!(
        first.remargin_yaml.entry_action,
        second.remargin_yaml.entry_action
    );
    assert_eq!(first.sidecar.entry_action, second.sidecar.entry_action);
    assert_eq!(first.settings_files.len(), second.settings_files.len());
    for (a, b) in first
        .settings_files
        .iter()
        .zip(second.settings_files.iter())
    {
        assert_eq!(a.path, b.path);
        assert_eq!(a.rules_to_remove, b.rules_to_remove);
        assert_eq!(a.rules_already_absent, b.rules_already_absent);
    }
}
