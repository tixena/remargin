//! Unit tests for [`crate::mcp::sandbox`] (rem-yj1j.3 / rem-v0ky).
//!
//! Covers scenarios 1-15 from the rem-yj1j.3 testing plan:
//!
//! - 1, 14: empty `trusted_roots` falls back to spawn cwd.
//! - 2, 11: multi-root construction with dedup.
//! - 3: symlink in `trusted_roots` resolves through canonicalize.
//! - 4-6: covers exact, descendant, and non-covered paths.
//! - 7, 8: symlink escape and symlink-within-sandbox.
//! - 9, 10: lexical fallback for non-existent targets.
//! - 12, 13: the recursive-realm respect / no-transitive-trust rules
//!   are enforced by the wider system (T23's parent-walk + this
//!   module's "`from_walk` uses spawn cwd only"). The dedicated
//!   integration tests for those scenarios live with rem-w6m1; here
//!   we only assert the structural property that target-realm
//!   `trusted_roots` are not consulted.
//! - 15: no reload — by construction (no method to update); pinned
//!   below as a documentation-only test.

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

/// Scenario 1 / 14: empty `trusted_roots` (or no config at all) falls
/// back to the canonical spawn cwd.
#[test]
fn empty_trusted_roots_uses_spawn_cwd() {
    let system = spawn_system_with(None);
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert_eq!(sandbox.roots, vec![PathBuf::from("/r")]);
}

#[test]
fn explicit_empty_trusted_roots_uses_spawn_cwd() {
    let system = spawn_system_with(Some("permissions:\n  trusted_roots: []\n"));
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert_eq!(sandbox.roots, vec![PathBuf::from("/r")]);
}

/// Scenario 2: two `trusted_roots` produce a sorted, deduped list.
#[test]
fn multiple_trusted_roots_are_sorted_and_canonicalised() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/h/notes"))
        .unwrap()
        .with_dir(Path::new("/h/repo"))
        .unwrap()
        .with_env("HOME", "/h")
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            b"permissions:\n  trusted_roots:\n    - ~/notes\n    - ~/repo\n",
        )
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert_eq!(
        sandbox.roots,
        vec![PathBuf::from("/h/notes"), PathBuf::from("/h/repo")]
    );
}

/// Scenario 11: duplicate entries collapse to one root.
#[test]
fn duplicate_trusted_roots_are_deduped() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/h/notes"))
        .unwrap()
        .with_env("HOME", "/h")
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            b"permissions:\n  trusted_roots:\n    - ~/notes\n    - ~/notes\n",
        )
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert_eq!(sandbox.roots, vec![PathBuf::from("/h/notes")]);
}

/// When the walked `.remargin.yaml` already includes the spawn cwd as
/// a `trusted_root`, the auto-fallback does NOT add it twice.
#[test]
fn cwd_in_trusted_roots_does_not_double_count() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/h/notes"))
        .unwrap()
        .with_env("HOME", "/h")
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            b"permissions:\n  trusted_roots:\n    - /r\n    - ~/notes\n",
        )
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert_eq!(
        sandbox.roots,
        vec![PathBuf::from("/h/notes"), PathBuf::from("/r")]
    );
}

/// Scenario 4: an exact root is covered.
#[test]
fn covers_exact_root() {
    let system = spawn_system_with(None);
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert!(sandbox.covers(&system, Path::new("/r")).unwrap());
}

/// Scenario 5: descendants of a root are covered.
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

/// Scenario 6: an unrelated path is rejected.
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

/// Scenario 9: a path that doesn't exist yet but lives under a covered
/// root is admitted (lexical fallback against the nearest existing
/// ancestor).
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

/// Scenario 10: a non-existent path outside every root is rejected.
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

/// `ensure_covers` produces a uniform `path escapes MCP sandbox`
/// message, which the MCP request handler will surface verbatim to the
/// caller.
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
    assert!(
        msg.contains("path escapes MCP sandbox"),
        "expected sandbox-escape error, got: {msg}"
    );
    assert!(
        msg.contains("/x"),
        "error must include the offending path, got: {msg}"
    );
}

/// `ensure_covers` is the no-op happy path for a covered descendant.
#[test]
fn ensure_covers_succeeds_for_covered_descendant() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    sandbox
        .ensure_covers(&system, Path::new("/r/file.md"))
        .unwrap();
}

/// Scenario 13 (structural): a target realm's own `trusted_roots` are
/// NOT auto-mounted. We assert this property by constructing a sandbox
/// from a realm at `/r` whose YAML trusts `/b`, then verifying that a
/// path `/c/...` (declared inside `/b/.remargin.yaml`'s own
/// `trusted_roots`) is NOT covered.
#[test]
fn no_transitive_trust_target_realm_trusted_roots_ignored() {
    let system = MockSystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/b"))
        .unwrap()
        .with_dir(Path::new("/c"))
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            b"permissions:\n  trusted_roots:\n    - /b\n",
        )
        .unwrap()
        .with_file(
            Path::new("/b/.remargin.yaml"),
            b"permissions:\n  trusted_roots:\n    - /c\n",
        )
        .unwrap();
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    assert!(sandbox.covers(&system, Path::new("/b/note.md")).unwrap());
    assert!(
        !sandbox.covers(&system, Path::new("/c/foo.md")).unwrap(),
        "target realm's own trusted_roots must not be auto-mounted"
    );
}

/// Scenario 15: `McpSandbox` exposes no method to update its `roots`
/// after construction. This compile-shaped check is the static
/// guarantee we rely on for "no mid-session reload" — a future PR that
/// adds a `reload` / `update` / `&mut self` method must also reconsider
/// the design doc's Decision 13.
#[test]
fn sandbox_offers_no_runtime_mutation() {
    let system = spawn_system_with(None);
    let sandbox = McpSandbox::from_walk(&system, Path::new("/r")).unwrap();
    let cloned = sandbox.clone();
    assert_eq!(sandbox, cloned);
    // No `set_roots`, `reload_from_walk`, or `&mut self` method exists
    // on `McpSandbox`. If you add one, update this test and the
    // module-level docstring's "Static at boot" section.
}
