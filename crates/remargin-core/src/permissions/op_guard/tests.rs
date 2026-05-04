//! Acceptance scenarios for the Layer 1 op guard (rem-yj1j.2 / T23).
//!
//! All tests run against `os_shim::mock::MockSystem`. Scenarios that
//! require driving real-op handlers (15, 16, 18) live with the
//! follow-up integration ticket — the unit tests here cover the
//! matcher's full decision table.

use std::env;
use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::config::Mode;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::{
    ResolvedDenyOps, ResolvedPermissions, ResolvedRestrict, RestrictPath, TrustedRoot,
};
use crate::parser::AuthorType;
use crate::permissions::op_guard::{
    CallerInfo, DENY_OPS_DENIAL_TEMPLATE, MUTATING_OPS, OpGuardError, OpKind, READ_OPS,
    RESTRICT_DENIAL_TEMPLATE, check_against_resolved, check_against_resolved_for_caller,
    is_mutating_op, op_kind, pre_mutate_check, restrict_covers,
};

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
// trusted_roots × restrict carve-out (rem-yj1j.x)
// ---------------------------------------------------------------------

/// Helper — build a `ResolvedPermissions` whose realm root carries a
/// `restrict '*'` and a list of trusted roots. Source file is
/// `<realm>/.remargin.yaml`.
fn allowlist_realm(realm: &str, trusted: &[&str]) -> ResolvedPermissions {
    let source_file = PathBuf::from(format!("{realm}/.remargin.yaml"));
    ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: Vec::new(),
        restrict: vec![ResolvedRestrict {
            also_deny_bash: Vec::new(),
            cli_allowed: false,
            path: RestrictPath::Wildcard {
                realm_root: PathBuf::from(realm),
            },
            source_file: source_file.clone(),
        }],
        trusted_roots: trusted
            .iter()
            .map(|p| TrustedRoot {
                path: PathBuf::from(p),
                source_file: source_file.clone(),
            })
            .collect(),
    }
}

/// Scenario 20 — outer `restrict '*'` is carved out for targets inside
/// a `trusted_root`. Mutating ops on those targets succeed.
#[test]
fn scenario_20_trusted_root_carves_out_outer_wildcard_restrict() {
    let resolved = allowlist_realm("/home/user", &["/home/user/notes"]);
    check_against_resolved("write", Path::new("/home/user/notes/foo.md"), &resolved).unwrap();
}

/// Scenario 21 — outer `restrict '*'` still blocks targets outside any
/// `trusted_root`.
#[test]
fn scenario_21_trusted_root_does_not_unrestrict_outside_paths() {
    let resolved = allowlist_realm("/home/user", &["/home/user/notes"]);
    let err = check_against_resolved("write", Path::new("/home/user/secret/foo.md"), &resolved)
        .unwrap_err();
    assert!(restricted_match(&err, "write", "/home/user/.remargin.yaml"));
}

/// Scenario 22 — a `restrict` declared *inside* a trusted root still
/// fires. The inner restrict's anchor lives below the trusted root, so
/// the trusted root is not at-or-below it and no carve-out applies.
#[test]
fn scenario_22_inner_restrict_inside_trusted_root_still_fires() {
    let outer_source = PathBuf::from("/home/user/.remargin.yaml");
    let inner_source = PathBuf::from("/home/user/notes/.remargin.yaml");
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: Vec::new(),
        restrict: vec![
            // Inner restrict declared inside the trusted root.
            ResolvedRestrict {
                also_deny_bash: Vec::new(),
                cli_allowed: false,
                path: RestrictPath::Absolute(PathBuf::from("/home/user/notes/secret")),
                source_file: inner_source,
            },
            // Outer wildcard restrict at the parent realm.
            ResolvedRestrict {
                also_deny_bash: Vec::new(),
                cli_allowed: false,
                path: RestrictPath::Wildcard {
                    realm_root: PathBuf::from("/home/user"),
                },
                source_file: outer_source,
            },
        ],
        trusted_roots: vec![TrustedRoot {
            path: PathBuf::from("/home/user/notes"),
            source_file: PathBuf::from("/home/user/.remargin.yaml"),
        }],
    };

    // Inside the trusted root but also inside the inner restrict — inner wins.
    let err = check_against_resolved(
        "write",
        Path::new("/home/user/notes/secret/foo.md"),
        &resolved,
    )
    .unwrap_err();
    assert!(restricted_match(
        &err,
        "write",
        "/home/user/notes/.remargin.yaml"
    ));

    // Inside the trusted root but outside the inner restrict — bypass works.
    check_against_resolved(
        "write",
        Path::new("/home/user/notes/public/foo.md"),
        &resolved,
    )
    .unwrap();
}

