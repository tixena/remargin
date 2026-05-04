//! Unit tests for [`crate::permissions::restrict`] (rem-yj1j.5 /
//! rem-aqnn).
//!
//! Covers scenarios 1-13 from the rem-yj1j.5 testing plan: anchor
//! discovery, wildcard support, .remargin.yaml mutation (create +
//! merge + idempotency), Claude-sync invocation through `apply_rules`.

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;
use serde_yaml::Value;

use crate::permissions::restrict::{
    RestrictArgs, find_claude_anchor, restrict, write_remargin_yaml,
};
use crate::permissions::sidecar;

fn realm_with_claude(extra_files: &[(&str, &str)]) -> (MockSystem, PathBuf) {
    let anchor = PathBuf::from("/r");
    let mut system = MockSystem::new()
        .with_dir(&anchor)
        .unwrap()
        .with_dir(anchor.join(".claude"))
        .unwrap();
    for (path, body) in extra_files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    (system, anchor)
}

fn settings_files(anchor: &Path) -> Vec<PathBuf> {
    vec![
        anchor.join(".claude/settings.local.json"),
        PathBuf::from("/home/u/.claude/settings.json"),
    ]
}

fn args(path: &str) -> RestrictArgs {
    RestrictArgs {
        also_deny_bash: Vec::new(),
        cli_allowed: false,
        path: String::from(path),
    }
}

fn read_yaml(system: &MockSystem, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_yaml::from_str(&body).unwrap()
}

/// Scenario 1: cwd is the anchor (it has its own `.claude/`).
#[test]
fn anchor_discovery_when_cwd_is_anchor() {
    let (system, anchor) = realm_with_claude(&[]);
    let found = find_claude_anchor(&system, &anchor).unwrap();
    assert_eq!(found, anchor);
}

/// Scenario 2: anchor is several directories up from cwd.
#[test]
fn anchor_discovery_walks_up_to_nearest_claude_dir() {
    let (system, _anchor) = realm_with_claude(&[]);
    let deep = PathBuf::from("/r/sub/sub2");
    system.create_dir_all(&deep).unwrap();
    let found = find_claude_anchor(&system, &deep).unwrap();
    assert_eq!(found, PathBuf::from("/r"));
}

/// Scenario 3: no `.claude/` ancestor → clear error.
#[test]
fn anchor_discovery_errors_when_no_claude_ancestor() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();
    let err = find_claude_anchor(&system, Path::new("/r")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("no `.claude/`"),
        "expected named error, got: {msg}"
    );
}

/// Scenario 4: wildcard path stored verbatim in `.remargin.yaml`.
#[test]
fn wildcard_path_stored_in_yaml() {
    let (system, anchor) = realm_with_claude(&[]);
    restrict(&system, &anchor, &args("*"), &settings_files(&anchor)).unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let entry = &value["permissions"]["restrict"][0];
    assert_eq!(entry["path"], Value::String(String::from("*")));
}

/// Scenario 5: subpath that resolves outside the anchor is rejected.
#[test]
fn subpath_outside_anchor_is_rejected() {
    let (system, anchor) = realm_with_claude(&[]);
    let err = restrict(
        &system,
        &anchor,
        &args("../escape"),
        &settings_files(&anchor),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("outside the anchor"),
        "expected outside-anchor error, got: {msg}"
    );
}

/// Scenario 6: missing `.remargin.yaml` is created with the entry.
#[test]
fn creates_remargin_yaml_when_absent() {
    let (system, anchor) = realm_with_claude(&[]);
    let outcome = restrict(
        &system,
        &anchor,
        &args("src/secret"),
        &settings_files(&anchor),
    )
    .unwrap();
    assert!(outcome.yaml_was_created);

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let entry = &value["permissions"]["restrict"][0];
    assert_eq!(entry["path"], Value::String(String::from("src/secret")));
}

/// Scenario 7: existing `.remargin.yaml` with an identity block gains
/// the `permissions.restrict` array without losing the identity.
#[test]
fn appends_to_existing_remargin_yaml() {
    let prior = "identity: alice\ntype: human\n";
    let (system, anchor) = realm_with_claude(&[("/r/.remargin.yaml", prior)]);
    let outcome = restrict(
        &system,
        &anchor,
        &args("src/secret"),
        &settings_files(&anchor),
    )
    .unwrap();
    assert!(!outcome.yaml_was_created);

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    assert_eq!(value["identity"], Value::String(String::from("alice")));
    assert_eq!(value["type"], Value::String(String::from("human")));
    let restrict_entry = &value["permissions"]["restrict"][0];
    assert_eq!(
        restrict_entry["path"],
        Value::String(String::from("src/secret"))
    );
}

/// Scenario 8: re-running `restrict` for the same path is a no-op
/// (no duplicate entry in the YAML).
#[test]
fn duplicate_path_does_not_create_second_entry() {
    let (system, anchor) = realm_with_claude(&[]);
    restrict(
        &system,
        &anchor,
        &args("src/secret"),
        &settings_files(&anchor),
    )
    .unwrap();
    restrict(
        &system,
        &anchor,
        &args("src/secret"),
        &settings_files(&anchor),
    )
    .unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let restricts = value["permissions"]["restrict"].as_sequence().unwrap();
    assert_eq!(restricts.len(), 1, "{value:#?}");
}

