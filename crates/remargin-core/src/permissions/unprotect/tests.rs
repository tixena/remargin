//! Unit tests for [`crate::permissions::unprotect`].
//!
//! Covers clean reverse, idempotency, hand-edited divergences (YAML-
//! without-sidecar, sidecar-without-YAML, manual rule deletion in
//! settings), wildcard, anchor-not-found.

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MemorySystem;
use serde_yaml::Value;

use crate::permissions::claude_sync::{RuleSet, apply_rules};
use crate::permissions::restrict::{self, RestrictArgs};
use crate::permissions::sidecar;
use crate::permissions::unprotect::{UnprotectArgs, unprotect};

/// Restrict `path`, then seed the settings files and the sidecar with a deny set, so the
/// sidecar-driven reverse has something to scrub. The current `restrict` writes neither.
fn restrict_with_legacy_sidecar(
    system: &MemorySystem,
    anchor: &Path,
    path: &str,
    settings: &[PathBuf],
) {
    restrict::restrict(system, anchor, &restrict_args(path), settings).unwrap();
    let absolute = if path == "*" {
        anchor.to_path_buf()
    } else {
        anchor.join(path)
    };
    let glob = absolute.display();
    let rules = RuleSet {
        allow: Vec::new(),
        deny: vec![format!("Edit({glob}/**)"), format!("Write({glob}/**)")],
    };
    apply_rules(
        system,
        anchor,
        &absolute.display().to_string(),
        &rules,
        settings,
        "legacy",
    )
    .unwrap();
}

fn realm_with_claude() -> (MemorySystem, PathBuf) {
    let anchor = PathBuf::from("/r");
    let system = MemorySystem::new()
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

fn read_yaml(system: &MemorySystem, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_yaml::from_str(&body).unwrap()
}

/// A realm carrying projected rules is scrubbed back to its state before `restrict`.
#[test]
fn clean_reverse_restores_state() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict_with_legacy_sidecar(&system, &anchor, "src/secret", &files);

    let outcome = unprotect(
        &system,
        &anchor,
        &UnprotectArgs::new(String::from("src/secret")),
    )
    .unwrap();
    assert!(outcome.yaml_entry_removed);
    assert!(outcome.warnings.is_empty(), "{:#?}", outcome.warnings);

    let yaml_body = system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();
    assert!(
        !yaml_body.contains("permissions:") && !yaml_body.contains("restrict:"),
        ".remargin.yaml should be compacted after the last restrict is removed: {yaml_body}",
    );

    let sc = sidecar::load(&system, &anchor).unwrap();
    assert!(sc.entries.is_empty());

    let settings_body = system.read_to_string(&files[0]).unwrap();
    assert!(
        !settings_body.contains("src/secret"),
        "settings should no longer reference src/secret after unprotect: {settings_body}"
    );
    assert!(!settings_body.contains("Bash(remargin *)"));
}

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
    assert_eq!(outcome.rules_removed, [] as [String; 0]);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("not currently restricted")),
        "{:#?}",
        outcome.warnings
    );
}

/// The YAML entry is removed, the settings stay untouched, and a warning surfaces.
#[test]
fn yaml_present_sidecar_absent_removes_yaml_only() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict_with_legacy_sidecar(&system, &anchor, "src/secret", &files);

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
    assert_eq!(outcome.rules_removed, [] as [String; 0]);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("no sidecar entry")),
        "{:#?}",
        outcome.warnings
    );

    let body = system.read_to_string(&files[0]).unwrap();
    assert!(
        body.contains("Edit(") && body.contains("src/secret"),
        "settings should still carry the Edit deny (sidecar absent, rules not scrubbed): {body}"
    );
}

/// The sidecar removal proceeds and a warning surfaces.
#[test]
fn yaml_missing_sidecar_present_reverts_settings_only() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict_with_legacy_sidecar(&system, &anchor, "src/secret", &files);

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

    let body = system.read_to_string(&files[0]).unwrap();
    assert!(
        !body.contains("src/secret"),
        "settings should no longer reference src/secret after sidecar-guided revert: {body}"
    );
    assert!(!body.contains("Bash(remargin *)"));
}

#[test]
fn manual_rule_deletion_surfaces_warning() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict_with_legacy_sidecar(&system, &anchor, "src/secret", &files);

    let local = files[0].clone();
    let body = system.read_to_string(&local).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let deny = value["permissions"]["deny"].as_array_mut().unwrap();
    let edit_rule = deny
        .iter()
        .find(|v| {
            v.as_str()
                .is_some_and(|s| s.starts_with("Edit(") && s.contains("src/secret"))
        })
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap();
    deny.retain(|v| v.as_str() != Some(edit_rule.as_str()));
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
            .any(|w| w.contains(edit_rule.as_str()) && w.contains("manually removed")),
        "expected manual-removal warning for {edit_rule:?}, got: {:#?}",
        outcome.warnings
    );
}

#[test]
fn wildcard_restrict_and_unprotect_round_trip() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict_with_legacy_sidecar(&system, &anchor, "*", &files);

    let outcome = unprotect(&system, &anchor, &UnprotectArgs::new(String::from("*"))).unwrap();
    assert!(outcome.yaml_entry_removed);
    assert!(outcome.warnings.is_empty(), "{:#?}", outcome.warnings);

    let body = system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();
    assert!(
        !body.contains("permissions:") && !body.contains("restrict:"),
        "wildcard unprotect should compact the YAML: {body}",
    );
}

#[test]
fn anchor_not_found_errors() {
    let system = MemorySystem::new().with_dir(Path::new("/r")).unwrap();
    let err = unprotect(
        &system,
        Path::new("/r"),
        &UnprotectArgs::new(String::from("foo")),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("no `.claude/`"), "got: {msg}");
}

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
    assert_eq!(second.rules_removed, [] as [String; 0]);
    assert!(
        second
            .warnings
            .iter()
            .any(|w| w.contains("not currently restricted")),
        "{:#?}",
        second.warnings
    );
}

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

/// Only the empty array is pruned; the rest of the `permissions:` block stays.
#[test]
fn last_restrict_removal_keeps_other_permissions_subkeys() {
    let (system, anchor) = realm_with_claude();
    let files = settings_files(&anchor);
    restrict::restrict(&system, &anchor, &restrict_args("src/secret"), &files).unwrap();

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

/// A hand-left empty array is compacted on the next call, even when no entry matches.
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

/// The public surface works through the dedicated helper, which stays callable only from here.
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

    system
        .read_to_string(&anchor.join(".remargin.yaml"))
        .unwrap();

    let body = "permissions:\n  trusted_roots: []\n";
    restrict::write_remargin_yaml(&system, &anchor, body).unwrap();
}
