//! Unit tests for `permissions::inspect`.

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::permissions::inspect::{check, show};

fn mock_with(files: &[(&str, &str)]) -> MockSystem {
    let mut system = MockSystem::new();
    for (path, body) in files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    system
}

// ---------------------------------------------------------------------
// show()
// ---------------------------------------------------------------------

#[test]
fn show_empty_when_no_config() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();
    let out = show(&system, Path::new("/r")).unwrap();
    assert!(out.allow_dot_folders.is_empty());
    assert!(out.deny_ops.is_empty());
    assert!(out.trusted_roots.is_empty());
}

#[test]
fn show_single_realm_full_block() {
    let yaml = "\
permissions:
  trusted_roots:
    - path: src
  deny_ops:
    - path: src/secret
      ops: [purge]
  allow_dot_folders: ['.github']
";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();

    assert_eq!(out.trusted_roots.len(), 1);
    assert_eq!(out.trusted_roots[0].path_text, "/r/src");
    assert_eq!(
        out.trusted_roots[0].source_file,
        PathBuf::from("/r/.remargin.yaml")
    );

    assert_eq!(out.deny_ops.len(), 1);
    assert_eq!(out.deny_ops[0].ops, vec![String::from("purge")]);

    assert_eq!(out.allow_dot_folders.len(), 1);
    assert_eq!(
        out.allow_dot_folders[0].names,
        vec![String::from(".github")]
    );
    assert_eq!(
        out.allow_dot_folders[0].source_file,
        PathBuf::from("/r/.remargin.yaml"),
    );
}

#[test]
fn show_multi_realm_walk_surfaces_both() {
    let parent = "permissions:\n  trusted_roots:\n    - path: top\n";
    let child = "permissions:\n  trusted_roots:\n    - path: nested\n";
    let system = mock_with(&[
        ("/r/.remargin.yaml", parent),
        ("/r/sub/.remargin.yaml", child),
    ]);
    let out = show(&system, Path::new("/r/sub")).unwrap();
    assert_eq!(out.trusted_roots.len(), 2);
    assert_eq!(
        out.trusted_roots[0].source_file,
        PathBuf::from("/r/sub/.remargin.yaml")
    );
    assert_eq!(
        out.trusted_roots[1].source_file,
        PathBuf::from("/r/.remargin.yaml")
    );
}

#[test]
fn show_round_trips_through_json() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();
    let json = serde_json::to_string(&out).unwrap();
    assert!(json.contains("\"path_text\":\"/r/src\""));
    assert!(json.contains("\"source_file\":\"/r/.remargin.yaml\""));
}

#[test]
fn show_wildcard_entry_carries_realm_root() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: '*'\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();
    assert_eq!(out.trusted_roots[0].path_text, "*");
    assert_eq!(out.trusted_roots[0].realm_root, Some(PathBuf::from("/r")));
    assert!(out.trusted_roots[0].absolute_path.is_none());
}

#[test]
fn show_allow_dot_folders_provenance_across_stacked_yamls() {
    let parent = "permissions:\n  allow_dot_folders: ['.git']\n";
    let child = "permissions:\n  allow_dot_folders: ['.cache']\n";
    let system = mock_with(&[
        ("/r/.remargin.yaml", parent),
        ("/r/sub/.remargin.yaml", child),
    ]);
    let out = show(&system, Path::new("/r/sub")).unwrap();

    assert_eq!(out.allow_dot_folders.len(), 2);
    assert_eq!(out.allow_dot_folders[0].names, vec![String::from(".cache")],);
    assert_eq!(
        out.allow_dot_folders[0].source_file,
        PathBuf::from("/r/sub/.remargin.yaml"),
    );
    assert_eq!(out.allow_dot_folders[1].names, vec![String::from(".git")]);
    assert_eq!(
        out.allow_dot_folders[1].source_file,
        PathBuf::from("/r/.remargin.yaml"),
    );
}

#[test]
fn show_allow_dot_folders_source_file_survives_json_roundtrip() {
    let yaml = "permissions:\n  allow_dot_folders: ['.assets']\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();
    let json = serde_json::to_string(&out).unwrap();
    assert!(json.contains("\"source_file\":\"/r/.remargin.yaml\""));
    assert!(!json.contains("\"source_file\":\"\""));
}

// ---------------------------------------------------------------------
// check()
// ---------------------------------------------------------------------

