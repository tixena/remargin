//! Acceptance scenarios for the per-op guard under allow-list polarity.

use std::path::{Path, PathBuf};

use os_shim::mock::MemorySystem;

use crate::config::Mode;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::trusted_root_covers;
use crate::config::permissions::resolve::{
    ResolvedDenyOps, ResolvedDenyOpsItem, ResolvedPermissions, ResolvedTrustedRoot, TrustedRootPath,
};
use crate::parser::AuthorType;
use crate::permissions::op_guard::{
    CallerInfo, DENY_OPS_DENIAL_TEMPLATE, MUTATING_OPS, OUTSIDE_ALLOWED_DENIAL_TEMPLATE,
    OpGuardError, OpKind, READ_OPS, check_against_resolved, check_against_resolved_for_caller,
    is_mutating_op, op_kind, pre_mutate_check,
};

fn realm_with(yaml: &str) -> MemorySystem {
    MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), yaml.as_bytes())
        .unwrap()
}

fn outside_allowed_match(err: &anyhow::Error, op_name: &str, source: &str) -> bool {
    matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::OutsideAllowedRoots { op, source_file, target: _ })
            if op == op_name && source_file == &PathBuf::from(source)
    )
}

fn denied_op_match(err: &anyhow::Error, op_name: &str, source: &str) -> bool {
    matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp { op, source_file, target: _ })
            if op == op_name && source_file == &PathBuf::from(source)
    )
}

fn dot_folder_match(err: &anyhow::Error, expected_folder: &str, source: &str) -> bool {
    matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DotFolderDenied { folder, source_file, op: _, target: _ })
            if folder == expected_folder && source_file == &PathBuf::from(source)
    )
}

#[test]
fn scenario_01_no_restrict_allows_everything() {
    let system = realm_with("identity: alice\n");
    pre_mutate_check(&system, "comment", Path::new("/r/foo.md")).unwrap();
}

/// The guard surfaces the failed resolution as an op error naming the yaml and the anchor.
#[test]
fn out_of_realm_trusted_root_surfaces_op_error() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: /other/secret\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/foo.md")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/r/.remargin.yaml"), "{chain}");
    assert!(chain.contains("/other/secret"), "{chain}");
    assert!(chain.contains("outside the realm"), "{chain}");
}

#[test]
fn scenario_02_restrict_subpath_allows_inside() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: src/secret\n");
    pre_mutate_check(&system, "comment", Path::new("/r/src/secret/foo.md")).unwrap();
}

#[test]
fn scenario_03_restrict_subpath_blocks_outside() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: src/secret\n");
    let err = pre_mutate_check(&system, "comment", Path::new("/r/src/public/foo.md")).unwrap_err();
    assert!(outside_allowed_match(&err, "comment", "/r/.remargin.yaml"));
}

#[test]
fn scenario_04_restrict_blocks_read_ops_outside_allow_list() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: src/secret\n");
    for op in READ_OPS {
        let err = pre_mutate_check(&system, op, Path::new("/r/src/public/foo.md")).unwrap_err();
        assert!(
            outside_allowed_match(&err, op, "/r/.remargin.yaml"),
            "read op {op} expected OutsideAllowedRoots, got: {err:#}",
        );
    }
}

#[test]
fn scenario_04b_restrict_allows_read_ops_inside_allow_list() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: src/secret\n");
    for op in READ_OPS {
        let result = pre_mutate_check(&system, op, Path::new("/r/src/secret/foo.md"));
        assert!(
            result.is_ok(),
            "read op {op} should be allowed; got: {:?}",
            result.err(),
        );
    }
}

#[test]
fn scenario_05_wildcard_restrict_allows_anywhere_in_realm() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: '*'\n");
    pre_mutate_check(&system, "write", Path::new("/r/anywhere/file.md")).unwrap();
}

#[test]
fn scenario_06_deny_ops_matches_and_refuses() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    let err = pre_mutate_check(&system, "purge", Path::new("/r/src/foo/x.md")).unwrap_err();
    assert!(denied_op_match(&err, "purge", "/r/.remargin.yaml"));
}

