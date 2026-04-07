//! Tests for the config and registry loader.

use std::path::Path;

use os_shim::mock::MockSystem;

use crate::parser::AuthorType;

use super::registry::RegistryParticipantStatus;
use super::{
    CliOverrides, Mode, ResolvedConfig, load_config, load_config_filtered, load_registry,
    resolve_key_path,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a minimal `.remargin.yaml` content string.
fn minimal_config_yaml(identity: &str) -> String {
    format!("identity: {identity}\n")
}

/// Create a full `.remargin.yaml` content string.
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

/// Create a `.remargin-registry.yaml` content string.
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

// ---------------------------------------------------------------------------
// Test 1: Walk-up finds config
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 2: Walk-up finds nothing
// ---------------------------------------------------------------------------

#[test]
fn walk_up_finds_nothing() {
    let system = MockSystem::new()
        .with_dir(Path::new("/empty/path"))
        .unwrap();

    let config = load_config(&system, Path::new("/empty/path")).unwrap();
    assert!(config.is_none());
}

// ---------------------------------------------------------------------------
// Test 3: Config and registry at different levels
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 4: Full config parse
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 5: Minimal config (defaults)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 6: Registry with revoked participant
// ---------------------------------------------------------------------------

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

    // Active participant can post.
    resolved.can_post("eduardo").unwrap();

    // Revoked participant cannot post.
    let err = resolved.can_post("revoked_user").unwrap_err();
    assert!(
        format!("{err}").contains("revoked"),
        "expected revoked error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Key rotation (multiple pubkeys)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 8: Key shorthand (plain name)
// ---------------------------------------------------------------------------

#[test]
fn key_shorthand_plain_name() {
    let system = MockSystem::new().with_env("HOME", "/home/user").unwrap();

    let path = resolve_key_path(&system, "id_ed25519").unwrap();
    assert_eq!(path, Path::new("/home/user/.ssh/id_ed25519"));
}

// ---------------------------------------------------------------------------
// Test 9: Key path (literal with tilde)
// ---------------------------------------------------------------------------

#[test]
fn key_path_literal_tilde() {
    let system = MockSystem::new().with_env("HOME", "/home/user").unwrap();

    let path = resolve_key_path(&system, "~/.remargin/keys/foo.key").unwrap();
    assert_eq!(path, Path::new("/home/user/.remargin/keys/foo.key"));
}

// ---------------------------------------------------------------------------
// Test 9b: Key path (literal absolute)
// ---------------------------------------------------------------------------

#[test]
fn key_path_literal_absolute() {
    let system = MockSystem::new();

    let path = resolve_key_path(&system, "/etc/keys/foo.key").unwrap();
    assert_eq!(path, Path::new("/etc/keys/foo.key"));
}

// ---------------------------------------------------------------------------
// Test 10: CLI override
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 11: Open mode (any author)
// ---------------------------------------------------------------------------

#[test]
fn open_mode_any_author() {
    let system = MockSystem::new();
    let resolved = ResolvedConfig::resolve(&system, None, None, &CliOverrides::default()).unwrap();

    // Open mode allows anyone.
    resolved.can_post("unknown_user").unwrap();
}

// ---------------------------------------------------------------------------
// Test 12: Strict mode, unregistered
// ---------------------------------------------------------------------------

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

    // Unregistered author is rejected.
    let err = resolved.can_post("stranger").unwrap_err();
    assert!(
        format!("{err}").contains("not registered"),
        "expected 'not registered' error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Test 13: Strict mode, registered (requires signature)
// ---------------------------------------------------------------------------

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

    // Active participant can post.
    resolved.can_post("eduardo").unwrap();

    // Strict mode requires signature for active participants.
    assert!(resolved.requires_signature("eduardo"));

    // Non-strict mode would not require signature.
    assert!(!resolved.requires_signature("stranger"));
}

// ---------------------------------------------------------------------------
// Test: Missing registry in registered mode
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test: Registry participant status default (active)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Helpers for type-filtered tests
// ---------------------------------------------------------------------------

/// Create a `.remargin.yaml` with identity and type.
fn typed_config_yaml(identity: &str, author_type: &str) -> String {
    format!("identity: {identity}\ntype: {author_type}\n")
}

// ---------------------------------------------------------------------------
// Test: Type filter matches first config
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test: Type filter skips non-matching, finds match higher up
// ---------------------------------------------------------------------------

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

    let config =
        load_config_filtered(&system, Path::new("/home/project/src"), Some("human"))
            .unwrap()
            .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));
    assert_eq!(config.author_type.as_deref(), Some("human"));
}

// ---------------------------------------------------------------------------
// Test: Type filter finds no match
// ---------------------------------------------------------------------------

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

    let config =
        load_config_filtered(&system, Path::new("/project/src"), Some("human")).unwrap();
    assert!(config.is_none());
}

// ---------------------------------------------------------------------------
// Test: No filter (backward compat)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test: No filter, no type field in config (backward compat)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test: Type filter with config missing type field (skips it)
// ---------------------------------------------------------------------------

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

    let config =
        load_config_filtered(&system, Path::new("/project/sub"), Some("human"))
            .unwrap()
            .unwrap();
    assert_eq!(config.identity.as_deref(), Some("eduardo"));
    assert_eq!(config.author_type.as_deref(), Some("human"));
}

// ---------------------------------------------------------------------------
// Test: Multiple configs, filter selects correct one
// ---------------------------------------------------------------------------

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

    // Filter for agent from /a/b: should skip /a/b (human) and find /a (agent).
    let config = load_config_filtered(&system, Path::new("/a/b"), Some("agent"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("agent_bot"));
    assert_eq!(config.author_type.as_deref(), Some("agent"));
}

// ---------------------------------------------------------------------------
// Test: load_config still works (wrapper test)
// ---------------------------------------------------------------------------

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

    // load_config (no filter) should return the first config found.
    let config = load_config(&system, Path::new("/project/src"))
        .unwrap()
        .unwrap();
    assert_eq!(config.identity.as_deref(), Some("agent_bot"));
}