/// Scenario 23 — `deny_ops` is not affected by `trusted_roots`. A
/// `deny_ops [purge]` rule fires even on targets inside a trusted root.
#[test]
fn scenario_23_deny_ops_is_not_carved_out_by_trusted_roots() {
    use crate::config::permissions::resolve::ResolvedDenyOps;

    let source_file = PathBuf::from("/home/user/.remargin.yaml");
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: vec![ResolvedDenyOps {
            ops: vec![OpName::Purge],
            path: PathBuf::from("/home/user"),
            source_file: source_file.clone(),
            to: Vec::new(),
        }],
        restrict: vec![ResolvedRestrict {
            also_deny_bash: Vec::new(),
            cli_allowed: false,
            path: RestrictPath::Wildcard {
                realm_root: PathBuf::from("/home/user"),
            },
            source_file: source_file.clone(),
        }],
        trusted_roots: vec![TrustedRoot {
            path: PathBuf::from("/home/user/notes"),
            source_file,
        }],
    };

    // write inside trusted root: allowed (restrict carved out).
    check_against_resolved("write", Path::new("/home/user/notes/foo.md"), &resolved).unwrap();

    // purge inside trusted root: still denied by deny_ops.
    let err = check_against_resolved("purge", Path::new("/home/user/notes/foo.md"), &resolved)
        .unwrap_err();
    assert!(denied_op_match(&err, "purge", "/home/user/.remargin.yaml"));
}

/// Scenario 24 — dot-folder default-deny under a wildcard restrict is
/// carved out by `trusted_roots`. Targets inside `<trusted>/.git/x` are
/// writable when the trusted root is at-or-below the wildcard's anchor.
/// (The "outside-trust still blocked" complement is covered by
/// scenario 21 / 09c — wildcard restrict beats dot-folder there.)
#[test]
fn scenario_24_trusted_root_carves_out_dot_folder_default_deny() {
    let resolved = allowlist_realm("/home/user", &["/home/user/notes"]);
    check_against_resolved(
        "write",
        Path::new("/home/user/notes/.git/foo.md"),
        &resolved,
    )
    .unwrap();
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

// ---------------------------------------------------------------------
// Scenario 13 (full form) — symlinks resolved before matching.
//
// `MockSystem` does not model symlinks, so this exercise needs the
// real filesystem. We materialise a realm with a `restrict` rule on a
// real path, create a symlink that points into the restricted subtree,
// and verify the guard refuses the op when invoked through the link.
// ---------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn scenario_13_symlink_target_resolves_to_restricted_path() {
    use std::fs;
    use std::os::unix::fs::symlink;

    use os_shim::real::RealSystem;
    use tempfile::TempDir;

    let realm = TempDir::new().unwrap();
    let realm_path = realm.path();
    fs::create_dir_all(realm_path.join("src/secret")).unwrap();
    fs::write(realm_path.join("src/secret/foo.md"), "x").unwrap();
    fs::write(
        realm_path.join(".remargin.yaml"),
        "permissions:\n  restrict:\n    - path: src/secret\n",
    )
    .unwrap();

    let link = realm_path.join("alias.md");
    symlink(realm_path.join("src/secret/foo.md"), &link).unwrap();

    let system = RealSystem::new();
    let err = pre_mutate_check(&system, "comment", &link).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("denied by `restrict`"),
        "expected restrict refusal through symlink, got: {chain}"
    );
}

// ---------------------------------------------------------------------
// Op classification (read vs write).
// ---------------------------------------------------------------------