#[test]
fn scenario_07_deny_ops_op_mismatch_allows() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    pre_mutate_check(&system, "comment", Path::new("/r/src/foo/x.md")).unwrap();
}

#[test]
fn scenario_08_deny_ops_covers_descendants() {
    let system = realm_with("permissions:\n  deny_ops:\n    - path: src/foo\n      ops: [purge]\n");
    let err = pre_mutate_check(&system, "purge", Path::new("/r/src/foo/sub/y.md")).unwrap_err();
    assert!(denied_op_match(&err, "purge", "/r/.remargin.yaml"));
}

#[test]
fn scenario_09_dot_folder_under_allow_list_is_denied() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: src/foo\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/src/foo/.git/x.md")).unwrap_err();
    assert!(dot_folder_match(&err, ".git", "/r/.remargin.yaml"));
}

/// With no restrict declared the dot-folder default-deny does not fire.
#[test]
fn scenario_09b_dot_folder_outside_restrict_is_allowed() {
    let system = realm_with("identity: alice\n");
    pre_mutate_check(&system, "write", Path::new("/r/.git/foo.md")).unwrap();
}

#[test]
fn scenario_09c_wildcard_with_dot_folder_denial() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: '*'\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/.git/x.md")).unwrap_err();
    assert!(dot_folder_match(&err, ".git", "/r/.remargin.yaml"));
}

#[test]
fn scenario_10_allow_dot_folders_unblocks_named_dot_folder() {
    let system = realm_with(
        "permissions:\n  trusted_roots:\n    - path: src/foo\n  allow_dot_folders: ['.git']\n",
    );
    pre_mutate_check(&system, "write", Path::new("/r/src/foo/.git/x.md")).unwrap();
}

#[test]
fn scenario_11_remargin_folder_special_cased_by_dot_folder_check() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: Vec::new(),
        trusted_roots: vec![ResolvedTrustedRoot {
            also_deny_bash: Vec::new(),
            cli_allowed: false,
            path: TrustedRootPath::Wildcard {
                realm_root: PathBuf::from("/r"),
            },
            source_file: PathBuf::from("/r/.remargin.yaml"),
        }],
        trusted_roots_lock: None,
    };
    let system = MemorySystem::new();
    check_against_resolved(
        &system,
        "write",
        Path::new("/r/.remargin/state.yaml"),
        &resolved,
    )
    .unwrap();
}

/// `trusted_roots` accumulates as an allow-list, so the entry declared at `/r/sub` covers its
/// file.
#[test]
fn scenario_12_multi_realm_walks_combine() {
    let parent = "permissions:\n  trusted_roots:\n    - path: '*'\n";
    let child = "permissions:\n  trusted_roots:\n    - path: '*'\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/r/sub"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/r/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    pre_mutate_check(&system, "write", Path::new("/r/sub/foo.md")).unwrap();
}

#[test]
fn scenario_17_no_caching_per_op_reresolves() {
    let with_restrict_outside = "permissions:\n  trusted_roots:\n    - path: only-this-subdir\n";
    let without_restrict = "identity: alice\n";

    let initial = MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            with_restrict_outside.as_bytes(),
        )
        .unwrap();
    let err = pre_mutate_check(&initial, "comment", Path::new("/r/file.md")).unwrap_err();
    assert!(outside_allowed_match(&err, "comment", "/r/.remargin.yaml"));

    let updated = initial
        .with_file(Path::new("/r/.remargin.yaml"), without_restrict.as_bytes())
        .unwrap();
    pre_mutate_check(&updated, "comment", Path::new("/r/file.md")).unwrap();
}

#[test]
fn scenario_19_source_file_in_every_refusal() {
    let system = realm_with("permissions:\n  trusted_roots:\n    - path: src\n");
    let err = pre_mutate_check(&system, "write", Path::new("/r/other.md")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/r/.remargin.yaml"),
        "error did not include source file path: {chain}"
    );
}