#[test]
fn check_no_restrict_anywhere_is_unrestricted() {
    let system = mock_with(&[("/r/.remargin.yaml", "identity: alice\n")]);
    let result = check(&system, Path::new("/r"), Path::new("/r/foo.md"), false).unwrap();
    assert!(!result.restricted);
    assert!(result.matching_rule.is_none());
}

/// Inside the allow-list — not restricted.
#[test]
fn check_path_inside_allow_list_is_not_restricted() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/src/secret/foo.md"),
        false,
    )
    .unwrap();
    assert!(!result.restricted);
}

/// Outside the allow-list — restricted.
#[test]
fn check_path_outside_allow_list_is_restricted() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/src/public/foo.md"),
        false,
    )
    .unwrap();
    assert!(result.restricted);
}

#[test]
fn check_wildcard_restrict_allows_anywhere_in_realm() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: '*'\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(&system, Path::new("/r"), Path::new("/r/anywhere.md"), false).unwrap();
    assert!(!result.restricted);
}

#[test]
fn check_deny_ops_entry_counts_as_restricted() {
    let yaml = "permissions:\n  deny_ops:\n    - path: foo\n      ops: [purge]\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(&system, Path::new("/r"), Path::new("/r/foo/x.md"), false).unwrap();
    assert!(result.restricted);
}

#[test]
fn check_why_populates_matching_rule_for_outside_allow_list() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/src/public/foo.md"),
        true,
    )
    .unwrap();
    let rule = result.matching_rule.unwrap();
    assert_eq!(rule.kind, "trusted_roots");
    assert_eq!(rule.source_file, PathBuf::from("/r/.remargin.yaml"));
    assert!(rule.rule_text.contains("outside trusted_roots"));
}

#[test]
fn check_non_existent_path_matched_lexically() {
    // Outside the allow-list → restricted.
    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/elsewhere/missing/file.md"),
        false,
    )
    .unwrap();
    assert!(result.restricted);
}

#[test]
fn check_canonicalised_match_inside_allow_list() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: real\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(&system, Path::new("/r"), Path::new("/r/real/x.md"), false).unwrap();
    assert!(!result.restricted);
}

#[test]
fn check_round_trips_through_json() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    // `/r/foo.md` is OUTSIDE the allow-list `/r/src`.
    let result = check(&system, Path::new("/r"), Path::new("/r/foo.md"), true).unwrap();
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"restricted\":true"));
    assert!(json.contains("\"kind\":\"trusted_roots\""));
}

// ---------------------------------------------------------------------
// inspect::check and op_guard agree (the bug fix lands here)
// ---------------------------------------------------------------------

/// `permissions check` and `op_guard::check_against_resolved` MUST give
/// the same answer for any path. They share the predicate
/// `target_is_sanctioned`, so this test pins that the two layers
/// cannot drift again.
#[test]
fn inspect_check_and_op_guard_agree_on_allow_list_membership() {
    use crate::config::permissions::resolve::resolve_permissions;
    use crate::permissions::op_guard::check_against_resolved;

    // Vault realm: `restrict '*'` → the whole realm is allow-listed.
    let inner = "permissions:\n  trusted_roots:\n    - path: '*'\n";
    let system = mock_with(&[("/home/user/vault/.remargin.yaml", inner)]);

    let inside = Path::new("/home/user/vault/foo.md");
    let resolved = resolve_permissions(&system, Path::new("/home/user/vault")).unwrap();

    // op_guard allows this write.
    check_against_resolved(&system, "write", inside, &resolved).unwrap();

    // permissions check must agree: not restricted.
    let out = check(&system, Path::new("/home/user/vault"), inside, true).unwrap();
    assert!(
        !out.restricted,
        "inspect::check disagreed with op_guard for inside-allow-list path; got restricted=true with matching_rule={:?}",
        out.matching_rule,
    );

    // Now a path OUTSIDE the allow-list (it shouldn't ever resolve here
    // from the vault cwd, but exercising the logic): walk from cwd of a
    // file outside any realm. Since the cwd has no `.remargin.yaml`, the
    // walk returns empty and the path is unrestricted (open mode).
    let outside_open = Path::new("/elsewhere/foo.md");
    let resolved_open = resolve_permissions(&system, Path::new("/elsewhere")).unwrap();
    check_against_resolved(&system, "write", outside_open, &resolved_open).unwrap();
    let out_open = check(&system, Path::new("/elsewhere"), outside_open, false).unwrap();
    assert!(!out_open.restricted);
}
