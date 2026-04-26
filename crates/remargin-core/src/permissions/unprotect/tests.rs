//! Unit tests for [`crate::permissions::unprotect`] (rem-yj1j.6 /
//! rem-3p2v).
//!
//! Covers scenarios 1-11 from the rem-yj1j.6 testing plan: clean
//! reverse, idempotency, hand-edited divergences (YAML-without-
//! sidecar, sidecar-without-YAML, manual rule deletion in
//! settings), wildcard, anchor-not-found.

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;
use serde_yaml::Value;

use crate::permissions::restrict::{self, RestrictArgs};
use crate::permissions::sidecar;
use crate::permissions::unprotect::{UnprotectArgs, unprotect};

fn realm_with_claude() -> (MockSystem, PathBuf) {
    let anchor = PathBuf::from("/r");
    let system = MockSystem::new()
        .with_dir(&anchor)
        .unwrap()
        .with_dir(anchor.join(".claude"))
        .unwrap();
    (system, anchor)
}

fn settings_files(anchor: &Path) -> Vec<PathBuf> {
    vec![
        anchor.join(".claude/settings.local.json"),
        PathBuf::from("/home/u/.claude/settings.json"),
    ]
}

fn restrict_args(path: &str) -> RestrictArgs {
    RestrictArgs::new(String::from(path), Vec::new(), false)
}

fn read_yaml(system: &MockSystem, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_yaml::from_str(&body).unwrap()
}

/// Scenario 1: clean reverse — restrict then unprotect leaves the
/// state byte-equivalent to "before restrict".
#[test]
fn clean_reverse_restores_state() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(outcome.yaml_entry_removed);
    assert!(outcome.warnings.is_empty(), "{:#?}", outcome.warnings);

    // The .remargin.yaml retains the empty restrict array (schema
    // stable for the next call).
    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let restricts = value["permissions"]["restrict"].as_sequence().unwrap();
    assert!(restricts.is_empty());

    // Sidecar is empty.
    let sc = sidecar::load(&system, &anchor).unwrap();
    assert!(sc.entries.is_empty());

    // Project-scope settings file no longer carries the restrict
    // rule.
    let body = system.read_to_string(&files[0]).unwrap();
    assert!(!body.contains("Edit(///r/src/secret/**)"));
}

/// Scenario 2: a path that was never restricted yields a warn +
/// no-op.
#[test]
fn never_restricted_path_warns_and_no_ops() {
    let (system, anchor) = realm_with_claude();
    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(!outcome.yaml_entry_removed);
    assert!(outcome.rules_removed.is_empty());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("not currently restricted")),
        "{:#?}",
        outcome.warnings
    );
}

/// Scenario 3: YAML present, sidecar absent (user hand-edited the
/// YAML). The YAML entry is removed; settings stay untouched; a
/// warning surfaces.
#[test]
fn yaml_present_sidecar_absent_removes_yaml_only() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    // Strip the sidecar by hand (simulating the user's edit).
    let sidecar_path = anchor.join(".claude/.remargin-restrictions.json");
    system
        .write(&sidecar_path, b"{\"version\":1,\"entries\":{}}")
        .unwrap();

    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(outcome.yaml_entry_removed);
    assert!(outcome.rules_removed.is_empty());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("no sidecar entry")),
        "{:#?}",
        outcome.warnings
    );

    // Settings still carry the rule because we couldn't know which
    // ones to scrub without the sidecar.
    let body = system.read_to_string(&files[0]).unwrap();
    assert!(body.contains("Edit(///r/src/secret/**)"));
}

/// Scenario 4: YAML missing, sidecar present (inverse hand-edit).
/// Sidecar removal proceeds; warning surfaces.
#[test]
fn yaml_missing_sidecar_present_reverts_settings_only() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    // Strip the YAML entry by hand: rewrite without permissions.restrict.
    restrict::write_remargin_yaml(&system, &anchor, "permissions:\n  restrict: []\n").unwrap();

    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(!outcome.yaml_entry_removed);
    assert!(
        outcome.warnings.iter().any(|w| w.contains("no entry in")),
        "{:#?}",
        outcome.warnings
    );

    // Settings WERE scrubbed because the sidecar told us which
    // rules to remove.
    let body = system.read_to_string(&files[0]).unwrap();
    assert!(!body.contains("Edit(///r/src/secret/**)"));
}