/// Every entry in the public `READ_OPS` constant classifies as
/// [`OpKind::Read`].
#[test]
fn op_kind_classifies_read_ops() {
    for op in READ_OPS {
        assert_eq!(op_kind(op), Some(OpKind::Read), "{op} should be Read");
    }
}

/// Every entry in the public `MUTATING_OPS` constant classifies as
/// [`OpKind::Write`].
#[test]
fn op_kind_classifies_mutating_ops() {
    for op in MUTATING_OPS {
        assert_eq!(op_kind(op), Some(OpKind::Write), "{op} should be Write");
    }
}

/// Op names not in either list are returned as `None`. Callers
/// (specifically `is_mutating_op`) treat that as fail-closed.
#[test]
fn op_kind_unknown_op_returns_none() {
    assert_eq!(op_kind("not-a-real-op"), None);
    // `is_mutating_op` defaults unknowns to mutating so an unclassified
    // op fails closed under `restrict`.
    assert!(is_mutating_op("not-a-real-op"));
}

/// `READ_OPS` and `MUTATING_OPS` partition the op space — no name
/// appears in both. Adding an op to the wrong list fails this test.
#[test]
fn read_and_mutating_op_lists_are_disjoint() {
    for read in READ_OPS {
        assert!(
            !MUTATING_OPS.contains(read),
            "{read} appears in both READ_OPS and MUTATING_OPS",
        );
    }
}

/// `READ_OPS` mirrors [`OpName::READ`] verbatim — the `deny_ops` parser
/// validates against `OpName`, the guard classifies via `READ_OPS`, so
/// drift between the two means an op the parser accepts is unclassified
/// at runtime (or vice versa).
#[test]
fn read_ops_constant_matches_op_name_read() {
    let from_enum: Vec<&str> = OpName::READ.iter().map(|op| op.as_str()).collect();
    let from_const: Vec<&str> = READ_OPS.to_vec();
    assert_eq!(from_const, from_enum);
}

/// `MUTATING_OPS` mirrors [`OpName::WRITE`] verbatim. See
/// [`read_ops_constant_matches_op_name_read`] for the rationale.
#[test]
fn mutating_ops_constant_matches_op_name_write() {
    let from_enum: Vec<&str> = OpName::WRITE.iter().map(|op| op.as_str()).collect();
    let from_const: Vec<&str> = MUTATING_OPS.to_vec();
    assert_eq!(from_const, from_enum);
}

// ---------------------------------------------------------------------
// Denial-error wording (pinned).
// ---------------------------------------------------------------------

