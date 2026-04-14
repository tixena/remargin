//! Tests for the config and registry loader.

use std::path::Path;

use os_shim::mock::MockSystem;

use crate::parser::AuthorType;

use super::registry::RegistryParticipantStatus;
use super::{
    CliOverrides, Mode, ResolvedConfig, load_config, load_config_filtered, load_registry,
    resolve_key_path, resolve_mode,
};

fn minimal_config_yaml(identity: &str) -> String {
    format!("identity: {identity}\n")
}

fn full_config_yaml() -> &'static str {
    "\
identity: eduardo
type: human
mode: strict
key: id_ed25519
assets_dir: .assets
ignore:
  - node_modules
  - target
"
}

fn registry_yaml() -> &'static str {
    "\
participants:
  eduardo:
    type: human
    status: active
    pubkeys:
      - ssh-ed25519 AAAAC3NzaC1...
      - ssh-ed25519 AAAAC3NzaC2...
    added: '2026-01-01'
  claude:
    type: agent
    status: active
    pubkeys: []
  revoked_user:
    type: human
    status: revoked
    pubkeys: []
"
}

#[test]
fn walk_up_finds_config() {
    let system = MockSystem::new()
        .with_dir(Path::new("/project/src/deep"))
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            minimal_config_yaml("eduardo").as_bytes(),
        )
        .unwrap();

    let config = load_config(&system, Path::new("/project/src/deep"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));
}

#[test]
fn walk_up_finds_nothing() {
    let system = MockSystem::new()
        .with_dir(Path::new("/empty/path"))
        .unwrap();

    let config = load_config(&system, Path::new("/empty/path")).unwrap();
    assert!(config.is_none());
}

#[test]
fn config_and_registry_at_different_levels() {
    let system = MockSystem::new()
        .with_dir(Path::new("/project/src"))
        .unwrap()
        .with_file(
            Path::new("/project/src/.remargin.yaml"),
            minimal_config_yaml("eduardo").as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let config = load_config(&system, Path::new("/project/src"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));

    let registry = load_registry(&system, Path::new("/project/src"))
        .unwrap()
        .unwrap();
    assert!(registry.participants.contains_key("eduardo"));
}

#[test]
fn full_config_parse() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            full_config_yaml().as_bytes(),
        )
        .unwrap();

    let config = load_config(&system, Path::new("/project"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));
    assert_eq!(config.author_type.as_deref(), Some("human"));
    assert_eq!(config.mode, Mode::Strict);
    assert_eq!(config.key.as_deref(), Some("id_ed25519"));
    assert_eq!(config.assets_dir, ".assets");
    assert_eq!(config.ignore, vec!["node_modules", "target"]);
}

#[test]
fn minimal_config_defaults() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            minimal_config_yaml("bob").as_bytes(),
        )
        .unwrap();

    let config = load_config(&system, Path::new("/project"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("bob"));
    assert_eq!(config.mode, Mode::Open);
    assert_eq!(config.assets_dir, "assets");
    assert!(config.ignore.is_empty());
    assert!(config.key.is_none());
    assert!(config.author_type.is_none());
}

#[test]
fn registry_revoked_participant() {
    let system = MockSystem::new()
        .with_file(Path::new("/project/.remargin.yaml"), b"mode: registered\n")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let config = load_config(&system, Path::new("/project")).unwrap();
    let registry = load_registry(&system, Path::new("/project")).unwrap();
    let resolved =
        ResolvedConfig::resolve(&system, config, registry, &CliOverrides::default()).unwrap();

    resolved.can_post("eduardo").unwrap();

    let err = resolved.can_post("revoked_user").unwrap_err();
    assert!(
        format!("{err}").contains("revoked"),
        "expected revoked error, got: {err}"
    );
}

#[test]
fn key_rotation_multiple_pubkeys() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let registry = load_registry(&system, Path::new("/project"))
        .unwrap()
        .unwrap();
    let eduardo = &registry.participants["eduardo"];
    assert_eq!(eduardo.pubkeys.len(), 2);
}

#[test]
fn key_shorthand_plain_name() {
    let system = MockSystem::new().with_env("HOME", "/home/user").unwrap();

    let path = resolve_key_path(&system, "id_ed25519").unwrap();
    assert_eq!(path, Path::new("/home/user/.ssh/id_ed25519"));
}

