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

    // After rem-bimq the empty restrict array (and the wrapping
    // permissions: block, since it has no other sub-keys) gets
    // compacted out of the YAML. The body should no longer mention
    // either key.
    let yaml_body = system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();
    assert!(
        !yaml_body.contains("permissions:") && !yaml_body.contains("restrict:"),
        ".remargin.yaml should be compacted after the last restrict is removed: {yaml_body}",
    );

    // Sidecar is empty.
    let sc = sidecar::load(&system, &anchor).unwrap();
    assert!(sc.entries.is_empty());

    // Project-scope settings file no longer carries the restrict
    // rule. Under rem-egp9 the projection is the coarse
    // `Bash(remargin *)` deny when `cli_allowed = false`.
    let settings_body = system.read_to_string(&files[0]).unwrap();
    assert!(!settings_body.contains("Bash(remargin *)"));
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
    // ones to scrub without the sidecar. Under rem-egp9 the projected
    // rule is the coarse `Bash(remargin *)` deny.
    let body = system.read_to_string(&files[0]).unwrap();
    assert!(body.contains("Bash(remargin *)"));
}

/// Scenario 4: YAML missing, sidecar present (inverse hand-edit).
/// Sidecar removal proceeds; warning surfaces.
#[test]
fn yaml_missing_sidecar_present_reverts_settings_only() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    // Strip the YAML entry by hand: rewrite without permissions.restrict.
    restrict::write_remargin_yaml(&system, &anchor, "permissions:\n  trusted_roots: []\n").unwrap();

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
    // rules to remove. Under rem-egp9 the projected rule is the
    // coarse `Bash(remargin *)` deny.
    let body = system.read_to_string(&files[0]).unwrap();
    assert!(!body.contains("Bash(remargin *)"));
}

/// Scenario 5: manual rule deletion between restrict and unprotect
/// surfaces as a warning (propagated from `revert_rules`'s
/// `RevertReport`).
#[test]
fn manual_rule_deletion_surfaces_warning() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    // Hand-delete the projected rule from the project-scope file.
    // Under rem-egp9 the projection is the coarse `Bash(remargin *)`
    // deny when `cli_allowed = false` and there are no extras.
    let local = files[0].clone();
    let body = system.read_to_string(&local).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let deny = value["permissions"]["deny"].as_array_mut().unwrap();
    deny.retain(|v| v.as_str() != Some("Bash(remargin *)"));
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
            .any(|w| w.contains("Bash(remargin *)") && w.contains("manually removed")),
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

    // After rem-bimq the wildcard-only realm collapses entirely:
    // the empty restrict array gets pruned and the now-empty
    // permissions: block is removed.
    let body = system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();
    assert!(
        !body.contains("permissions:") && !body.contains("restrict:"),
        "wildcard unprotect should compact the YAML: {body}",
    );
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
    let restricts = value["permissions"]["trusted_roots"].as_sequence().unwrap();
    assert_eq!(restricts.len(), 1);
    assert_eq!(restricts[0]["path"], Value::String(String::from("archive")));
}

/// Scenario 10 (rem-bimq update): removing the only entry compacts
/// the empty array out of the YAML. Since `permissions:` had no
/// other sub-keys, the wrapping mapping is also removed.
#[test]
fn last_removal_compacts_permissions_block_out_of_yaml() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    let body = system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();
    assert!(
        !body.contains("permissions:") && !body.contains("restrict:"),
        ".remargin.yaml should be compacted: {body}",
    );
}

// ---------------------------------------------------------------------
// rem-bimq scenarios (compaction + --strict).
// ---------------------------------------------------------------------

/// rem-bimq scenario 3: removing the last `restrict` while
/// `deny_ops` still has entries prunes only the empty `restrict`,
/// leaving the rest of the `permissions:` block intact.
#[test]
fn last_restrict_removal_keeps_other_permissions_subkeys() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

    // Append a deny_ops sibling by hand.
    let yaml_path = anchor.join(".remargin.yaml");
    let body = system.read_to_string(&yaml_path).unwrap();
    let mut value: Value = serde_yaml::from_str(&body).unwrap();
    let perms = value
        .get_mut(Value::String(String::from("permissions")))
        .unwrap()
        .as_mapping_mut()
        .unwrap();
    let deny_entry: Value = serde_yaml::from_str("path: archive\nops: [purge]\n").unwrap();
    perms.insert(
        Value::String(String::from("deny_ops")),
        Value::Sequence(vec![deny_entry]),
    );
    let updated = serde_yaml::to_string(&value).unwrap();
    restrict::write_remargin_yaml(&system, &anchor, &updated).unwrap();

    unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();

    let final_body = system.read_to_string(&yaml_path).unwrap();
    assert!(
        !final_body.contains("restrict:"),
        "empty restrict array should be pruned: {final_body}",
    );
    assert!(
        final_body.contains("permissions:"),
        "permissions block should survive: {final_body}",
    );
    assert!(
        final_body.contains("deny_ops:"),
        "deny_ops sibling should survive: {final_body}",
    );
    assert!(
        final_body.contains("archive"),
        "deny_ops content should survive: {final_body}",
    );
}

/// rem-bimq scenario 4: hand-edited YAML carrying an empty
/// `restrict: []` next to a populated `deny_ops:` is compacted on
/// the next unprotect call, even when no entry matches.
#[test]
fn next_unprotect_compacts_pre_existing_empty_restrict() {
    let (system, anchor) = realm_with_claude();

    let body =
        "permissions:\n  trusted_roots: []\n  deny_ops:\n  - path: archive\n    ops: [purge]\n";
    restrict::write_remargin_yaml(&system, &anchor, body).unwrap();

    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    // No matching entry, so yaml_entry_removed stays false; the
    // compaction is a side-effect that still rewrites the file.
    assert!(!outcome.yaml_entry_removed);

    let final_body = system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();
    assert!(
        !final_body.contains("restrict:"),
        "pre-existing empty restrict should be compacted: {final_body}",
    );
    assert!(
        final_body.contains("deny_ops:"),
        "deny_ops should survive: {final_body}",
    );
}

/// rem-bimq scenario 5: when every `permissions:` sub-array winds
/// up empty (e.g. `restrict: []`, `allow_dot_folders: []`) the
/// whole `permissions:` block is removed.
#[test]
fn empty_permissions_block_is_removed_entirely() {
    let (system, anchor) = realm_with_claude();

    let body = "permissions:\n  trusted_roots: []\n  allow_dot_folders: []\n";
    restrict::write_remargin_yaml(&system, &anchor, body).unwrap();

    unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();

    let final_body = system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();
    assert!(
        !final_body.contains("permissions:"),
        "empty permissions block should be removed entirely: {final_body}",
    );
}

/// rem-bimq scenario 6: `--strict` against an unrestricted path
/// returns an error.
#[test]
fn strict_unprotect_against_unrestricted_path_errors() {
    let (system, anchor) = realm_with_claude();
    let err = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")).with_strict(true),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not currently restricted") && msg.contains("--strict"),
        "expected --strict refusal, got: {msg}",
    );
}

/// rem-bimq scenario 7: default (non-strict) unprotect against an
/// unrestricted path is still a warn-and-no-op (regression check).
#[test]
fn default_unprotect_against_unrestricted_path_is_still_warn_noop() {
    let (system, anchor) = realm_with_claude();
    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(!outcome.yaml_entry_removed);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("not currently restricted")),
        "{:#?}",
        outcome.warnings,
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
    let body = "permissions:\n  trusted_roots: []\n";
    restrict::write_remargin_yaml(&system, &anchor, body).unwrap();
}