#[test]
fn trusted_root_covers_absolute_exact_and_descendants() {
    let entry = TrustedRootPath::Absolute(PathBuf::from("/r/src"));
    assert!(trusted_root_covers(&entry, Path::new("/r/src")));
    assert!(trusted_root_covers(&entry, Path::new("/r/src/foo.md")));
    assert!(trusted_root_covers(&entry, Path::new("/r/src/sub/foo.md")));
    assert!(!trusted_root_covers(&entry, Path::new("/r/other.md")));
}

#[test]
fn trusted_root_covers_wildcard_under_realm() {
    let entry = TrustedRootPath::Wildcard {
        realm_root: PathBuf::from("/r"),
    };
    assert!(trusted_root_covers(&entry, Path::new("/r/anything.md")));
    assert!(trusted_root_covers(&entry, Path::new("/r/sub/anything.md")));
    assert!(!trusted_root_covers(&entry, Path::new("/elsewhere/x.md")));
}

#[test]
fn is_mutating_op_recognises_full_set() {
    for op in MUTATING_OPS {
        assert!(is_mutating_op(op), "{op} should be mutating");
    }
    assert!(!is_mutating_op("get"));
    assert!(!is_mutating_op("query"));
}

#[test]
fn dot_folder_denial_active_for_read_ops() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: Vec::new(),
        trusted_roots: vec![ResolvedTrustedRoot {
            also_deny_bash: Vec::new(),
            cli_allowed: false,
            path: TrustedRootPath::Wildcard {
                realm_root: PathBuf::from("/r"),
            },
            source_file: PathBuf::from("/r/.remargin.yaml"),
        }],
        trusted_roots_lock: None,
    };
    let system = MemorySystem::new();
    let err = check_against_resolved(&system, "get", Path::new("/r/src/.git/x.md"), &resolved)
        .unwrap_err();
    assert!(dot_folder_match(&err, ".git", "/r/.remargin.yaml"));
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
        "permissions:\n  trusted_roots:\n    - path: src/secret\n",
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

#[test]
fn denial_error_wording_matches_canonical_template() {
    let outside = OpGuardError::OutsideAllowedRoots {
        op: String::from("comment"),
        source_file: PathBuf::from("/r/.remargin.yaml"),
        target: PathBuf::from("/r/secret/foo.md"),
    };
    let outside_msg = format!("{outside}");
    let outside_expected_backtick = "op `comment` on `/r/secret/foo.md` is denied: outside the allow-list declared by `trusted_roots` in /r/.remargin.yaml";
    let outside_expected_quoted = "op 'comment' on '/r/secret/foo.md' is denied: outside the allow-list declared by 'trusted_roots' in /r/.remargin.yaml";
    assert!(
        outside_msg == outside_expected_backtick || outside_msg == outside_expected_quoted,
        "OutsideAllowedRoots wording drifted; got: {outside_msg}",
    );

    let denied = OpGuardError::DeniedOp {
        op: String::from("purge"),
        source_file: PathBuf::from("/r/.remargin.yaml"),
        target: PathBuf::from("/r/signed/x.md"),
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

fn deny_ops_items(ops: Vec<ResolvedDenyOpsItem>, path: &str) -> Vec<ResolvedDenyOps> {
    vec![ResolvedDenyOps {
        ops,
        path: PathBuf::from(path),
        source_file: PathBuf::from("/r/.remargin.yaml"),
    }]
}

fn bare(name: OpName) -> ResolvedDenyOpsItem {
    ResolvedDenyOpsItem {
        exceptions: Vec::new(),
        name,
    }
}

fn with_exceptions(name: OpName, exceptions: &[&str]) -> ResolvedDenyOpsItem {
    ResolvedDenyOpsItem {
        exceptions: exceptions.iter().copied().map(String::from).collect(),
        name,
    }
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
fn exceptions_bare_blanket_refuses() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(vec![bare(OpName::Purge)], "/r/secret"),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    let err = check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp {
            op: _,
            source_file: _,
            target: _
        })
    ));
}