#[test]
fn key_path_literal_tilde() {
    let system = MockSystem::new().with_env("HOME", "/home/user").unwrap();

    let path = resolve_key_path(&system, "~/.remargin/keys/foo.key").unwrap();
    assert_eq!(path, Path::new("/home/user/.remargin/keys/foo.key"));
}

#[test]
fn key_path_literal_absolute() {
    let system = MockSystem::new();

    let path = resolve_key_path(&system, "/etc/keys/foo.key").unwrap();
    assert_eq!(path, Path::new("/etc/keys/foo.key"));
}

#[test]
fn cli_override_identity() {
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: config_user\ntype: human\nmode: open\nkey: id_ed25519\n",
        )
        .unwrap();

    let config = load_config(&system, Path::new("/project")).unwrap();
    let cli = CliOverrides {
        identity: Some("cli_user"),
        author_type: Some("agent"),
        mode: Some("strict"),
        key: Some("~/.remargin/mykey"),
        assets_dir: Some("my_assets"),
    };
    let resolved = ResolvedConfig::resolve(&system, config, None, &cli).unwrap();

    assert_eq!(resolved.identity.as_deref(), Some("cli_user"));
    assert_eq!(resolved.author_type, Some(AuthorType::Agent));
    assert_eq!(resolved.mode, Mode::Strict);
    assert_eq!(
        resolved.key_path,
        Some(Path::new("/home/user/.remargin/mykey").to_path_buf())
    );
    assert_eq!(resolved.assets_dir, "my_assets");
}

#[test]
fn open_mode_any_author() {
    let system = MockSystem::new();
    let resolved = ResolvedConfig::resolve(&system, None, None, &CliOverrides::default()).unwrap();

    resolved.can_post("unknown_user").unwrap();
}

#[test]
fn strict_mode_unregistered() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let registry = load_registry(&system, Path::new("/project")).unwrap();
    let cli = CliOverrides {
        mode: Some("strict"),
        ..CliOverrides::default()
    };
    let resolved = ResolvedConfig::resolve(&system, None, registry, &cli).unwrap();

    let err = resolved.can_post("stranger").unwrap_err();
    assert!(
        format!("{err}").contains("not registered"),
        "expected 'not registered' error, got: {err}"
    );
}

#[test]
fn strict_mode_requires_signature() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let registry = load_registry(&system, Path::new("/project")).unwrap();
    let cli = CliOverrides {
        mode: Some("strict"),
        ..CliOverrides::default()
    };
    let resolved = ResolvedConfig::resolve(&system, None, registry, &cli).unwrap();

    resolved.can_post("eduardo").unwrap();

    assert!(resolved.requires_signature("eduardo"));

    assert!(!resolved.requires_signature("stranger"));
}

#[test]
fn registered_mode_no_registry() {
    let system = MockSystem::new();
    let cli = CliOverrides {
        mode: Some("registered"),
        ..CliOverrides::default()
    };
    let resolved = ResolvedConfig::resolve(&system, None, None, &cli).unwrap();

    let err = resolved.can_post("anyone").unwrap_err();
    assert!(
        format!("{err}").contains("no registry found"),
        "expected 'no registry found' error, got: {err}"
    );
}

#[test]
fn registry_status_default_active() {
    let yaml = "\
participants:
  nostatus:
    type: human
";
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            yaml.as_bytes(),
        )
        .unwrap();

    let registry = load_registry(&system, Path::new("/project"))
        .unwrap()
        .unwrap();
    assert_eq!(
        registry.participants["nostatus"].status,
        RegistryParticipantStatus::Active
    );
}

fn typed_config_yaml(identity: &str, author_type: &str) -> String {
    format!("identity: {identity}\ntype: {author_type}\n")
}

#[test]
fn type_filter_matches_first_config() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            typed_config_yaml("eduardo", "human").as_bytes(),
        )
        .unwrap();

    let config = load_config_filtered(&system, Path::new("/project"), Some("human"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));
    assert_eq!(config.author_type.as_deref(), Some("human"));
}

