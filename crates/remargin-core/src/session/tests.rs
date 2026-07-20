//! Tests for downward session discovery over temp trees.

use std::path::Path;

use os_shim::mock::MockSystem;

use super::discovery::discover_sessions;

/// Build a `demo-remargin`-shaped tree: a root that declares its own
/// identity and a system prompt, five child realms each declaring their
/// own identity, and a `session:` block on `finance`.
fn demo_tree() -> MockSystem {
    MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: eburgos_notes_agent\nsystem_prompt:\n  name: root\n  prompt: root body\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/audience/.remargin.yaml"),
            b"identity: audience\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/content/.remargin.yaml"),
            b"identity: content\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/coordinator/.remargin.yaml"),
            b"identity: coordinator\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/finance/.remargin.yaml"),
            b"identity: finance\nsession:\n  loop: 30s\n  goal: process pending work\n",
        )
        .unwrap()
        .with_file(Path::new("/demo/ops/.remargin.yaml"), b"identity: ops\n")
        .unwrap()
}

#[test]
fn six_identities_over_demo_shaped_tree() {
    let system = demo_tree();

    let sessions = discover_sessions(&system, Path::new("/demo")).unwrap();

    let ids: Vec<&str> = sessions.iter().map(|s| s.identity.as_str()).collect();
    assert_eq!(
        ids,
        [
            "eburgos_notes_agent",
            "audience",
            "content",
            "coordinator",
            "finance",
            "ops",
        ]
    );

    assert_eq!(sessions[0].folder.as_path(), Path::new("/demo"));
    assert_eq!(sessions[4].folder.as_path(), Path::new("/demo/finance"));
    assert!(sessions[4].session.is_some());
    // Every other realm has no session: block of its own.
    assert!(sessions[1].session.is_none());
}

#[test]
fn each_session_carries_its_resolved_system_prompt() {
    let system = demo_tree();

    let sessions = discover_sessions(&system, Path::new("/demo")).unwrap();

    // The root declares the prompt; children inherit it via the walk-up.
    assert_eq!(sessions[0].system_prompt.name, "root");
    assert_eq!(sessions[0].system_prompt.prompt, "root body");
    assert_eq!(sessions[3].system_prompt.prompt, "root body");
    assert!(!sessions[3].system_prompt.is_default);
}

#[test]
fn inherit_only_subfolder_is_not_emitted() {
    let system = MockSystem::new()
        .with_file(Path::new("/demo/.remargin.yaml"), b"identity: root_agent\n")
        .unwrap()
        .with_file(
            Path::new("/demo/sub/.remargin.yaml"),
            b"system_prompt:\n  name: sub\n  prompt: sub body\n",
        )
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/demo")).unwrap();

    let ids: Vec<&str> = sessions.iter().map(|s| s.identity.as_str()).collect();
    assert_eq!(ids, ["root_agent"]);
}

#[test]
fn nested_realm_boundary_yields_two_scoped_sessions() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/a/.remargin.yaml"), b"identity: a_id\n")
        .unwrap()
        .with_file(Path::new("/tree/a/b/.remargin.yaml"), b"identity: b_id\n")
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].identity, "a_id");
    assert_eq!(sessions[0].folder.as_path(), Path::new("/tree/a"));
    assert_eq!(sessions[0].scope_root.as_path(), Path::new("/tree/a"));
    assert_eq!(sessions[1].identity, "b_id");
    assert_eq!(sessions[1].folder.as_path(), Path::new("/tree/a/b"));
    assert_eq!(sessions[1].scope_root.as_path(), Path::new("/tree/a/b"));
}

#[test]
fn same_identity_in_sibling_folders_stays_distinct() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/bar/.remargin.yaml"), b"identity: x\n")
        .unwrap()
        .with_file(Path::new("/tree/foo/.remargin.yaml"), b"identity: x\n")
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().all(|s| s.identity == "x"));
    assert_eq!(sessions[0].folder.as_path(), Path::new("/tree/bar"));
    assert_eq!(sessions[1].folder.as_path(), Path::new("/tree/foo"));
}

#[test]
fn no_identity_anywhere_yields_zero_sessions() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/.remargin.yaml"), b"mode: open\n")
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    assert!(sessions.is_empty());
}

#[test]
fn root_identity_inherited_from_ancestor_uses_cwd_as_folder() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/vault/.remargin.yaml"),
            b"identity: vault_agent\nsession:\n  loop: 5min\n  goal: x\n",
        )
        .unwrap()
        .with_file(
            Path::new("/vault/proj/.remargin.yaml"),
            b"system_prompt:\n  name: proj\n  prompt: proj body\n",
        )
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/vault/proj")).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].identity, "vault_agent");
    assert_eq!(sessions[0].folder.as_path(), Path::new("/vault/proj"));
    assert_eq!(sessions[0].scope_root.as_path(), Path::new("/vault/proj"));
    // The session: block travels with the identity's declaring config.
    assert!(sessions[0].session.is_some());
}

#[test]
fn dot_directories_are_skipped() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/.remargin.yaml"), b"identity: root\n")
        .unwrap()
        .with_file(
            Path::new("/tree/.git/.remargin.yaml"),
            b"identity: should_not_appear\n",
        )
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    let ids: Vec<&str> = sessions.iter().map(|s| s.identity.as_str()).collect();
    assert_eq!(ids, ["root"]);
}