#[test]
fn exceptions_bare_unrelated_op_allowed() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(vec![bare(OpName::Purge)], "/r/secret"),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "comment",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_match_allows_caller() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(OpName::Purge, &["eduardo-burgos"])],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_miss_refuses_caller() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(OpName::Purge, &["eduardo-burgos"])],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("someone-else", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    let err = check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOpNotExcepted {
            caller: _,
            op: _,
            source_file: _,
            target: _
        })
    ));
    let chain = format!("{err:#}");
    assert!(chain.contains("someone-else"));
}

#[test]
fn exceptions_two_entries_first_matches() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(
                OpName::Write,
                &["eduardo-burgos", "remargin_dev_agent"],
            )],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "write",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_two_entries_second_matches() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(
                OpName::Write,
                &["eduardo-burgos", "remargin_dev_agent"],
            )],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("remargin_dev_agent", AuthorType::Agent, Mode::Strict);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "write",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_empty_list_acts_as_blanket() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(vec![with_exceptions(OpName::Purge, &[])], "/r/secret"),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    let err = check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp {
            op: _,
            source_file: _,
            target: _
        })
    ));
}

#[test]
fn exceptions_mixed_bare_and_full_list() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![
                bare(OpName::Sign),
                with_exceptions(OpName::Purge, &["eduardo-burgos"]),
                bare(OpName::Delete),
            ],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();

    let sign_err = check_against_resolved_for_caller(
        &system,
        "sign",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap_err();
    assert!(matches!(
        sign_err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp {
            op: _,
            source_file: _,
            target: _
        })
    ));

    check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();

    let delete_err = check_against_resolved_for_caller(
        &system,
        "delete",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap_err();
    assert!(matches!(
        delete_err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp {
            op: _,
            source_file: _,
            target: _
        })
    ));
}

#[test]
fn exceptions_union_semantics_bare_wins_over_full() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: vec![
            ResolvedDenyOps {
                ops: vec![bare(OpName::Purge)],
                path: PathBuf::from("/r/secret"),
                source_file: PathBuf::from("/r/.remargin.yaml"),
            },
            ResolvedDenyOps {
                ops: vec![with_exceptions(OpName::Purge, &["eduardo-burgos"])],
                path: PathBuf::from("/r/secret"),
                source_file: PathBuf::from("/r/.remargin.yaml"),
            },
        ],
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    let err = check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::DeniedOp {
            op: _,
            source_file: _,
            target: _
        })
    ));
}

#[test]
fn exceptions_honored_in_open_mode() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(OpName::Purge, &["eduardo-burgos"])],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Open);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_honored_in_registered_mode() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(OpName::Purge, &["eduardo-burgos"])],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Registered);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_honored_in_strict_mode() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(OpName::Purge, &["eduardo-burgos"])],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("eduardo-burgos", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_entry_path_does_not_cover_target_allows() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(vec![bare(OpName::Purge)], "/r/scratch"),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/other.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn exceptions_match_via_identity_id() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: deny_ops_items(
            vec![with_exceptions(OpName::Purge, &["alice-id"])],
            "/r/secret",
        ),
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = CallerInfo {
        author_type: Some(AuthorType::Human),
        identity_id: Some(String::from("alice-id")),
        identity_name: Some(String::from("alice-display-name")),
        mode: Mode::Strict,
    };
    let system = MemorySystem::new();
    check_against_resolved_for_caller(
        &system,
        "purge",
        Path::new("/r/secret/x.md"),
        &resolved,
        &caller,
    )
    .unwrap();
}

fn ssh_test_system() -> MemorySystem {
    MemorySystem::new().with_env("HOME", "/h").unwrap()
}

