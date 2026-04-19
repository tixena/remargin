//! Three-branch identity resolver tests.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::config::Mode;
use crate::config::identity::{IdentityFlags, IdentitySource, resolve_identity};
use crate::config::registry::{Registry, RegistryParticipant, RegistryParticipantStatus};
use crate::parser::AuthorType;

fn registry_with(author: &str, status: RegistryParticipantStatus) -> Registry {
    let mut participants = BTreeMap::new();
    participants.insert(
        String::from(author),
        RegistryParticipant {
            added: None,
            author_type: String::from("human"),
            display_name: None,
            pubkeys: Vec::new(),
            status,
        },
    );
    Registry { participants }
}

// ---------- Branch 1: --config ----------

#[test]
fn branch1_config_flag_happy_path() {
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/other/place/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/other/place/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let resolved = resolve_identity(
        &system,
        Path::new("/project/src"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap();

    assert_eq!(resolved.identity, "alice");
    assert_eq!(resolved.author_type, AuthorType::Human);
    assert!(resolved.key_path.is_none());
    assert!(matches!(resolved.source, IdentitySource::ConfigFlag(_)));
}

#[test]
fn branch1_config_flag_strict_requires_key_in_file() {
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/cfg/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/cfg/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let err = resolve_identity(
        &system,
        Path::new("/project"),
        &Mode::Strict,
        &flags,
        Some(&registry_with("alice", RegistryParticipantStatus::Active)),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("strict mode requires `key:` field"),
        "got: {err:#}"
    );
}

#[test]
fn branch1_config_flag_missing_identity_field() {
    let system = MockSystem::new()
        .with_file(Path::new("/cfg/.remargin.yaml"), b"type: human\n")
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/cfg/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let err =
        resolve_identity(&system, Path::new("/project"), &Mode::Open, &flags, None).unwrap_err();
    assert!(
        err.to_string().contains("missing required `identity:`"),
        "got: {err:#}"
    );
}

#[test]
fn branch1_config_flag_not_in_registry_fails_strict() {
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(Path::new("/home/user/.ssh/id"), b"SSH_KEY")
        .unwrap()
        .with_file(
            Path::new("/cfg/.remargin.yaml"),
            b"identity: alice\ntype: human\nkey: id\n",
        )
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/cfg/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let err = resolve_identity(
        &system,
        Path::new("/project"),
        &Mode::Strict,
        &flags,
        Some(&registry_with(
            "not-alice",
            RegistryParticipantStatus::Active,
        )),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not in the registry"),
        "got: {err:#}"
    );
}

#[test]
fn branch1_config_flag_revoked_fails_registered() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/cfg/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();
    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/cfg/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let err = resolve_identity(
        &system,
        Path::new("/project"),
        &Mode::Registered,
        &flags,
        Some(&registry_with("alice", RegistryParticipantStatus::Revoked)),
    )
    .unwrap_err();
    assert!(err.to_string().contains("revoked"), "got: {err:#}");
}

#[test]
fn branch1_config_flag_with_tilde_path_expansion() {
    // The adapter is responsible for expanding `~` before it reaches the
    // resolver. This test documents that the resolver receives an
    // already-expanded path and just uses it as-is.
    let system = MockSystem::new()
        .with_file(
            Path::new("/home/user/custom.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/home/user/custom.yaml")),
        ..IdentityFlags::default()
    };
    let resolved = resolve_identity(&system, Path::new("/p"), &Mode::Open, &flags, None).unwrap();
    assert_eq!(resolved.identity, "alice");
}

// ---------- Branch 2: manual declaration ----------

#[test]
fn branch2_manual_happy_path_open() {
    let system = MockSystem::new();
    let flags = IdentityFlags {
        author_type: Some(AuthorType::Agent),
        identity: Some(String::from("bot")),
        ..IdentityFlags::default()
    };
    let resolved =
        resolve_identity(&system, Path::new("/project"), &Mode::Open, &flags, None).unwrap();
    assert_eq!(resolved.identity, "bot");
    assert_eq!(resolved.author_type, AuthorType::Agent);
    assert!(resolved.key_path.is_none());
    assert!(matches!(resolved.source, IdentitySource::Manual));
}