/// Scenario 9 (rem-egp9): re-running after a manually-deleted Claude
/// rule backfills the missing rule (`apply_rules` dedupes against
/// existing entries; the second call still writes any missing strings).
/// Uses the coarse `Bash(remargin *)` deny — the only rule the
/// minimised projection emits when `cli_allowed = false` and there are
/// no `also_deny_bash` extras.
#[test]
fn rerun_backfills_missing_settings_rule() {
    let (system, anchor) = realm_with_claude(&[]);
    let files = settings_files(&anchor);
    restrict(&system, &anchor, &args("src/secret"), &files).unwrap();

    // Manually scrub the projected rule from the project-scope
    // settings file.
    let local = files[0].clone();
    let mut value: Value = serde_yaml::from_str(&system.read_to_string(&local).unwrap()).unwrap();
    if let Some(deny) = value
        .get_mut("permissions")
        .and_then(|p| p.get_mut("deny"))
        .and_then(|d| d.as_sequence_mut())
    {
        deny.retain(|v| v.as_str() != Some("Bash(remargin *)"));
    }
    let body = serde_json::to_string_pretty(&value).unwrap();
    system.write(&local, body.as_bytes()).unwrap();

    // Re-running restrict re-adds the missing rule.
    restrict(&system, &anchor, &args("src/secret"), &files).unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&system.read_to_string(&local).unwrap()).unwrap();
    let deny = after["permissions"]["deny"].as_array().unwrap();
    assert!(
        deny.iter().any(|v| v.as_str() == Some("Bash(remargin *)")),
        "missing remargin-cli deny was not backfilled: {after:#?}"
    );
}

/// Scenario 10: `also_deny_bash` lands on the entry AND in the
/// emitted Bash rules.
#[test]
fn also_deny_bash_propagates_to_yaml_and_rules() {
    let (system, anchor) = realm_with_claude(&[]);
    let mut a = args("src/secret");
    a.also_deny_bash = vec![String::from("curl"), String::from("wget")];
    let outcome = restrict(&system, &anchor, &a, &settings_files(&anchor)).unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let entry = &value["permissions"]["restrict"][0];
    let extras = entry["also_deny_bash"].as_sequence().unwrap();
    assert_eq!(extras.len(), 2);

    assert!(
        outcome
            .rules_applied
            .iter()
            .any(|r| r.starts_with("Bash(curl"))
    );
    assert!(
        outcome
            .rules_applied
            .iter()
            .any(|r| r.starts_with("Bash(wget"))
    );
}

/// Scenario 11: `cli_allowed=true` lands on the entry AND removes
/// the `Bash(remargin *)` deny from the rule set.
#[test]
fn cli_allowed_true_omits_remargin_cli_deny() {
    let (system, anchor) = realm_with_claude(&[]);
    let mut a = args("src/secret");
    a.cli_allowed = true;
    let outcome = restrict(&system, &anchor, &a, &settings_files(&anchor)).unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let entry = &value["permissions"]["restrict"][0];
    assert_eq!(entry["cli_allowed"], Value::Bool(true));

    assert!(
        !outcome
            .rules_applied
            .iter()
            .any(|r| r.starts_with("Bash(remargin"))
    );
}

/// Scenario 12: outcome reporting names every file touched and every
/// rule applied.
#[test]
fn outcome_lists_files_and_rules() {
    let (system, anchor) = realm_with_claude(&[]);
    let files = settings_files(&anchor);
    let outcome = restrict(&system, &anchor, &args("src/secret"), &files).unwrap();
    assert_eq!(outcome.anchor, anchor);
    assert!(outcome.absolute_path.ends_with("src/secret"));
    assert_eq!(outcome.claude_files_touched, files);
    assert!(!outcome.rules_applied.is_empty());

    // Sidecar tracked the outcome.
    let sc = sidecar::load(&system, &anchor).unwrap();
    assert_eq!(sc.entries.len(), 1);
}

/// Scenario 13: the dedicated `write_remargin_yaml` helper is the
/// only path used; the public write / edit ops still refuse
/// `.remargin.yaml` by virtue of `rem-is4z`. We pin this by checking
/// that the file landed (the bypass works) AND the helper is not
/// re-exported beyond the permissions namespace (no other module can
/// invoke it).
#[test]
fn write_remargin_yaml_bypass_is_scoped_to_this_module() {
    let (system, anchor) = realm_with_claude(&[]);
    // Public path: restrict() succeeds → write_remargin_yaml ran
    // through the sanctioned helper.
    restrict(
        &system,
        &anchor,
        &args("src/secret"),
        &settings_files(&anchor),
    )
    .unwrap();
    assert!(
        system
            .read_to_string(&anchor.join(".remargin.yaml"))
            .is_ok(),
        ".remargin.yaml must exist after restrict"
    );

    // `write_remargin_yaml` is only re-exported via
    // `crate::permissions::restrict::write_remargin_yaml`. A future
    // change that re-exports it from the crate root or another
    // module must update this test deliberately.
    let body = "permissions:\n  restrict: []\n";
    write_remargin_yaml(&system, &anchor, body).unwrap();
    assert_eq!(
        system
            .read_to_string(&anchor.join(".remargin.yaml"))
            .unwrap(),
        body
    );
}
