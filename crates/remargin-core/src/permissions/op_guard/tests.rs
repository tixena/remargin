//! Acceptance scenarios for the per-op guard under allow-list polarity.

use std::env;
use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::config::Mode;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::{
    ResolvedDenyOps, ResolvedPermissions, ResolvedRestrict, RestrictPath,
};
use crate::parser::AuthorType;
use crate::permissions::op_guard::{
    CallerInfo, DENY_OPS_DENIAL_TEMPLATE, MUTATING_OPS, OUTSIDE_ALLOWED_DENIAL_TEMPLATE,
    OpGuardError, OpKind, READ_OPS, check_against_resolved, check_against_resolved_for_caller,
    is_mutating_op, op_kind, pre_mutate_check, restrict_covers,
};

fn realm_with(yaml: &str) -> MockSystem {
    MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), yaml.as_bytes())
        .unwrap()
}

fn outside_allowed_match(err: &anyhow::Error, op_name: &str, source: &str) -> bool {
    matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::OutsideAllowedRoots { op, source_file, .. })
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
// Allow-list polarity
// ---------------------------------------------------------------------

/// No `restrict` declared → open mode → everything allowed.
#[test]
fn scenario_01_no_restrict_allows_everything() {
    let system = realm_with("identity: alice\n");
    pre_mutate_check(&system, "comment", Path::new("/r/foo.md")).unwrap();
}

/// `restrict src/secret` allow-lists that subpath; mutating op INSIDE
/// the allow-list succeeds.
#[test]
fn scenario_02_restrict_subpath_allows_inside() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/secret\n");
    pre_mutate_check(&system, "comment", Path::new("/r/src/secret/foo.md")).unwrap();
}

/// `restrict src/secret` blocks targets OUTSIDE the allow-list.
#[test]
fn scenario_03_restrict_subpath_blocks_outside() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/secret\n");
    let err = pre_mutate_check(&system, "comment", Path::new("/r/src/public/foo.md")).unwrap_err();
    assert!(outside_allowed_match(&err, "comment", "/r/.remargin.yaml"));
}

/// Read ops bypass the allow-list check; only `deny_ops` can block reads.
#[test]
fn scenario_04_restrict_does_not_block_read_ops() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/secret\n");
    for op in READ_OPS {
        let result = pre_mutate_check(&system, op, Path::new("/r/src/public/foo.md"));
        assert!(result.is_ok(), "read op {op} should not be blocked");
    }
}

/// `restrict '*'` covers the whole realm; any path under it is allowed.
#[test]
fn scenario_05_wildcard_restrict_allows_anywhere_in_realm() {
    let system = realm_with("permissions:\n  restrict:\n    - path: '*'\n");
    pre_mutate_check(&system, "write", Path::new("/r/anywhere/file.md")).unwrap();
}

/// `deny_ops` matches; refusal cites `DeniedOp`.
#[test]
fn scenario_06_deny_ops_matches_and_refuses() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    let err = pre_mutate_check(&system, "purge", Path::new("/r/src/foo/x.md")).unwrap_err();
    assert!(denied_op_match(&err, "purge", "/r/.remargin.yaml"));
}

/// `deny_ops` op mismatch → allowed.
#[test]
fn scenario_07_deny_ops_op_mismatch_allows() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    pre_mutate_check(&system, "comment", Path::new("/r/src/foo/x.md")).unwrap();
}

/// `deny_ops` covers descendants.
#[test]
fn scenario_08_deny_ops_covers_descendants() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    let err = pre_mutate_check(&system, "purge", Path::new("/r/src/foo/sub/y.md")).unwrap_err();
    assert!(denied_op_match(&err, "purge", "/r/.remargin.yaml"));
}

/// Inside allow-list, target is also inside an unlisted dot-folder →
/// `DotFolderDenied` fires.
#[test]
fn scenario_09_dot_folder_under_allow_list_is_denied() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src/foo\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/src/foo/.git/x.md")).unwrap_err();
    assert!(dot_folder_match(&err, ".git", "/r/.remargin.yaml"));
}

/// Open mode (no restrict) — dot-folder default-deny does not fire.
#[test]
fn scenario_09b_dot_folder_outside_restrict_is_allowed() {
    let system = realm_with("identity: alice\n");
    pre_mutate_check(&system, "write", Path::new("/r/.git/foo.md")).unwrap();
}

/// `restrict '*'` allows everything in realm but dot-folder default-deny
/// still fires for `.git/` etc.
#[test]
fn scenario_09c_wildcard_with_dot_folder_denial() {
    let system = realm_with("permissions:\n  restrict:\n    - path: '*'\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/.git/x.md")).unwrap_err();
    assert!(dot_folder_match(&err, ".git", "/r/.remargin.yaml"));
}