#[test]
fn type_filter_skips_non_matching_finds_higher() {
    let system = MockSystem::new()
        .with_dir(Path::new("/home/project/src"))
        .unwrap()
        .with_file(
            Path::new("/home/project/src/.remargin.yaml"),
            typed_config_yaml("agent_bot", "agent").as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/home/.remargin.yaml"),
            typed_config_yaml("eduardo", "human").as_bytes(),
        )
        .unwrap();

    let config = load_config_filtered(&system, Path::new("/home/project/src"), Some("human"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));
    assert_eq!(config.author_type.as_deref(), Some("human"));
}

#[test]
fn type_filter_no_match() {
    let system = MockSystem::new()
        .with_dir(Path::new("/project/src"))
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            typed_config_yaml("agent_bot", "agent").as_bytes(),
        )
        .unwrap();

    let config = load_config_filtered(&system, Path::new("/project/src"), Some("human")).unwrap();
    assert!(config.is_none());
}

#[test]
fn no_filter_backward_compat() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            typed_config_yaml("agent_bot", "agent").as_bytes(),
        )
        .unwrap();

    let config = load_config_filtered(&system, Path::new("/project"), None)
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("agent_bot"));
    assert_eq!(config.author_type.as_deref(), Some("agent"));
}

#[test]
fn no_filter_no_type_field() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            minimal_config_yaml("bob").as_bytes(),
        )
        .unwrap();

    let config = load_config_filtered(&system, Path::new("/project"), None)
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("bob"));
    assert!(config.author_type.is_none());
}

#[test]
fn type_filter_skips_config_without_type() {
    let system = MockSystem::new()
        .with_dir(Path::new("/project/sub"))
        .unwrap()
        .with_file(
            Path::new("/project/sub/.remargin.yaml"),
            minimal_config_yaml("bob").as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            typed_config_yaml("eduardo", "human").as_bytes(),
        )
        .unwrap();

    let config = load_config_filtered(&system, Path::new("/project/sub"), Some("human"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));
    assert_eq!(config.author_type.as_deref(), Some("human"));
}

#[test]
fn type_filter_multiple_configs_selects_correct() {
    let system = MockSystem::new()
        .with_dir(Path::new("/a/b"))
        .unwrap()
        .with_file(
            Path::new("/a/b/.remargin.yaml"),
            typed_config_yaml("human_user", "human").as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/a/.remargin.yaml"),
            typed_config_yaml("agent_bot", "agent").as_bytes(),
        )
        .unwrap();

    let config = load_config_filtered(&system, Path::new("/a/b"), Some("agent"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("agent_bot"));
    assert_eq!(config.author_type.as_deref(), Some("agent"));
}

#[test]
fn load_config_wrapper_still_works() {
    let system = MockSystem::new()
        .with_dir(Path::new("/project/src"))
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            typed_config_yaml("agent_bot", "agent").as_bytes(),
        )
        .unwrap();

    let config = load_config(&system, Path::new("/project/src"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("agent_bot"));
}

#[test]
fn resolve_mode_finds_vault_root_mode() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"mode: strict\ntype: human\nidentity: eduardo\n",
        )
        .unwrap();

    let resolved = resolve_mode(&system, Path::new("/project")).unwrap();
    assert_eq!(resolved.mode, Mode::Strict);
    assert_eq!(
        resolved.source.as_deref(),
        Some(Path::new("/project/.remargin.yaml"))
    );
}

#[test]
fn resolve_mode_walks_up() {
    let system = MockSystem::new()
        .with_dir(Path::new("/project/src/deep"))
        .unwrap()
        .with_file(Path::new("/project/.remargin.yaml"), b"mode: registered\n")
        .unwrap();

    let resolved = resolve_mode(&system, Path::new("/project/src/deep")).unwrap();
    assert_eq!(resolved.mode, Mode::Registered);
    assert_eq!(
        resolved.source.as_deref(),
        Some(Path::new("/project/.remargin.yaml"))
    );
}

#[test]
fn resolve_mode_defaults_to_open_when_no_config() {
    let system = MockSystem::new()
        .with_dir(Path::new("/empty/path"))
        .unwrap();

    let resolved = resolve_mode(&system, Path::new("/empty/path")).unwrap();
    assert_eq!(resolved.mode, Mode::Open);
    assert!(resolved.source.is_none());
}

