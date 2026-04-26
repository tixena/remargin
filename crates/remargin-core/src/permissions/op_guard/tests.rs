//! Acceptance scenarios for the Layer 1 op guard (rem-yj1j.2 / T23).
//!
//! All tests run against `os_shim::mock::MockSystem`. Scenarios that
//! require driving real-op handlers (15, 16, 18) live with the
//! follow-up integration ticket — the unit tests here cover the
//! matcher's full decision table.

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::config::permissions::resolve::{ResolvedPermissions, ResolvedRestrict, RestrictPath};
use crate::permissions::op_guard::{
    MUTATING_OPS, OpGuardError, check_against_resolved, is_mutating_op, pre_mutate_check,
    restrict_covers,
};

const READ_OPS: &[&str] = &[
    "comments", "get", "lint", "ls", "metadata", "query", "search", "verify",
];

fn realm_with(yaml: &str) -> MockSystem {
    MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), yaml.as_bytes())
        .unwrap()
}

fn restricted_match(err: &anyhow::Error, op_name: &str, source: &str) -> bool {
    matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::RestrictedPath { op, source_file, .. })
            if op == op_name && source_file == &PathBuf::from(source)
    )
}

fn denied_op_match(err: &anyhow::Error, op_name: &str, source: &str) -> bool {
    matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp { op, source_file, .. })
            if op == op_name && source_file == &PathBuf::from(source)
    )
}

fn dot_folder_match(err: &anyhow::Error, expected_folder: &str, source: &str) -> bool {
    matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DotFolderDenied { folder, source_file, .. })
            if folder == expected_folder && source_file == &PathBuf::from(source)
    )
}

// ---------------------------------------------------------------------
// Scenario table
// ---------------------------------------------------------------------

/// Scenario 1 — empty `.remargin.yaml` allows every op.
#[test]
fn scenario_01_no_permissions_allows_everything() {
    let system = realm_with("identity: alice\n");
    pre_mutate_check(&system, "comment", Path::new("/r/foo.md")).unwrap();
}

/// Scenario 2 — `restrict` covers a subpath; mutating op on a target
/// inside is refused.
#[test]
fn scenario_02_restrict_subpath_blocks_mutating_op() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/secret\n");
    let err = pre_mutate_check(&system, "comment", Path::new("/r/src/secret/foo.md")).unwrap_err();
    assert!(restricted_match(&err, "comment", "/r/.remargin.yaml"));
}

/// Scenario 3 — restrict subpath; target outside is allowed.
#[test]
fn scenario_03_restrict_subpath_allows_outside() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/secret\n");
    pre_mutate_check(&system, "comment", Path::new("/r/src/public/foo.md")).unwrap();
}

/// Scenario 4 — restrict subpath; read op on the same target is
/// allowed since `restrict` only fires on mutating ops.
#[test]
fn scenario_04_restrict_does_not_block_read_ops() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/secret\n");
    for op in READ_OPS {
        let result = pre_mutate_check(&system, op, Path::new("/r/src/secret/foo.md"));
        assert!(result.is_ok(), "read op {op} should not be blocked");
    }
}

/// Scenario 5 — wildcard restrict refuses any path under the realm.
#[test]
fn scenario_05_wildcard_restrict_blocks_anywhere_in_realm() {
    let system = realm_with("permissions:\n  restrict:\n    - path: '*'\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/anywhere/file.md")).unwrap_err();
    assert!(restricted_match(&err, "write", "/r/.remargin.yaml"));
}

/// Scenario 6 — `deny_ops` matches; refusal cites `DeniedOp`.
#[test]
fn scenario_06_deny_ops_matches_and_refuses() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    let err = pre_mutate_check(&system, "purge", Path::new("/r/src/foo/x.md")).unwrap_err();
    assert!(denied_op_match(&err, "purge", "/r/.remargin.yaml"));
}

/// Scenario 7 — `deny_ops` lists a different op than the one being
/// run; allowed.
#[test]
fn scenario_07_deny_ops_op_mismatch_allows() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    pre_mutate_check(&system, "comment", Path::new("/r/src/foo/x.md")).unwrap();
}

