//! Tests for the config and registry loader.

use std::path::Path;

use os_shim::mock::MockSystem;

use crate::parser::AuthorType;

use super::identity::IdentityFlags;
use super::registry::RegistryParticipantStatus;
use super::{
    Mode, ResolvedConfig, load_config, load_config_filtered, load_registry, resolve_key_path,
    resolve_mode,
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

    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

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
fn manual_identity_declaration_supersedes_walked_config() {
    // Branch 2 of the resolver: --identity + --type + --key is a complete
    // manual declaration. It does not read any .remargin.yaml to fill in
    // identity fields — the walked config's `identity: config_user` is
    // irrelevant. Mode still comes from the walked config (mode is a
    // property of the directory tree, not of the identity declaration)
    // and assets_dir still honors the CLI flag when set.
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: config_user\ntype: human\nmode: open\nkey: id_ed25519\n",
        )
        .unwrap();

    let flags = IdentityFlags {
        author_type: Some(AuthorType::Agent),
        identity: Some(String::from("cli_user")),
        key: Some(String::from("~/.remargin/mykey")),
        ..IdentityFlags::default()
    };

    let resolved =
        ResolvedConfig::resolve(&system, Path::new("/project"), &flags, Some("my_assets")).unwrap();

    assert_eq!(resolved.identity.as_deref(), Some("cli_user"));
    assert_eq!(resolved.author_type, Some(AuthorType::Agent));
    assert_eq!(resolved.mode, Mode::Open);
    assert_eq!(
        resolved.key_path,
        Some(Path::new("/home/user/.remargin/mykey").to_path_buf())
    );
    assert_eq!(resolved.assets_dir, "my_assets");
}

#[test]
fn open_mode_any_author() {
    let system = MockSystem::new();
    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/empty"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

    resolved.can_post("unknown_user").unwrap();
}

#[test]
fn strict_mode_unregistered() {
    let system = MockSystem::new()
        .with_file(Path::new("/project/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

    let err = resolved.can_post("stranger").unwrap_err();
    assert!(
        format!("{err}").contains("not registered"),
        "expected 'not registered' error, got: {err}"
    );
}

#[test]
fn strict_mode_requires_signature() {
    let system = MockSystem::new()
        .with_file(Path::new("/project/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

    resolved.can_post("eduardo").unwrap();

    assert!(resolved.requires_signature("eduardo"));

    assert!(!resolved.requires_signature("stranger"));
}

#[test]
fn registered_mode_no_registry() {
    let system = MockSystem::new()
        .with_file(Path::new("/project/.remargin.yaml"), b"mode: registered\n")
        .unwrap();
    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

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
fn agent_type_filter_does_not_inherit_human_identity() {
    // Scenario: user has ~/.remargin.yaml with type: human and an identity.
    // Workspace has no config. An agent operation passes --type agent;
    // the resolver walks up with `type == agent` as a strict-equality
    // filter and finds only the human file, which fails the filter.
    //
    // Expected: the walk exhausts with no match. The resolver does NOT
    // silently borrow the human identity with a swapped author_type —
    // the three-branch design (rem-11u) makes this a hard error instead
    // of a silent misattribution.
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

    let flags = IdentityFlags {
        author_type: Some(AuthorType::Agent),
        ..IdentityFlags::default()
    };

    let result =
        ResolvedConfig::resolve(&system, Path::new("/home/user/project/src"), &flags, None);
    assert!(
        result.is_err(),
        "expected walk-exhaust error when --type agent cannot match the human config, \
         but got identity={:?} author_type={:?}",
        result.as_ref().ok().and_then(|r| r.identity.clone()),
        result.as_ref().ok().map(|r| r.author_type.clone()),
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("no identity resolved") || msg.contains("no .remargin.yaml matched"),
        "expected walk-exhaust error message, got: {msg}"
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

// ---------- resolve_signing_key: rem-dyz fail-fast contract ----------

#[test]
fn resolve_signing_key_returns_none_in_open_mode() {
    // Open mode: even registered authors with a key resolved on the
    // config do not "require" a signature from the op's perspective. The
    // helper must return Ok(None) so create_comment skips signing.
    //
    // Mode is sourced from the config file (rem-wws). No config → default
    // mode is Open.
    let system = MockSystem::new();
    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/empty"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

    assert!(resolved.resolve_signing_key("eduardo").is_none());
}

#[test]
fn resolve_signing_key_returns_none_for_unregistered_in_strict() {
    // Strict + unregistered author: `requires_signature` is false
    // (author not registered active), so the helper short-circuits with
    // `None`. The resolver itself would reject an unregistered identity
    // (rem-xc8x), so in practice this code path is only reached for
    // arbitrary author names the op layer looks up (e.g. verifying
    // siblings authored by someone else).
    let system = MockSystem::new()
        .with_file(Path::new("/project/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

    assert!(resolved.resolve_signing_key("stranger").is_none());
}

#[test]
fn resolve_signing_key_returns_key_when_present() {
    // Strict + registered active + key_path set: the helper hands back a
    // reference to the resolved key path so the caller signs with it.
    //
    // The `.remargin.yaml` declares the complete identity (eduardo /
    // human / id_ed25519) directly — under the three-branch resolver
    // key is paired with identity inside the same file (branch 3 walk)
    // or inside a manual --identity/--type/--key declaration (branch 2).
    // Supplying `--key` alone is not a valid shape.
    let system = MockSystem::new()
        .with_env("HOME", "/home/eduardo")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: eduardo\ntype: human\nmode: strict\nkey: id_ed25519\n",
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let resolved = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap();

    let key = resolved.resolve_signing_key("eduardo").unwrap();
    assert!(
        key.ends_with("id_ed25519"),
        "key must resolve through ~/.ssh; got {}",
        key.display(),
    );
}

#[test]
fn resolve_bails_when_strict_identity_has_no_key() {
    // Strict + registered active identity + NO key_path: the resolver
    // itself now fails fast (rem-xc8x). Previously `create_comment`
    // silently wrote an unsigned artifact here and the post-write gate
    // tripped on the NEXT mutation (rem-dyz). After rem-xc8x the gate
    // moves to construction time so ops never see an invalid config.
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: eduardo\nmode: strict\n",
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let err = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("eduardo"),
        "error must name the identity, got: {msg}"
    );
    assert!(
        msg.contains("no signing key"),
        "error must say a signing key is missing, got: {msg}"
    );
    assert!(
        msg.contains("--key"),
        "error must point at --key flag, got: {msg}"
    );
    assert!(
        msg.contains(".remargin.yaml"),
        "error must point at config file key field, got: {msg}"
    );
}

#[test]
fn resolve_bails_when_revoked_identity_in_strict_mode() {
    // rem-xc8x acceptance: a revoked participant in strict mode causes
    // `ResolvedConfig::resolve` to error, not the op handler. This
    // replaces the equivalent op-level `can_post` check.
    let system = MockSystem::new()
        .with_env("HOME", "/home/eduardo")
        .unwrap()
        .with_file(
            Path::new("/project/.remargin.yaml"),
            b"identity: revoked_user\nmode: strict\nkey: id_ed25519\n",
        )
        .unwrap()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            registry_yaml().as_bytes(),
        )
        .unwrap();

    let err = ResolvedConfig::resolve(
        &system,
        Path::new("/project"),
        &IdentityFlags::default(),
        None,
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("revoked_user"),
        "error must name the identity, got: {msg}"
    );
    assert!(
        msg.contains("revoked"),
        "error must surface the revocation, got: {msg}"
    );
}