#[test]
fn strict_agent_denied_default_ssh_read() {
    let system = ssh_test_system();
    let resolved = ResolvedPermissions::default();
    let caller = caller("nimbus", AuthorType::Agent, Mode::Strict);
    let err = check_against_resolved_for_caller(
        &system,
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
    let system = ssh_test_system();
    let resolved = ResolvedPermissions::default();
    let caller = caller("alice", AuthorType::Human, Mode::Strict);
    check_against_resolved_for_caller(
        &system,
        "get",
        Path::new("/h/.ssh/id_ed25519"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn strict_agent_default_ssh_override_via_user_exception() {
    let system = ssh_test_system();
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: vec![ResolvedDenyOps {
            ops: vec![with_exceptions(OpName::Get, &["nimbus"])],
            path: PathBuf::from("/h/.ssh"),
            source_file: PathBuf::from("/r/.remargin.yaml"),
        }],
        trusted_roots: Vec::new(),
        trusted_roots_lock: None,
    };
    let caller = caller("nimbus", AuthorType::Agent, Mode::Strict);
    check_against_resolved_for_caller(
        &system,
        "get",
        Path::new("/h/.ssh/id_ed25519"),
        &resolved,
        &caller,
    )
    .unwrap();
}

#[test]
fn open_mode_agent_can_read_ssh_no_synthesized_default() {
    let system = ssh_test_system();
    let resolved = ResolvedPermissions::default();
    let caller = caller("nimbus", AuthorType::Agent, Mode::Open);
    check_against_resolved_for_caller(
        &system,
        "get",
        Path::new("/h/.ssh/id_ed25519"),
        &resolved,
        &caller,
    )
    .unwrap();
}

/// With no `trusted_roots:` key the guard is silent; the call site supplies the implicit root.
#[test]
fn rem_djfx_trusted_roots_key_absent_falls_back_to_open() {
    let system = realm_with("permissions:\n  allow_dot_folders: ['.git']\n");
    pre_mutate_check(&system, "write", Path::new("/r/anywhere/foo.md")).unwrap();
    pre_mutate_check(&system, "get", Path::new("/elsewhere/x.md")).unwrap();
}

#[test]
fn rem_djfx_explicit_empty_trusted_roots_locks_reads_and_writes() {
    let system = realm_with("permissions:\n  trusted_roots: []\n");
    let err_w = pre_mutate_check(&system, "write", Path::new("/r/foo.md")).unwrap_err();
    assert!(outside_allowed_match(&err_w, "write", "/r/.remargin.yaml"));
    let err_r = pre_mutate_check(&system, "get", Path::new("/r/foo.md")).unwrap_err();
    assert!(outside_allowed_match(&err_r, "get", "/r/.remargin.yaml"));
}

/// A deeper `trusted_roots: []` locks, but entries inherited from shallower files stay reachable.
#[test]
fn rem_djfx_lock_does_not_drop_inherited_parent_roots() {
    let parent = "permissions:\n  trusted_roots:\n    - path: top\n";
    let child = "permissions:\n  trusted_roots: []\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/r/sub"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/r/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    pre_mutate_check(&system, "write", Path::new("/r/top/foo.md")).unwrap();
    let err = pre_mutate_check(&system, "write", Path::new("/r/sub/foo.md")).unwrap_err();
    assert!(matches!(
        err.downcast_ref::<OpGuardError>(),
        Some(OpGuardError::OutsideAllowedRoots {
            op: _,
            source_file: _,
            target: _
        })
    ));
}

#[test]
fn rem_djfx_deny_ops_wins_for_reads() {
    let system = realm_with(
        "permissions:\n  trusted_roots:\n    - path: '*'\n  deny_ops:\n    - path: secret\n      ops: [get]\n",
    );
    let err = pre_mutate_check(&system, "get", Path::new("/r/secret/x.md")).unwrap_err();
    assert!(denied_op_match(&err, "get", "/r/.remargin.yaml"));
}

#[test]
fn rem_djfx_remargin_dot_folder_read_parity() {
    let resolved = ResolvedPermissions {
        allow_dot_folders: Vec::new(),
        cli_allowed: None,
        deny_ops: Vec::new(),
        trusted_roots: vec![ResolvedTrustedRoot {
            also_deny_bash: Vec::new(),
            cli_allowed: false,
            path: TrustedRootPath::Wildcard {
                realm_root: PathBuf::from("/r"),
            },
            source_file: PathBuf::from("/r/.remargin.yaml"),
        }],
        trusted_roots_lock: None,
    };
    let system = MemorySystem::new();
    check_against_resolved(
        &system,
        "get",
        Path::new("/r/.remargin/state.yaml"),
        &resolved,
    )
    .unwrap();
}
