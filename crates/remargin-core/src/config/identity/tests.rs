//! Three-branch identity resolver tests.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use os_shim::mock::MemorySystem;

use crate::config::Mode;
use crate::config::ResolvedConfig;
use crate::config::identity::{IdentityFlags, IdentityReport, IdentitySource, resolve_identity};
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

#[test]
fn branch1_config_flag_happy_path() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

#[test]
fn branch2_manual_happy_path_open() {
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new();
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

#[test]
fn branch3_walk_happy_path_no_filters() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

    let flags = IdentityFlags::default();
    let resolved = resolve_identity(
        &system,
        Path::new("/project/inner"),
        &Mode::Open,
        &flags,
        None,
    )
    .unwrap();
    assert_eq!(resolved.identity, "bob");
}

#[test]
fn branch3_walk_filter_by_key_skips_nonmatch() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new();
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

#[test]
fn config_flag_plus_manual_flags_bails() {
    let system = MemorySystem::new();
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

#[test]
fn branch1_relative_key_anchors_to_config_dir_not_cwd() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

#[test]
fn identity_source_display_renders_each_variant() {
    assert_eq!(
        IdentitySource::ConfigFlag(PathBuf::from("/etc/cfg.yaml")).to_string(),
        "--config /etc/cfg.yaml"
    );
    assert_eq!(IdentitySource::Manual.to_string(), "manual CLI flags");
    assert_eq!(
        IdentitySource::Walk(PathBuf::from("/p/.remargin.yaml")).to_string(),
        "walk match at /p/.remargin.yaml"
    );
}

fn config_with(identity: Option<&str>, author_type: Option<AuthorType>) -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type,
        identity: identity.map(String::from),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

#[test]
fn identity_report_from_resolved_with_no_identity_is_not_found() {
    let cfg = config_with(None, None);
    let report = IdentityReport::from_resolved(&cfg);
    assert!(!report.found);
    assert!(report.identity.is_none());
}

#[test]
fn identity_report_from_resolved_populates_every_field_when_present() {
    let mut cfg = config_with(Some("alice"), Some(AuthorType::Human));
    cfg.key_path = Some(PathBuf::from("/keys/alice.pub"));
    cfg.source_path = Some(PathBuf::from("/proj/.remargin.yaml"));

    let report = IdentityReport::from_resolved(&cfg);
    assert!(report.found);
    assert_eq!(report.identity.as_deref(), Some("alice"));
    assert_eq!(report.author_type.as_deref(), Some("human"));
    assert_eq!(report.key.as_deref(), Some("/keys/alice.pub"));
    assert_eq!(report.mode.as_deref(), Some("open"));
    assert_eq!(report.path.as_deref(), Some("/proj/.remargin.yaml"));
}

#[test]
fn identity_report_not_found_has_all_fields_empty() {
    let report = IdentityReport::not_found();
    assert!(!report.found);
    assert!(report.identity.is_none());
    assert!(report.author_type.is_none());
    assert!(report.key.is_none());
    assert!(report.mode.is_none());
    assert!(report.path.is_none());
}

/// Clap refuses this combination; the resolver must too, for adapters that bypass clap.
#[test]
fn resolve_identity_rejects_config_path_mixed_with_manual_flags() {
    let system = MemorySystem::new();
    let flags = IdentityFlags {
        author_type: Some(AuthorType::Human),
        config_path: Some(PathBuf::from("/cfg.yaml")),
        identity: Some(String::from("alice")),
        key: None,
    };
    let err = resolve_identity(&system, Path::new("/"), &Mode::Open, &flags, None).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("--config conflicts with"),
        "expected exclusivity diagnostic, got: {msg}"
    );
}
