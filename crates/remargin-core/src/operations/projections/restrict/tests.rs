//! Unit tests for [`crate::operations::projections::restrict::project_restrict`]
//! (rem-puy5).

use std::io;
use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::operations::plan::{
    ConfigConflict, ConfigPlanDiff, EntryAction, RemarginYamlDiff, SidecarDiff,
};
use crate::operations::projections::restrict::{RestrictProjection, project_restrict};
use crate::permissions::restrict::{RestrictArgs, restrict};

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

fn restrict_args(path: &str) -> RestrictArgs {
    RestrictArgs::new(String::from(path), Vec::new(), false)
}

#[track_caller]
fn diff_or_fail(projection: RestrictProjection) -> Box<ConfigPlanDiff> {
    match projection {
        RestrictProjection::Diff(diff) => diff,
        RestrictProjection::Reject(reason) => {
            assert_eq!(reason, "<expected RestrictProjection::Diff>");
            Box::new(ConfigPlanDiff {
                absolute_path: PathBuf::new(),
                anchor: PathBuf::new(),
                conflicts: Vec::new(),
                remargin_yaml: RemarginYamlDiff {
                    entry_action: EntryAction::Noop,
                    path: PathBuf::new(),
                    previous_entry: None,
                    projected_entry: None,
                    will_be_created: false,
                },
                settings_files: Vec::new(),
                sidecar: SidecarDiff {
                    entry_action: EntryAction::Noop,
                    path: PathBuf::new(),
                    will_be_created: false,
                },
            })
        }
    }
}

#[track_caller]
fn reject_or_fail(projection: RestrictProjection) -> String {
    match projection {
        RestrictProjection::Reject(reason) => reason,
        RestrictProjection::Diff(_diff) => {
            assert_eq!(String::new(), "<expected RestrictProjection::Reject>");
            String::new()
        }
    }
}

fn write_settings_file(system: MockSystem, path: &Path, body: &str) -> MockSystem {
    let parent = path.parent().unwrap_or(path);
    system
        .with_dir(parent)
        .unwrap()
        .with_file(path, body.as_bytes())
        .unwrap()
}

#[test]
fn anchor_at_cwd_with_empty_state_projects_added() {
    let (system, realm, project, user) = fresh_realm();
    let args = restrict_args("src/secret");

    let projection =
        project_restrict(&system, &realm, &args, &[project.clone(), user.clone()]).unwrap();
    let diff = diff_or_fail(projection);

    assert_eq!(diff.anchor, realm);
    assert_eq!(diff.absolute_path, realm.join("src/secret"));
    assert!(diff.remargin_yaml.will_be_created);
    assert!(matches!(
        diff.remargin_yaml.entry_action,
        EntryAction::Added
    ));
    assert!(diff.settings_files[0].will_be_created);
    assert!(!diff.settings_files[0].deny_rules_to_add.is_empty());
    assert!(matches!(diff.sidecar.entry_action, EntryAction::Added));
    assert!(
        !diff
            .conflicts
            .iter()
            .any(|c| matches!(c, ConfigConflict::AnchorIsAncestor { .. }))
    );
    let _: io::Error = system
        .read_to_string(&realm.join(".remargin.yaml"))
        .unwrap_err();
    let _: io::Error = system.read_to_string(&project).unwrap_err();
    let _: io::Error = system.read_to_string(&user).unwrap_err();
}

#[test]
fn anchor_is_ancestor_when_cwd_is_subdirectory() {
    let (system, realm, project, user) = fresh_realm();
    let cwd = realm.join("sub");
    let system_with_sub = system.with_dir(&cwd).unwrap();
    let args = restrict_args("sub/secret");

    let projection = project_restrict(&system_with_sub, &cwd, &args, &[project, user]).unwrap();
    let diff = diff_or_fail(projection);

    assert!(
        diff.conflicts
            .iter()
            .any(|c| matches!(c, ConfigConflict::AnchorIsAncestor { .. })),
        "expected AnchorIsAncestor in {:?}",
        diff.conflicts
    );
}

#[test]
fn no_anchor_returns_reject() {
    let cwd = PathBuf::from("/orphan");
    let system = MockSystem::new().with_dir(&cwd).unwrap();
    let args = restrict_args("foo");

    let projection = project_restrict(
        &system,
        &cwd,
        &args,
        &[
            cwd.join(".claude/settings.local.json"),
            PathBuf::from("/home/u/.claude/settings.json"),
        ],
    )
    .unwrap();
    let reason = reject_or_fail(projection);
    assert!(
        reason.contains("/orphan") || reason.contains(".claude"),
        "reject reason should reference the cwd / anchor: {reason}"
    );
}