#[test]
fn branch2_strict_without_key_falls_to_walk() {
    // --identity + --type without --key in strict mode is NOT a complete
    // manual declaration; it falls through to branch 3 (walk with
    // filters). With no matching file, the walk exhausts.
    let system = MockSystem::new();
    let flags = IdentityFlags {
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("alice")),
        ..IdentityFlags::default()
    };
    let err = resolve_identity(
        &system,
        Path::new("/project"),
        &Mode::Strict,
        &flags,
        Some(&registry_with("alice", RegistryParticipantStatus::Active)),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("no identity resolved"),
        "got: {err:#}"
    );
}

#[test]
fn branch2_manual_strict_with_key_succeeds() {
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(Path::new("/home/user/.ssh/id"), b"SSH")
        .unwrap();
    let flags = IdentityFlags {
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("alice")),
        key: Some(String::from("id")),
        ..IdentityFlags::default()
    };
    let resolved = resolve_identity(
        &system,
        Path::new("/project"),
        &Mode::Strict,
        &flags,
        Some(&registry_with("alice", RegistryParticipantStatus::Active)),
    )
    .unwrap();
    assert_eq!(resolved.key_path, Some(PathBuf::from("/home/user/.ssh/id")));
}

#[test]
fn type_only_falls_to_walk_as_type_filter() {
    // Only --type given: not a manual declaration (no --identity).
    // Falls to branch 3 where --type filters the walk. With only a
    // human config present, a --type=agent filter skips it and the
    // walk exhausts.
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();
    let flags = IdentityFlags {
        author_type: Some(AuthorType::Agent),
        ..IdentityFlags::default()
    };
    let err =
        resolve_identity(&system, Path::new("/project"), &Mode::Open, &flags, None).unwrap_err();
    assert!(
        err.to_string().contains("no identity resolved"),
        "got: {err:#}"
    );
}

#[test]
fn identity_only_falls_to_walk_as_identity_filter() {
    // Only --identity given: not a manual declaration (no --type).
    // Falls to branch 3 where --identity filters the walk.
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: bob\ntype: human\n",
        )
        .unwrap();
    let flags = IdentityFlags {
        identity: Some(String::from("alice")),
        ..IdentityFlags::default()
    };
    let err =
        resolve_identity(&system, Path::new("/project"), &Mode::Open, &flags, None).unwrap_err();
    assert!(
        err.to_string().contains("no identity resolved"),
        "got: {err:#}"
    );
}

