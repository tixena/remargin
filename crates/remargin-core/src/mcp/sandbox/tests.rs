//! Unit tests for [`crate::mcp::sandbox`].
//!
//! Post-eradication the sandbox is always anchored at the spawn cwd —
//! the only way to widen its reach is to spawn in a different
//! directory. The trusted_roots-extension scenarios that lived here
//! are gone.

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::mcp::sandbox::McpSandbox;

fn spawn_system_with(yaml: Option<&str>) -> MockSystem {
    let mut system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_env("HOME", "/h")
        .unwrap();
    if let Some(body) = yaml {
        system = system
            .with_file(Path::new("/r/.remargin.yaml"), body.as_bytes())
            .unwrap();
    }
    system
}

#[test]
fn from_walk_uses_spawn_cwd_with_no_config() {
    let system = spawn_system_with(None);
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert_eq!(sandbox.roots, vec![PathBuf::from("/r")]);
}

#[test]
fn from_walk_uses_spawn_cwd_even_with_restrict_declared() {
    // The MCP sandbox is the spawn-cwd boundary; per-op allow-list
    // (`restrict`) is enforced by op_guard, not by widening / narrowing
    // the sandbox.
    let system = spawn_system_with(Some("permissions:\n  restrict:\n    - path: '*'\n"));
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert_eq!(sandbox.roots, vec![PathBuf::from("/r")]);
}

#[test]
fn covers_exact_root() {
    let system = spawn_system_with(None);
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert!(sandbox.covers(&system, Path::new("/r")).unwrap());
}

#[test]
fn covers_descendant() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/sub/deep"))
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert!(sandbox.covers(&system, Path::new("/r/sub/deep")).unwrap());
}

#[test]
fn does_not_cover_unrelated_path() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/x/y"))
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert!(!sandbox.covers(&system, Path::new("/x/y")).unwrap());
}

#[test]
fn covers_nonexistent_descendant_under_root() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert!(
        sandbox
            .covers(&system, Path::new("/r/new/file.md"))
            .unwrap()
    );
}

#[test]
fn does_not_cover_nonexistent_path_outside_root() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/x"))
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert!(!sandbox.covers(&system, Path::new("/x/new.md")).unwrap());
}

#[test]
fn ensure_covers_bails_with_named_error_when_outside() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/x"))
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    let err = sandbox.ensure_covers(&system, Path::new("/x")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("path escapes MCP sandbox"));
    assert!(msg.contains("/x"));
}

#[test]
fn ensure_covers_succeeds_for_covered_descendant() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    sandbox
        .ensure_covers(&system, Path::new("/r/file.md"))
        .unwrap();
}

#[test]
fn sandbox_offers_no_runtime_mutation() {
    let system = spawn_system_with(None);
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    let cloned = sandbox.clone();
    assert_eq!(sandbox, cloned);
}