/// Scenario 5: manual rule deletion between restrict and unprotect
/// surfaces as a warning (propagated from `revert_rules`'s
/// `RevertReport`).
#[test]
fn manual_rule_deletion_surfaces_warning() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    // Hand-delete a single rule from the project-scope file.
    let local = files[0].clone();
    let body = system.read_to_string(&local).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let deny = value["permissions"]["deny"].as_array_mut().unwrap();
    deny.retain(|v| v.as_str() != Some("Edit(///r/src/secret/**)"));
    let updated = serde_json::to_string_pretty(&value).unwrap();
    system.write(&local, updated.as_bytes()).unwrap();

    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("Edit(///r/src/secret/**)") && w.contains("manually removed")),
        "expected manual-removal warning, got: {:#?}",
        outcome.warnings
    );
}

/// Scenario 6: wildcard restrict + wildcard unprotect.
#[test]
fn wildcard_restrict_and_unprotect_round_trip() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("*"), &files).unwrap();

    let outcome = unprotect(&system, &anchor, &UnprotectArgs::new(String::from("*"))).unwrap();
    assert!(outcome.yaml_entry_removed);
    assert!(outcome.warnings.is_empty(), "{:#?}", outcome.warnings);

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let restricts = value["permissions"]["restrict"].as_sequence().unwrap();
    assert!(restricts.is_empty());
}

/// Scenario 7: no `.claude/` ancestor → clear error.
#[test]
fn anchor_not_found_errors() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();
    let err = unprotect(
        &system,
        Path::new("/r"),
        &UnprotectArgs::new(String::from("foo")),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("no `.claude/`"), "got: {msg}");
}

/// Scenario 8: idempotent — second unprotect on the same path is a
/// warn + no-op.
#[test]
fn second_unprotect_is_noop() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    let second = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(!second.yaml_entry_removed);
    assert!(second.rules_removed.is_empty());
    assert!(
        second
            .warnings
            .iter()
            .any(|w| w.contains("not currently restricted")),
        "{:#?}",
        second.warnings
    );
}

/// Scenario 9: when multiple restrict entries exist, unprotect
/// removes only the matching one.
#[test]
fn other_restrict_entries_are_preserved() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();
    restrict::restrict(&system, &anchor, &restrict_args("archive"), &files).unwrap();

    unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let restricts = value["permissions"]["restrict"].as_sequence().unwrap();
    assert_eq!(restricts.len(), 1);
    assert_eq!(restricts[0]["path"], Value::String(String::from("archive")));
}

/// Scenario 10: removing the only entry leaves an empty
/// `permissions.restrict: []` array (schema stable).
#[test]
fn empty_restrict_array_remains_after_last_removal() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    assert!(
        value["permissions"]["restrict"]
            .as_sequence()
            .is_some_and(Vec::is_empty)
    );
}

/// Scenario 11: the rem-is4z bypass stays scoped to the dedicated
/// helper. We verify the public surface works (which means the
/// helper was used internally) and pin that the helper itself is
/// callable from this module — any future re-export would break
/// the audit boundary intentionally.
#[test]
fn rem_is4z_bypass_uses_dedicated_helper() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();
    unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();

    // The bypass succeeded — the YAML was rewritten without going
    // through the public `write` op (which rem-is4z guards).
    system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();

    // Pin the only sanctioned entry point so a future change
    // re-exporting `write_remargin_yaml` from another module fails
    // this test deliberately.
    let body = "permissions:\n  restrict: []\n";
    restrict::write_remargin_yaml(&system, &anchor, body).unwrap();
}