/// `allow_dot_folders` lifts the default-deny for the named folders.
#[test]
fn scenario_10_allow_dot_folders_unblocks_named_dot_folder() {
    let system = realm_with(
        "permissions:\n  restrict:\n    - path: src/foo\n  allow_dot_folders: ['.git']\n",
    );
    pre_mutate_check(&system, "write", Path::new("/r/src/foo/.git/x.md")).unwrap();
}

/// `.remargin/` is always allowed — no dot-folder default-deny.
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
    };
    check_against_resolved("write", Path::new("/r/.remargin/state.yaml"), &resolved).unwrap();
}

/// Multi-realm: deepest restrict declaration is what cites the source
/// file when a path is outside its scope but inside the parent's.
/// Since `restrict` accumulates as an allow-list, the deepest entry
/// declared at `/r/sub` covers `/r/sub/foo.md`, so the op succeeds.
#[test]
fn scenario_12_multi_realm_walks_combine() {
    let parent = "permissions:\n  restrict:\n    - path: '*'\n";
    let child = "permissions:\n  restrict:\n    - path: '*'\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/r/sub"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/r/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    pre_mutate_check(&system, "write", Path::new("/r/sub/foo.md")).unwrap();
}

/// Per-op re-resolution: editing `.remargin.yaml` between calls takes
/// effect immediately.
#[test]
fn scenario_17_no_caching_per_op_reresolves() {
    let with_restrict_outside = "permissions:\n  restrict:\n    - path: only-this-subdir\n";
    let without_restrict = "identity: alice\n";

    let initial = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            with_restrict_outside.as_bytes(),
        )
        .unwrap();
    // `/r/file.md` is OUTSIDE the allow-list `/r/only-this-subdir`.
    let err = pre_mutate_check(&initial, "comment", Path::new("/r/file.md")).unwrap_err();
    assert!(outside_allowed_match(&err, "comment", "/r/.remargin.yaml"));

    let updated = initial
        .with_file(Path::new("/r/.remargin.yaml"), without_restrict.as_bytes())
        .unwrap();
    pre_mutate_check(&updated, "comment", Path::new("/r/file.md")).unwrap();
}

/// Refusal carries the absolute path of the declaring `.remargin.yaml`.
#[test]
fn scenario_19_source_file_in_every_refusal() {
    let system = realm_with("permissions:\n  restrict:\n    - path: src\n");
    // Outside the allow-list `src` → refused.
    let err = pre_mutate_check(&system, "write", Path::new("/r/other.md")).unwrap_err();
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

/// Read-side ops bypass the dot-folder default-deny as well.
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
    };
    check_against_resolved("get", Path::new("/r/src/.git/x.md"), &resolved).unwrap();
}

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

#[cfg(unix)]
#[test]
fn scenario_13_symlink_target_resolves_to_allow_list_outside() {
    use std::fs;
    use std::os::unix::fs::symlink;

    use os_shim::real::RealSystem;
    use tempfile::TempDir;

    let realm = TempDir::new().unwrap();
    let realm_path = realm.path();
    fs::create_dir_all(realm_path.join("public")).unwrap();
    fs::create_dir_all(realm_path.join("src/secret")).unwrap();
    fs::write(realm_path.join("public/foo.md"), "x").unwrap();
    fs::write(
        realm_path.join(".remargin.yaml"),
        "permissions:\n  restrict:\n    - path: src/secret\n",
    )
    .unwrap();

    // Symlink the public file from outside the allow-list to verify the
    // canonical (resolved-symlink) path is what's checked.
    let link = realm_path.join("alias.md");
    symlink(realm_path.join("public/foo.md"), &link).unwrap();

    let system = RealSystem::new();
    let err = pre_mutate_check(&system, "comment", &link).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("outside the allow-list"),
        "expected allow-list refusal through symlink, got: {chain}"
    );
}

// ---------------------------------------------------------------------
// Op classification (read vs write)
// ---------------------------------------------------------------------

#[test]
fn op_kind_classifies_read_ops() {
    for op in READ_OPS {
        assert_eq!(op_kind(op), Some(OpKind::Read), "{op} should be Read");
    }
}

#[test]
fn op_kind_classifies_mutating_ops() {
    for op in MUTATING_OPS {
        assert_eq!(op_kind(op), Some(OpKind::Write), "{op} should be Write");
    }
}

#[test]
fn op_kind_unknown_op_returns_none() {
    assert_eq!(op_kind("not-a-real-op"), None);
    assert!(is_mutating_op("not-a-real-op"));
}

#[test]
fn read_and_mutating_op_lists_are_disjoint() {
    for read in READ_OPS {
        assert!(
            !MUTATING_OPS.contains(read),
            "{read} appears in both READ_OPS and MUTATING_OPS",
        );
    }
}

#[test]
fn read_ops_constant_matches_op_name_read() {
    let from_enum: Vec<&str> = OpName::READ.iter().map(|op| op.as_str()).collect();
    let from_const: Vec<&str> = READ_OPS.to_vec();
    assert_eq!(from_const, from_enum);
}

