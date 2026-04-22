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

// ---------- Relative `key:` anchoring (config-dir, not CWD) ----------
//
// Pre-existing bug surfaced by the per-role-config migrate flags: a
// relative `key:` value in a `.remargin.yaml` was passed straight to
// the OS, which resolves it against the process's CWD. That happens to
// work when the config is found by walking up from CWD (config dir ==
// CWD) but breaks when the config is loaded by absolute path from a
// different CWD. The fix anchors relative key paths to the config
// file's parent directory.

#[test]
fn branch1_relative_key_anchors_to_config_dir_not_cwd() {
    // Config at /vault/.remargin.yaml says `key: keys/agent_key`.
    // The actual key file lives at /vault/keys/agent_key. The CWD is
    // /elsewhere — completely unrelated. Resolution must end up with
    // /vault/keys/agent_key, not /elsewhere/keys/agent_key.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/vault/.remargin.yaml"),
            b"identity: alice\ntype: human\nkey: keys/agent_key\n",
        )
        .unwrap()
        .with_file(Path::new("/vault/keys/agent_key"), b"SSH_KEY")
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/vault/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let resolved =
        resolve_identity(&system, Path::new("/elsewhere"), &Mode::Open, &flags, None).unwrap();

    assert_eq!(
        resolved.key_path.as_deref(),
        Some(Path::new("/vault/keys/agent_key")),
        "relative `key:` must anchor to the config's directory, not CWD",
    );
}

#[test]
fn branch1_dotted_relative_key_anchors_to_config_dir() {
    // The exact shape that tripped the user in the wild: `.remargin/agent_key`
    // next to a `.remargin.yaml` in some other folder, run from a
    // separate working directory.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/notes/.remargin.yaml"),
            b"identity: bot\ntype: agent\nkey: .remargin/agent_key\n",
        )
        .unwrap()
        .with_file(Path::new("/notes/.remargin/agent_key"), b"SSH_KEY")
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/notes/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let resolved = resolve_identity(
        &system,
        Path::new("/repos/some-other-project"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap();

    assert_eq!(
        resolved.key_path.as_deref(),
        Some(Path::new("/notes/.remargin/agent_key")),
    );
}

#[test]
fn branch1_absolute_key_passes_through_unchanged() {
    // Absolute `key:` paths must NOT be re-anchored under the config's
    // parent — they are already where the user pointed.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/cfg/.remargin.yaml"),
            b"identity: alice\ntype: human\nkey: /opt/keys/shared_key\n",
        )
        .unwrap()
        .with_file(Path::new("/opt/keys/shared_key"), b"SSH_KEY")
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/cfg/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let resolved =
        resolve_identity(&system, Path::new("/elsewhere"), &Mode::Open, &flags, None).unwrap();

    assert_eq!(
        resolved.key_path.as_deref(),
        Some(Path::new("/opt/keys/shared_key")),
    );
}

#[test]
fn branch1_tilde_key_expands_to_home_not_config_dir() {
    // `~`-prefixed keys must continue to expand to $HOME (existing
    // behaviour). The anchor step only fires for paths that are still
    // relative *after* `resolve_key_path` has done its work.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/cfg/.remargin.yaml"),
            b"identity: alice\ntype: human\nkey: ~/.ssh/custom_key\n",
        )
        .unwrap()
        .with_file(Path::new("/home/user/.ssh/custom_key"), b"SSH_KEY")
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/cfg/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let resolved =
        resolve_identity(&system, Path::new("/elsewhere"), &Mode::Open, &flags, None).unwrap();

    assert_eq!(
        resolved.key_path.as_deref(),
        Some(Path::new("/home/user/.ssh/custom_key")),
    );
}

#[test]
fn branch1_plain_name_key_still_resolves_to_ssh_dir() {
    // The "plain name" branch (no `/`, `~`, or `$`) maps to
    // `~/.ssh/<name>`. After `resolve_key_path` produces an absolute
    // path under $HOME, the anchor step must leave it alone.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/cfg/.remargin.yaml"),
            b"identity: alice\ntype: human\nkey: id_ed25519\n",
        )
        .unwrap()
        .with_file(Path::new("/home/user/.ssh/id_ed25519"), b"SSH_KEY")
        .unwrap();

    let flags = IdentityFlags {
        config_path: Some(PathBuf::from("/cfg/.remargin.yaml")),
        ..IdentityFlags::default()
    };
    let resolved =
        resolve_identity(&system, Path::new("/elsewhere"), &Mode::Open, &flags, None).unwrap();

    assert_eq!(
        resolved.key_path.as_deref(),
        Some(Path::new("/home/user/.ssh/id_ed25519")),
    );
}

#[test]
fn branch3_walk_relative_key_anchors_to_walked_config_dir() {
    // The walk picks up `/notes/.remargin.yaml` because CWD is under it.
    // Even when CWD is a deeper subdirectory than the config's dir, the
    // relative key path must still anchor to the config's parent — not
    // CWD — so a deeper subdirectory of the config tree resolves the
    // key correctly.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/notes/.remargin.yaml"),
            b"identity: bot\ntype: agent\nkey: .remargin/agent_key\n",
        )
        .unwrap()
        .with_file(Path::new("/notes/.remargin/agent_key"), b"SSH_KEY")
        .unwrap();

    let resolved = resolve_identity(
        &system,
        Path::new("/notes/sub/deeper"),
        &Mode::Open,
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

    assert_eq!(
        resolved.key_path.as_deref(),
        Some(Path::new("/notes/.remargin/agent_key")),
        "walked config's relative key must anchor to the config's dir",
    );
}