/// Pin the canonical templates for each denial kind. The actual
/// `Display` impls use backtick delimiters; the documented templates
/// use single quotes (the wording in the design docs / acceptance
/// criteria). We accept either delimiter so wording drift in either
/// direction trips this test.
#[test]
fn denial_error_wording_matches_canonical_template() {
    let restrict = OpGuardError::RestrictedPath {
        op: String::from("comment"),
        source_file: PathBuf::from("/r/.remargin.yaml"),
        target: PathBuf::from("/r/secret/foo.md"),
    };
    let restrict_msg = format!("{restrict}");
    let restrict_expected_backtick =
        "op `comment` on `/r/secret/foo.md` is denied by `restrict` rule in /r/.remargin.yaml";
    let restrict_expected_quoted =
        "op 'comment' on '/r/secret/foo.md' is denied by 'restrict' rule in /r/.remargin.yaml";
    assert!(
        restrict_msg == restrict_expected_backtick || restrict_msg == restrict_expected_quoted,
        "RestrictedPath wording drifted; got: {restrict_msg}",
    );

    let denied = OpGuardError::DeniedOp {
        op: String::from("purge"),
        source_file: PathBuf::from("/r/.remargin.yaml"),
        target: PathBuf::from("/r/signed/x.md"),
        to: Vec::new(),
    };
    let denied_msg = format!("{denied}");
    let denied_expected_backtick =
        "op `purge` on `/r/signed/x.md` is denied by `deny_ops` rule in /r/.remargin.yaml";
    let denied_expected_quoted =
        "op 'purge' on '/r/signed/x.md' is denied by 'deny_ops' rule in /r/.remargin.yaml";
    assert!(
        denied_msg == denied_expected_backtick || denied_msg == denied_expected_quoted,
        "DeniedOp wording drifted; got: {denied_msg}",
    );

    // The template constants document the same shape as the actual
    // wording (modulo delimiters). Pin a couple of structural
    // invariants so a typo in either constant trips the test.
    assert!(RESTRICT_DENIAL_TEMPLATE.contains("'restrict' rule in"));
    assert!(RESTRICT_DENIAL_TEMPLATE.contains("{op}"));
    assert!(RESTRICT_DENIAL_TEMPLATE.contains("{target}"));
    assert!(RESTRICT_DENIAL_TEMPLATE.contains("{source_file}"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("'deny_ops' rule in"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("{op}"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("{target}"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("{source_file}"));
}

// ---------------------------------------------------------------------
// rem-egp9 — identity-scoped deny_ops + agent ~/.ssh/** default
// ---------------------------------------------------------------------

fn deny_ops_with_to(ops: Vec<OpName>, path: &str, to: &[&str]) -> Vec<ResolvedDenyOps> {
    vec![ResolvedDenyOps {
        ops,
        path: PathBuf::from(path),
        source_file: PathBuf::from("/r/.remargin.yaml"),
        to: to.iter().copied().map(String::from).collect(),
    }]
}

fn caller(name: &str, author_type: AuthorType, mode: Mode) -> CallerInfo {
    CallerInfo {
        author_type: Some(author_type),
        identity_id: Some(String::from(name)),
        identity_name: Some(String::from(name)),
        mode,
    }
}

/// rem-egp9: `to:` filter matches the caller's identity in strict
/// mode → deny fires.
#[test]
fn deny_ops_to_matches_caller_in_strict_mode_refuses() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["alice"]),
        restrict: Vec::new(),
        trusted_roots: Vec::new(),
    };
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    let err =
        check_against_resolved_for_caller("purge", Path::new("/r/secret/x.md"), &resolved, &caller)
            .unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("alice"),
        "refusal must name the identity: {chain}"
    );
    assert!(
        chain.contains("deny_ops"),
        "refusal must cite deny_ops: {chain}"
    );
}

/// rem-egp9: `to:` filter does NOT match the caller in strict mode →
/// deny does not fire (rule applies only to other identities).
#[test]
fn deny_ops_to_does_not_match_caller_in_strict_mode_allows() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["bob"]),
        restrict: Vec::new(),
        trusted_roots: Vec::new(),
    };
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    check_against_resolved_for_caller("purge", Path::new("/r/secret/x.md"), &resolved, &caller)
        .unwrap();
}

/// rem-egp9: in open mode the `to:` filter is ignored — the deny
/// fires for every identity (the realm cannot trust the declared
/// identity). Lint surfaces a warning at parse time.
#[test]
fn deny_ops_to_is_ignored_in_open_mode() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["bob"]),
        restrict: Vec::new(),
        trusted_roots: Vec::new(),
    };
    // Caller "alice" is NOT in `to`, but the realm is open mode.
    // The deny fires anyway because open mode cannot trust identity.
    let caller = caller("alice", AuthorType::Human, Mode::Open);
    let err =
        check_against_resolved_for_caller("purge", Path::new("/r/secret/x.md"), &resolved, &caller)
            .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp { .. })
    ));
}

/// rem-egp9: a strict-mode AGENT caller is denied READ on
/// `~/.ssh/id_ed25519` by the synthesized default. Drives the read
/// path through the `get` op (which `is_mutating_op` reports `false`
/// — the synthesized deny covers every op including reads).
#[test]
fn strict_agent_denied_default_ssh_read() {
    // Stand $HOME up so the synthesized path is deterministic.
    // SAFETY: tests run single-threaded by default; no other test in
    // this scope touches HOME at the same time.
    // SAFETY: std::env::set_var/remove_var are unsafe in 2024 edition.
    // SAFETY: tests run single-threaded by default; the rem-egp9
    // suite owns the HOME env var for the duration of the test.
    // SAFETY (rem-egp9): cargo test runs each test on its own thread but
    // env vars are process-global. The HOME-touching tests in this scope
    // are not parallel-safe with each other, but they are deterministic
    // when run individually (cargo test -- --test-threads=1 honored by CI).
    let _: () = unsafe { env::set_var("HOME", "/h") };
    let resolved = ResolvedPermissions::default();
    let caller = caller("nimbus", AuthorType::Agent, Mode::Strict);
    let err = check_against_resolved_for_caller(
        "get",
        Path::new("/h/.ssh/id_ed25519"),
        &resolved,
        &caller,
    )
    .unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("deny_ops"), "{chain}");
}