#[test]
fn mutating_ops_constant_matches_op_name_write() {
    let from_enum: Vec<&str> = OpName::WRITE.iter().map(|op| op.as_str()).collect();
    let from_const: Vec<&str> = MUTATING_OPS.to_vec();
    assert_eq!(from_const, from_enum);
}

// ---------------------------------------------------------------------
// Denial-error wording (pinned)
// ---------------------------------------------------------------------

#[test]
fn denial_error_wording_matches_canonical_template() {
    let outside = OpGuardError::OutsideAllowedRoots {
        op: String::from("comment"),
        source_file: PathBuf::from("/r/.remargin.yaml"),
        target: PathBuf::from("/r/secret/foo.md"),
    };
    let outside_msg = format!("{outside}");
    let outside_expected_backtick = "op `comment` on `/r/secret/foo.md` is denied: outside the allow-list declared by `restrict` in /r/.remargin.yaml";
    let outside_expected_quoted = "op 'comment' on '/r/secret/foo.md' is denied: outside the allow-list declared by 'restrict' in /r/.remargin.yaml";
    assert!(
        outside_msg == outside_expected_backtick || outside_msg == outside_expected_quoted,
        "OutsideAllowedRoots wording drifted; got: {outside_msg}",
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

    assert!(OUTSIDE_ALLOWED_DENIAL_TEMPLATE.contains("outside the allow-list"));
    assert!(OUTSIDE_ALLOWED_DENIAL_TEMPLATE.contains("{op}"));
    assert!(OUTSIDE_ALLOWED_DENIAL_TEMPLATE.contains("{target}"));
    assert!(OUTSIDE_ALLOWED_DENIAL_TEMPLATE.contains("{source_file}"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("'deny_ops' rule in"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("{op}"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("{target}"));
    assert!(DENY_OPS_DENIAL_TEMPLATE.contains("{source_file}"));
}

// ---------------------------------------------------------------------
// Identity-scoped deny_ops + agent ~/.ssh/** default (rem-egp9)
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

#[test]
fn deny_ops_to_matches_caller_in_strict_mode_refuses() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["alice"]),
        restrict: Vec::new(),
    };
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    let err =
        check_against_resolved_for_caller("purge", Path::new("/r/secret/x.md"), &resolved, &caller)
            .unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("alice"));
    assert!(chain.contains("deny_ops"));
}

#[test]
fn deny_ops_to_does_not_match_caller_in_strict_mode_allows() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["bob"]),
        restrict: Vec::new(),
    };
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    check_against_resolved_for_caller("purge", Path::new("/r/secret/x.md"), &resolved, &caller)
        .unwrap();
}

#[test]
fn deny_ops_to_is_ignored_in_open_mode() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["bob"]),
        restrict: Vec::new(),
    };
    let caller = caller("alice", AuthorType::Human, Mode::Open);
    let err =
        check_against_resolved_for_caller("purge", Path::new("/r/secret/x.md"), &resolved, &caller)
            .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp { .. })
    ));
}

#[test]
fn strict_agent_denied_default_ssh_read() {
    // SAFETY: cargo test threads share env vars. Tests in this module
    // that touch HOME run serially via -- --test-threads=1 in CI.
    // SAFETY: tests touching HOME serialise via cargo's default
    // single-threaded test runner; this block fully owns the env var
    // for the duration of the test.
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

#[test]
fn strict_human_can_read_ssh() {
    // SAFETY: tests touching HOME serialise via cargo's default
    // single-threaded test runner; this block fully owns the env var
    // for the duration of the test.
    let _: () = unsafe { env::set_var("HOME", "/h") };
    let resolved = ResolvedPermissions::default();
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    check_against_resolved_for_caller("get", Path::new("/h/.ssh/id_ed25519"), &resolved, &caller)
        .unwrap();
}

#[test]
fn strict_agent_default_ssh_override_via_explicit_to_with_empty_ops() {
    // SAFETY: tests touching HOME serialise via cargo's default
    // single-threaded test runner; this block fully owns the env var
    // for the duration of the test.
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
    };
    let caller = caller("nimbus", AuthorType::Agent, Mode::Strict);
    check_against_resolved_for_caller("get", Path::new("/h/.ssh/id_ed25519"), &resolved, &caller)
        .unwrap();
}

#[test]
fn open_mode_agent_can_read_ssh_no_synthesized_default() {
    // SAFETY: tests touching HOME serialise via cargo's default
    // single-threaded test runner; this block fully owns the env var
    // for the duration of the test.
    let _: () = unsafe { env::set_var("HOME", "/h") };
    let resolved = ResolvedPermissions::default();
    let caller = caller("nimbus", AuthorType::Agent, Mode::Open);
    check_against_resolved_for_caller("get", Path::new("/h/.ssh/id_ed25519"), &resolved, &caller)
        .unwrap();
}

#[test]
fn deny_ops_to_matches_id_when_name_does_not() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        deny_ops: deny_ops_with_to(vec![OpName::Purge], "/r/secret", &["alice-id"]),
        restrict: Vec::new(),
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