#[test]
fn resolve_mode_ignores_type_filter() {
    // The whole point: even when the nearest config is `type: agent`, the
    // mode resolution does not skip it looking for a human config — it
    // returns the agent config's mode, because mode is a directory-tree
    // property.
    let system = MockSystem::new()
        .with_dir(Path::new("/home/vault/sub"))
        .unwrap()
        .with_file(
            Path::new("/home/vault/.remargin.yaml"),
            b"type: agent\nmode: strict\n",
        )
        .unwrap()
        .with_file(
            Path::new("/home/.remargin.yaml"),
            b"type: human\nidentity: eduardo\nmode: open\n",
        )
        .unwrap();

    let resolved = resolve_mode(&system, Path::new("/home/vault/sub")).unwrap();
    assert_eq!(resolved.mode, Mode::Strict);
    assert_eq!(
        resolved.source.as_deref(),
        Some(Path::new("/home/vault/.remargin.yaml"))
    );
}

#[test]
fn resolve_mode_uses_default_when_config_omits_mode() {
    // A config without a `mode:` field parses as Mode::Open (default).
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            minimal_config_yaml("bob").as_bytes(),
        )
        .unwrap();

    let resolved = resolve_mode(&system, Path::new("/project")).unwrap();
    assert_eq!(resolved.mode, Mode::Open);
    assert_eq!(
        resolved.source.as_deref(),
        Some(Path::new("/project/.remargin.yaml"))
    );
}

#[test]
fn mode_as_str_roundtrip() {
    assert_eq!(Mode::Open.as_str(), "open");
    assert_eq!(Mode::Registered.as_str(), "registered");
    assert_eq!(Mode::Strict.as_str(), "strict");
}

#[test]
fn agent_override_rejects_inherited_human_identity() {
    // Scenario: user has ~/.remargin.yaml with type: human and an identity.
    // Workspace has no config. An agent operation walks up and finds the
    // human config, then overrides author_type to "agent".
    //
    // Expected: error — the identity belongs to a human config; using it
    // with agent type is a type mismatch. The agent should have its own
    // identity configured, not silently borrow the human's.
    let system = MockSystem::new()
        .with_dir(Path::new("/home/user/project/src"))
        .unwrap()
        .with_file(
            Path::new("/home/user/.remargin.yaml"),
            typed_config_yaml("eduardo", "human").as_bytes(),
        )
        .unwrap();

    let config = load_config(&system, Path::new("/home/user/project/src"))
        .unwrap()
        .unwrap();
    assert_eq!(config.author_type.as_deref(), Some("human"));
    assert_eq!(config.identity.as_deref(), Some("eduardo"));

    let cli = CliOverrides {
        author_type: Some("agent"),
        ..CliOverrides::default()
    };

    let result = ResolvedConfig::resolve(&system, Some(config), None, &cli);
    assert!(
        result.is_err(),
        "expected error when agent type overrides human config identity, \
         but got identity={:?} author_type={:?}",
        result.as_ref().ok().and_then(|r| r.identity.clone()),
        result.as_ref().ok().map(|r| r.author_type.clone()),
    );
}

#[test]
fn registry_participant_display_name_mixed() {
    // Mixed registry: some participants set `display_name`, some don't.
    // Both shapes must parse, and the struct must carry `Some` / `None`
    // respectively. Downstream JSON output (in the CLI) substitutes
    // the participant id when `None`.
    let yaml = "\
participants:
  alice:
    display_name: \"Alice Doe\"
    type: human
    status: active
    pubkeys: []
  bob:
    type: agent
    status: active
    pubkeys: []
";
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            yaml.as_bytes(),
        )
        .unwrap();

    let registry = load_registry(&system, Path::new("/project"))
        .unwrap()
        .unwrap();

    let alice = &registry.participants["alice"];
    assert_eq!(alice.display_name.as_deref(), Some("Alice Doe"));
    assert_eq!(alice.status, RegistryParticipantStatus::Active);

    let bob = &registry.participants["bob"];
    assert!(
        bob.display_name.is_none(),
        "bob has no display_name, expected None; got {:?}",
        bob.display_name,
    );
}