#[test]
fn path_outside_anchor_returns_reject() {
    let (system, realm, project, user) = fresh_realm();
    let args = restrict_args("../escape");

    let projection = project_restrict(&system, &realm, &args, &[project, user]).unwrap();
    let reason = reject_or_fail(projection);
    assert!(
        reason.contains("outside the anchor"),
        "expected outside-anchor reject: {reason}"
    );
}

#[test]
fn wildcard_resolves_to_anchor_and_emits_realm_rules() {
    let (system, realm, project, user) = fresh_realm();
    let args = restrict_args("*");

    let projection = project_restrict(&system, &realm, &args, &[project, user]).unwrap();
    let diff = diff_or_fail(projection);
    assert_eq!(diff.absolute_path, realm);
    let realm_str = realm.display().to_string();
    assert!(
        diff.settings_files[0]
            .deny_rules_to_add
            .iter()
            .any(|r| r.contains(&realm_str) && r.contains("/**")),
        "expected realm-wide deny rule, got {:?}",
        diff.settings_files[0].deny_rules_to_add,
    );
}

#[test]
fn yaml_noop_on_repeated_call_with_same_args() {
    let (system, realm, project, user) = fresh_realm();
    let args = restrict_args("src/secret");
    let settings = vec![project, user];

    restrict(&system, &realm, &args, &settings).unwrap();

    let projection = project_restrict(&system, &realm, &args, &settings).unwrap();
    let diff = diff_or_fail(projection);

    assert!(matches!(diff.remargin_yaml.entry_action, EntryAction::Noop));
    assert!(matches!(diff.sidecar.entry_action, EntryAction::Noop));
    for sf in &diff.settings_files {
        assert!(
            sf.deny_rules_to_add.is_empty(),
            "second projection should have nothing to add: {:?}",
            sf.deny_rules_to_add,
        );
    }
}

#[test]
fn yaml_entry_change_surfaces_conflict_with_previous() {
    let (system, realm, project, user) = fresh_realm();
    let initial = restrict_args("src/secret");
    let settings = vec![project, user];

    restrict(&system, &realm, &initial, &settings).unwrap();

    let updated = RestrictArgs::new(String::from("src/secret"), Vec::new(), true);
    let projection = project_restrict(&system, &realm, &updated, &settings).unwrap();
    let diff = diff_or_fail(projection);

    let saw_yaml = diff.conflicts.iter().any(|c| {
        matches!(
            c,
            ConfigConflict::YamlEntryWouldChange { previous, .. } if !previous.cli_allowed
        )
    });
    assert!(
        saw_yaml,
        "expected YamlEntryWouldChange in {:?}",
        diff.conflicts
    );
}

#[test]
fn allow_deny_overlap_surfaces_when_existing_allow_matches_projected_deny() {
    let (system, realm, project, user) = fresh_realm();
    let secret_glob = format!("{}/src/secret/**", realm.display());
    let body = serde_json::json!({
        "permissions": {
            "allow": [format!("Read(//{secret_glob})")],
            "deny": []
        }
    });
    let seeded = write_settings_file(system, &user, &body.to_string());

    let args = restrict_args("src/secret");
    let projection = project_restrict(&seeded, &realm, &args, &[project, user.clone()]).unwrap();
    let diff = diff_or_fail(projection);

    let saw_overlap = diff.conflicts.iter().any(|c| {
        matches!(
            c,
            ConfigConflict::AllowDenyOverlap { settings_file, .. } if settings_file == &user
        )
    });
    assert!(
        saw_overlap,
        "expected AllowDenyOverlap on user-scope file. conflicts: {:?}, sims: {:?}",
        diff.conflicts, diff.settings_files,
    );
}

#[test]
fn project_restrict_does_not_write_to_disk() {
    let (system, realm, project, user) = fresh_realm();
    let args = restrict_args("src/secret");
    let yaml_path = realm.join(".remargin.yaml");

    let _projection =
        project_restrict(&system, &realm, &args, &[project.clone(), user.clone()]).unwrap();

    let _: io::Error = system.read_to_string(&yaml_path).unwrap_err();
    let _: io::Error = system.read_to_string(&project).unwrap_err();
    let _: io::Error = system.read_to_string(&user).unwrap_err();
    let _: io::Error = system
        .read_to_string(&realm.join(".claude/.remargin-restrictions.json"))
        .unwrap_err();
}