/// rem-egp9: a strict-mode HUMAN caller can read the same SSH path
/// (the synthesized default applies to agents only).
#[test]
fn strict_human_can_read_ssh() {
    // SAFETY: tests run single-threaded by default; the rem-egp9
    // suite owns the HOME env var for the duration of the test.
    // SAFETY (rem-egp9): cargo test runs each test on its own thread but
    // env vars are process-global. The HOME-touching tests in this scope
    // are not parallel-safe with each other, but they are deterministic
    // when run individually (cargo test -- --test-threads=1 honored by CI).
    let _: () = unsafe { env::set_var("HOME", "/h") };
    let resolved = ResolvedPermissions::default();
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    check_against_resolved_for_caller("get", Path::new("/h/.ssh/id_ed25519"), &resolved, &caller)
        .unwrap();
}

/// rem-egp9: the user can override the synthesized default by listing
/// the same path with `to: [<agent_id>]` and `ops: []`.
#[test]
fn strict_agent_default_ssh_override_via_explicit_to_with_empty_ops() {
    // SAFETY: tests run single-threaded by default; the rem-egp9
    // suite owns the HOME env var for the duration of the test.
    // SAFETY (rem-egp9): cargo test runs each test on its own thread but
    // env vars are process-global. The HOME-touching tests in this scope
    // are not parallel-safe with each other, but they are deterministic
    // when run individually (cargo test -- --test-threads=1 honored by CI).
    let _: () = unsafe { env::set_var("HOME", "/h") };
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: vec![ResolvedDenyOps {
            ops: Vec::new(),
            path: PathBuf::from("/h/.ssh"),
            source_file: PathBuf::from("/r/.remargin.yaml"),
            to: vec![String::from("nimbus")],
        }],
        restrict: Vec::new(),
        trusted_roots: Vec::new(),
    };
    let caller = caller("nimbus", AuthorType::Agent, Mode::Strict);
    check_against_resolved_for_caller("get", Path::new("/h/.ssh/id_ed25519"), &resolved, &caller)
        .unwrap();
}

/// rem-egp9: the synthesized SSH default does NOT fire in open mode
/// (the realm cannot trust that the caller really is an agent).
#[test]
fn open_mode_agent_can_read_ssh_no_synthesized_default() {
    // SAFETY: tests run single-threaded by default; the rem-egp9
    // suite owns the HOME env var for the duration of the test.
    // SAFETY (rem-egp9): cargo test runs each test on its own thread but
    // env vars are process-global. The HOME-touching tests in this scope
    // are not parallel-safe with each other, but they are deterministic
    // when run individually (cargo test -- --test-threads=1 honored by CI).
    let _: () = unsafe { env::set_var("HOME", "/h") };
    let resolved = ResolvedPermissions::default();
    let caller = caller("nimbus", AuthorType::Agent, Mode::Open);
    check_against_resolved_for_caller("get", Path::new("/h/.ssh/id_ed25519"), &resolved, &caller)
        .unwrap();
}

/// rem-egp9: identity matching falls back from name to id.
#[test]
fn deny_ops_to_matches_id_when_name_does_not() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["alice-id"]),
        restrict: Vec::new(),
        trusted_roots: Vec::new(),
    };
    let caller = CallerInfo {
        author_type: Some(AuthorType::Human),
        identity_id: Some(String::from("alice-id")),
        identity_name: Some(String::from("alice-display-name")),
        mode: Mode::Strict,
    };
    let err =
        check_against_resolved_for_caller("purge", Path::new("/r/secret/x.md"), &resolved, &caller)
            .unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("alice-id"), "{chain}");
}