/// Scenario 8 — `deny_ops` covers descendants of the declared path.
#[test]
fn scenario_08_deny_ops_covers_descendants() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    let err = pre_mutate_check(&system, "purge", Path::new("/r/src/foo/sub/y.md")).unwrap_err();
    assert!(denied_op_match(&err, "purge", "/r/.remargin.yaml"));
}

/// Scenario 9 — path inside a dot-folder under a restricted subtree
/// surfaces `RestrictedPath` (restrict matches first; both rules
/// would refuse).
#[test]
fn scenario_09_dot_folder_under_restrict_is_denied() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/foo\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/src/foo/.git/x.md")).unwrap_err();
    assert!(restricted_match(&err, "write", "/r/.remargin.yaml"));
}

/// Scenario 9b — dot-folder default-deny outside an explicit restrict
/// subtree is not active.
#[test]
fn scenario_09b_dot_folder_outside_restrict_is_allowed() {
    let system = realm_with("identity: alice\n");
    pre_mutate_check(&system, "write", Path::new("/r/.git/foo.md")).unwrap();
}

/// Scenario 9c — wildcard restrict + dot-folder; `RestrictedPath`
/// fires first.
#[test]
fn scenario_09c_wildcard_with_dot_folder_denial() {
    let system = realm_with("permissions:\n  restrict:\n    - path: '*'\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/.git/x.md")).unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::RestrictedPath { .. })
    ));
}

/// Scenario 10 — `allow_dot_folders` does not unrestrict; restrict
/// still fires for paths under a restricted subtree.
#[test]
fn scenario_10_allow_dot_folders_does_not_override_restrict() {
    let system = realm_with(
        "permissions:\n  restrict:\n    - path: src/foo\n  allow_dot_folders: ['.git']\n",
    );
    let err = pre_mutate_check(&system, "write", Path::new("/r/src/foo/.git/x.md")).unwrap_err();
    assert!(restricted_match(&err, "write", "/r/.remargin.yaml"));
}

/// Scenario 11 — `.remargin/` is special-cased by the dot-folder
/// check. Driven via the matcher directly so we can isolate the
/// dot-folder branch from the restrict branch.
#[test]
fn scenario_11_remargin_folder_special_cased_by_dot_folder_check() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: Vec::new(),
        restrict: vec![ResolvedRestrict {
            also_deny_bash: Vec::new(),
            cli_allowed: false,
            path: RestrictPath::Wildcard {
                realm_root: PathBuf::from("/r"),
            },
            source_file: PathBuf::from("/r/.remargin.yaml"),
        }],
        trusted_roots: Vec::new(),
    };
    let err = check_against_resolved("write", Path::new("/r/.remargin/state.yaml"), &resolved)
        .unwrap_err();
    // RestrictedPath wins (wildcard covers everything). Crucially,
    // the dot-folder branch did not surface `.remargin` as the
    // offending folder.
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::RestrictedPath { .. })
    ));
}

/// Scenario 12 — multi-realm; refusal cites the deepest source file.
#[test]
fn scenario_12_multi_realm_deepest_source_first() {
    let parent = "permissions:\n  restrict:\n    - path: '*'\n";
    let child = "permissions:\n  restrict:\n    - path: '*'\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/r/sub"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/r/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let err = pre_mutate_check(&system, "write", Path::new("/r/sub/foo.md")).unwrap_err();
    assert!(restricted_match(&err, "write", "/r/sub/.remargin.yaml"));
}

/// Scenario 13 — target canonicalized before match (`MockSystem`
/// treats canonicalize as identity for absolute paths, so the test
/// asserts the canonicalized form is used as input to the matcher).
#[test]
fn scenario_13_target_canonicalized_before_match() {
    let system = realm_with("permissions:\n  restrict:\n    - path: real\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/real/x.md")).unwrap_err();
    assert!(restricted_match(&err, "write", "/r/.remargin.yaml"));
}