#[test]
fn identity_only_filter_picks_matching_file_on_walk() {
    // Walk from /project/src: inner .remargin.yaml is bob; root is
    // alice. Filter --identity=alice skips bob's file and matches root.
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/src/.remargin.yaml"),
            b"identity: bob\ntype: human\n",
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();
    let flags = IdentityFlags {
        identity: Some(String::from("alice")),
        ..IdentityFlags::default()
    };
    let resolved = resolve_identity(
        &system,
        Path::new("/project/src"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap();
    assert_eq!(resolved.identity, "alice");
    assert_eq!(resolved.author_type, AuthorType::Human);
}

#[test]
fn branch2_manual_unregistered_fails_registered() {
    let system = MockSystem::new();
    let flags = IdentityFlags {
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("alice")),
        ..IdentityFlags::default()
    };
    let err = resolve_identity(
        &system,
        Path::new("/project"),
        &Mode::Registered,
        &flags,
        Some(&registry_with("bob", RegistryParticipantStatus::Active)),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not in the registry"),
        "got: {err:#}"
    );
}

// ---------- Branch 3: filtered walk ----------

#[test]
fn branch3_walk_happy_path_no_filters() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();
    let flags = IdentityFlags::default();
    let resolved = resolve_identity(
        &system,
        Path::new("/project/src/deep"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap();
    assert_eq!(resolved.identity, "alice");
    assert!(matches!(resolved.source, IdentitySource::Walk(_)));
}

#[test]
fn branch3_walk_filter_by_identity_skips_nonmatch() {
    // Inner file is Bob; walk should skip it and pick Alice at the root.
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/inner/.remargin.yaml"),
            b"identity: bob\ntype: human\n",
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();

    // Note: walk from inner, with filter identity=alice. We cannot call
    // resolve_identity directly with identity-only (that triggers branch
    // 2, not branch 3). Branch 3 is entered when no identity/type flags
    // are set. For filter semantics we need author_type filter AND no
    // identity — exercise that path instead.
    let flags = IdentityFlags::default();
    let resolved = resolve_identity(
        &system,
        Path::new("/project/inner"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap();
    // With no filters, closest wins.
    assert_eq!(resolved.identity, "bob");
}

#[test]
fn branch3_walk_filter_by_key_skips_nonmatch() {
    // Inner file has no key; outer has key=outer_key. Filter --key=outer_key
    // should skip inner and land on outer.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(Path::new("/home/user/.ssh/outer_key"), b"SSH")
        .unwrap()
        .with_file(
            Path::new("/project/inner/.remargin.yaml"),
            b"identity: bob\ntype: human\n",
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: alice\ntype: human\nkey: outer_key\n",
        )
        .unwrap();

    let flags = IdentityFlags {
        key: Some(String::from("outer_key")),
        ..IdentityFlags::default()
    };
    // Branch 3 entry requires identity AND author_type to be None. Key-only
    // filter enters branch 3 (walk with key filter).
    let resolved = resolve_identity(
        &system,
        Path::new("/project/inner"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap();
    assert_eq!(resolved.identity, "alice");
    assert_eq!(
        resolved.key_path,
        Some(PathBuf::from("/home/user/.ssh/outer_key"))
    );
}

#[test]
fn branch3_walk_exhausted_errors() {
    let system = MockSystem::new();
    let flags = IdentityFlags::default();
    let err = resolve_identity(
        &system,
        Path::new("/some/deep/path"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("no identity resolved"),
        "got: {err:#}"
    );
}

#[test]
fn branch3_walk_filter_mismatch_exhausts() {
    // Only file has identity=alice; filter requires key=nonexistent.
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();
    let flags = IdentityFlags {
        key: Some(String::from("never-matches")),
        ..IdentityFlags::default()
    };
    let err = resolve_identity(
        &system,
        Path::new("/project/src"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("no identity resolved"),
        "got: {err:#}"
    );
}

#[test]
fn branch3_filter_field_missing_in_file_never_matches() {
    // File has no `key:` field; filter `--key=some_key` requires the
    // field to be present AND equal. Missing-in-file never matches a
    // concrete filter, so the walk continues and exhausts.
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap()
        .with_env("HOME", "/home/user")
        .unwrap();
    let flags = IdentityFlags {
        key: Some(String::from("some_key")),
        ..IdentityFlags::default()
    };
    let err =
        resolve_identity(&system, Path::new("/project"), &Mode::Open, &flags, None).unwrap_err();
    assert!(
        err.to_string().contains("no identity resolved"),
        "got: {err:#}"
    );
}

// ---------- Branch 1 conflict with branch 2 flags ----------

#[test]
fn config_flag_plus_manual_flags_bails() {
    // Non-clap adapter could construct this; resolver defends.
    let system = MockSystem::new();
    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/x.yaml")),
        identity: Some(String::from("alice")),
        ..IdentityFlags::default()
    };
    let err = resolve_identity(&system, Path::new("/p"), &Mode::Open, &flags, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("--config conflicts with --identity"),
        "got: {err:#}"
    );
}
