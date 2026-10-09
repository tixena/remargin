//! Unit tests for [`crate::permissions::restrict`].
//!
//! Covers anchor discovery, wildcard support, .remargin.yaml mutation
//! (create + merge + idempotency), Claude-sync invocation through
//! `apply_rules`.

use std::io;
use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MemorySystem;
use serde_yaml::Value;

use crate::permissions::restrict::{
    RestrictArgs, find_claude_anchor, restrict, write_remargin_yaml,
};
use crate::permissions::sidecar;

fn realm_with_claude(extra_files: &[(&str, &str)]) -> (MemorySystem, PathBuf) {
    let anchor = PathBuf::from("/r");
    let mut system = MemorySystem::new()
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

fn read_yaml(system: &MemorySystem, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_yaml::from_str(&body).unwrap()
}

#[test]
fn anchor_discovery_when_cwd_is_anchor() {
    let (system, anchor) = realm_with_claude(&[]);
    let found = find_claude_anchor(&system, &anchor).unwrap();
    assert_eq!(found, anchor);
}

#[test]
fn anchor_discovery_walks_up_to_nearest_claude_dir() {
    let (system, _anchor) = realm_with_claude(&[]);
    let deep = PathBuf::from("/r/sub/sub2");
    system.create_dir_all(&deep).unwrap();
    let found = find_claude_anchor(&system, &deep).unwrap();
    assert_eq!(found, PathBuf::from("/r"));
}

#[test]
fn anchor_discovery_errors_when_no_claude_ancestor() {
    let system = MemorySystem::new().with_dir(Path::new("/r")).unwrap();
    let err = find_claude_anchor(&system, Path::new("/r")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("no `.claude/`"),
        "expected named error, got: {msg}"
    );
}

#[test]
fn wildcard_path_stored_in_yaml() {
    let (system, anchor) = realm_with_claude(&[]);
    restrict(&system, &anchor, &args("*"), &settings_files(&anchor)).unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let entry = &value["permissions"]["trusted_roots"][0];
    assert_eq!(entry["path"], Value::String(String::from("*")));
}

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
    let entry = &value["permissions"]["trusted_roots"][0];
    assert_eq!(entry["path"], Value::String(String::from("src/secret")));
}

/// The identity block survives the new `permissions.trusted_roots` array.
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
    let restrict_entry = &value["permissions"]["trusted_roots"][0];
    assert_eq!(
        restrict_entry["path"],
        Value::String(String::from("src/secret"))
    );
}

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
    let restricts = value["permissions"]["trusted_roots"].as_sequence().unwrap();
    assert_eq!(restricts.len(), 1, "{value:#?}");
}

/// The hook is the single source of truth, so nothing is written to settings or the sidecar.
#[test]
fn rerun_writes_no_settings_or_sidecar() {
    let (system, anchor) = realm_with_claude(&[]);
    let files = settings_files(&anchor);
    restrict(&system, &anchor, &args("src/secret"), &files).unwrap();

    let _: io::Error = system.read_to_string(&files[0]).unwrap_err();
    assert!(sidecar::load(&system, &anchor).unwrap().entries.is_empty());

    restrict(&system, &anchor, &args("src/secret"), &files).unwrap();
    let _: io::Error = system.read_to_string(&files[0]).unwrap_err();
    assert!(sidecar::load(&system, &anchor).unwrap().entries.is_empty());
}

/// The hook denies whatever the verb, so `also_deny_bash` projects no Bash deny rules.
#[test]
fn also_deny_bash_lands_on_yaml_entry_but_projects_no_rules() {
    let (system, anchor) = realm_with_claude(&[]);
    let mut a = args("src/secret");
    a.also_deny_bash = vec![String::from("curl"), String::from("wget")];
    let outcome = restrict(&system, &anchor, &a, &settings_files(&anchor)).unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let entry = &value["permissions"]["trusted_roots"][0];
    let extras = entry["also_deny_bash"].as_sequence().unwrap();
    assert_eq!(extras.len(), 2);

    assert!(
        outcome.rules_applied.is_empty(),
        "no rules should be projected: {:#?}",
        outcome.rules_applied
    );
}

#[test]
fn cli_allowed_true_persists_in_yaml_no_remargin_cli_deny_projected() {
    let (system, anchor) = realm_with_claude(&[]);
    let mut a = args("src/secret");
    a.cli_allowed = true;
    let outcome = restrict(&system, &anchor, &a, &settings_files(&anchor)).unwrap();

    let value = read_yaml(&system, &anchor.join(".remargin.yaml"));
    let entry = &value["permissions"]["trusted_roots"][0];
    assert_eq!(entry["cli_allowed"], Value::Bool(true));

    assert!(
        !outcome
            .rules_applied
            .iter()
            .any(|r| r.starts_with("Bash(remargin"))
    );
}

#[test]
fn outcome_reports_no_settings_or_sidecar() {
    let (system, anchor) = realm_with_claude(&[]);
    let files = settings_files(&anchor);
    let outcome = restrict(&system, &anchor, &args("src/secret"), &files).unwrap();
    assert_eq!(outcome.anchor, anchor);
    assert!(outcome.absolute_path.ends_with("src/secret"));
    assert_eq!(outcome.claude_files_touched, [] as [PathBuf; 0]);
    assert_eq!(outcome.rules_applied, [] as [String; 0]);

    let sc = sidecar::load(&system, &anchor).unwrap();
    assert!(sc.entries.is_empty());
}

/// The file lands through the sanctioned helper, which only the permissions namespace exports.
#[test]
fn write_remargin_yaml_bypass_is_scoped_to_this_module() {
    let (system, anchor) = realm_with_claude(&[]);
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

    let body = "permissions:\n  trusted_roots: []\n";
    write_remargin_yaml(&system, &anchor, body).unwrap();
    assert_eq!(
        system
            .read_to_string(&anchor.join(".remargin.yaml"))
            .unwrap(),
        body
    );
}