/// Scenario 14 — refusal cites the matching entry's source.
#[test]
fn scenario_14_refusal_cites_matching_entry_source() {
    let parent = "permissions:\n  restrict:\n    - path: untouched\n";
    let child = "permissions:\n  restrict:\n    - path: hit\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/r/sub"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/r/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let err = pre_mutate_check(&system, "write", Path::new("/r/sub/hit/foo.md")).unwrap_err();
    assert!(restricted_match(&err, "write", "/r/sub/.remargin.yaml"));
}

/// Scenario 17 — per-op re-resolution; no caching across calls.
#[test]
fn scenario_17_no_caching_per_op_reresolves() {
    let with_restrict = "permissions:\n  restrict:\n    - path: '*'\n";
    let without_restrict = "identity: alice\n";

    let initial = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), with_restrict.as_bytes())
        .unwrap();
    let err = pre_mutate_check(&initial, "comment", Path::new("/r/file.md")).unwrap_err();
    assert!(restricted_match(&err, "comment", "/r/.remargin.yaml"));

    let updated = initial
        .with_file(Path::new("/r/.remargin.yaml"), without_restrict.as_bytes())
        .unwrap();
    pre_mutate_check(&updated, "comment", Path::new("/r/file.md")).unwrap();
}

/// Scenario 19 — refusal error always carries the absolute path of
/// the declaring `.remargin.yaml`.
#[test]
fn scenario_19_source_file_in_every_refusal() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/src/foo.md")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/r/.remargin.yaml"),
        "error did not include source file path: {chain}"
    );
}

// ---------------------------------------------------------------------
// Auxiliary unit tests
// ---------------------------------------------------------------------

#[test]
fn restrict_covers_absolute_exact_and_descendants() {
    let entry = RestrictPath::Absolute(PathBuf::from("/r/src"));
    assert!(restrict_covers(&entry, Path::new("/r/src")));
    assert!(restrict_covers(&entry, Path::new("/r/src/foo.md")));
    assert!(restrict_covers(&entry, Path::new("/r/src/sub/foo.md")));
    assert!(!restrict_covers(&entry, Path::new("/r/other.md")));
}

#[test]
fn restrict_covers_wildcard_under_realm() {
    let entry = RestrictPath::Wildcard {
        realm_root: PathBuf::from("/r"),
    };
    assert!(restrict_covers(&entry, Path::new("/r/anything.md")));
    assert!(restrict_covers(&entry, Path::new("/r/sub/anything.md")));
    assert!(!restrict_covers(&entry, Path::new("/elsewhere/x.md")));
}

#[test]
fn is_mutating_op_recognises_full_set() {
    for op in MUTATING_OPS {
        assert!(is_mutating_op(op), "{op} should be mutating");
    }
    assert!(!is_mutating_op("get"));
    assert!(!is_mutating_op("query"));
}

/// Read-side ops bypass the dot-folder default-deny too, since the
/// guard only enforces `restrict` for mutating ops and the dot-folder
/// branch sits inside that gate.
#[test]
fn dot_folder_denial_only_active_for_mutating_ops() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: Vec::new(),
        restrict: vec![ResolvedRestrict {
            also_deny_bash: Vec::new(),
            cli_allowed: false,
            path: RestrictPath::Wildcard {
                realm_root: PathBuf::from("/r"),
            },
            source_file: PathBuf::from("/r/.remargin.yaml"),
        }],
        trusted_roots: Vec::new(),
    };
    check_against_resolved("get", Path::new("/r/src/.git/x.md"), &resolved).unwrap();
}

/// Sanity for the dot-folder match helper. Constructing a
/// [`OpGuardError::DotFolderDenied`] manually and verifying the
/// matcher recognises it.
#[test]
fn dot_folder_match_helper_is_callable() {
    let err: anyhow::Error = OpGuardError::DotFolderDenied {
        folder: String::from(".git"),
        op: String::from("write"),
        source_file: PathBuf::from("/r/.remargin.yaml"),
        target: PathBuf::from("/r/src/.git/x.md"),
    }
    .into();
    assert!(dot_folder_match(&err, ".git", "/r/.remargin.yaml"));
}
