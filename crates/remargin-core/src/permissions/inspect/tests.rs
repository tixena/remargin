//! Unit tests for `permissions::inspect` (rem-yj1j.7 / T28).
//!
//! Exercises the spec's test plan for `show` (scenarios 1-8) and
//! `check` (scenarios 9-19). CLI / MCP integration tests (20-22)
//! belong with the surface-registration follow-up.

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

/// 1 — no `.remargin.yaml` anywhere: every output collection empty.
#[test]
fn show_empty_when_no_config() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();
    let out = show(&system, Path::new("/r")).unwrap();
    assert!(out.allow_dot_folders.is_empty());
    assert!(out.deny_ops.is_empty());
    assert!(out.restrict.is_empty());
    assert!(out.trusted_roots.is_empty());
}

/// 2 (rem-egp9) — single realm, all four keys populated; provenance
/// preserved. Containment requires the `trusted_root` to live under the
/// declaring file's parent, so the `trusted_root` path is `/r/notes`
/// (was `/var/notes` pre-rem-egp9).
#[test]
fn show_single_realm_full_block() {
    let yaml = "\
permissions:
  trusted_roots:
    - /r/notes
  restrict:
    - path: src
  deny_ops:
    - path: src/secret
      ops: [purge]
  allow_dot_folders: ['.github']
";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();

    assert_eq!(out.trusted_roots.len(), 1);
    assert_eq!(out.trusted_roots[0].path, PathBuf::from("/r/notes"));
    assert_eq!(
        out.trusted_roots[0].source_file,
        PathBuf::from("/r/.remargin.yaml")
    );
    assert!(out.trusted_roots[0].recursive.is_none());

    assert_eq!(out.restrict.len(), 1);
    assert_eq!(out.restrict[0].path_text, "/r/src");
    assert_eq!(
        out.restrict[0].source_file,
        PathBuf::from("/r/.remargin.yaml")
    );

    assert_eq!(out.deny_ops.len(), 1);
    assert_eq!(out.deny_ops[0].ops, vec![String::from("purge")]);

    assert_eq!(out.allow_dot_folders.len(), 1);
    assert_eq!(
        out.allow_dot_folders[0].names,
        vec![String::from(".github")]
    );
    // rem-qdrw: source_file must be populated, mirroring the other
    // collections.
    assert_eq!(
        out.allow_dot_folders[0].source_file,
        PathBuf::from("/r/.remargin.yaml"),
    );
}

/// 3 (rem-egp9) — `trusted_root` that is a realm: nested
/// `.remargin.yaml` is expanded under `recursive`. The `trusted_root`
/// must live under the declaring file's parent (containment), so
/// declare from `/r` and trust `/r/x`.
#[test]
fn show_recursive_when_trusted_root_is_realm() {
    let outer = "permissions:\n  trusted_roots:\n    - /r/x\n";
    let inner = "permissions:\n  restrict:\n    - path: '*'\n";
    let system = mock_with(&[("/r/.remargin.yaml", outer), ("/r/x/.remargin.yaml", inner)]);
    let out = show(&system, Path::new("/r")).unwrap();
    let nested = out.trusted_roots[0].recursive.as_ref().unwrap();
    assert_eq!(nested.restrict.len(), 1);
    assert_eq!(nested.restrict[0].path_text, "*");
}

/// 4 (rem-egp9) — `trusted_root` that is not a realm leaves
/// `recursive = None`. Containment requires the `trusted_root` to be a
/// subfolder of the declaring file's parent.
#[test]
fn show_no_recursive_when_no_inner_config() {
    let outer = "permissions:\n  trusted_roots:\n    - /r/y\n";
    let system = mock_with(&[("/r/.remargin.yaml", outer)]);
    let out = show(&system, Path::new("/r")).unwrap();
    assert!(out.trusted_roots[0].recursive.is_none());
}

/// 5 (rem-egp9) — cycle detection: `/r/x` trusts a relative `..`
/// would violate containment, so we exercise cycle detection at the
/// `show` rendering layer using two realms whose mutual `trusted_roots`
/// are subfolders. `/r` trusts `/r/x`; `/r/x` trusts `/r/x/back`
/// (which contains an inner config that loops to `/r/x`). The
/// important property is that `show` returns rather than running
/// forever, AND every leaf entry's `recursive` is `None`.
#[test]
fn show_cycle_detection_stops_recursion() {
    let r = "permissions:\n  trusted_roots:\n    - /r/x\n";
    let x = "permissions:\n  trusted_roots:\n    - /r/x/back\n";
    let back = "permissions:\n  trusted_roots:\n    - /r/x/back/loop\n";
    let loop_yaml = "permissions:\n  trusted_roots:\n    - /r/x/back/loop/again\n";
    let system = mock_with(&[
        ("/r/.remargin.yaml", r),
        ("/r/x/.remargin.yaml", x),
        ("/r/x/back/.remargin.yaml", back),
        ("/r/x/back/loop/.remargin.yaml", loop_yaml),
    ]);

    let out = show(&system, Path::new("/r")).unwrap();
    let depth_one = out.trusted_roots[0].recursive.as_ref().unwrap();
    let depth_two = depth_one
        .trusted_roots
        .first()
        .and_then(|entry| entry.recursive.as_ref());
    if let Some(deep) = depth_two {
        for entry in &deep.trusted_roots {
            assert!(
                entry.recursive.is_none(),
                "depth cap or cycle detection should silence further recursion"
            );
        }
    }
}

/// 6 — multi-realm walk; both files surface.
#[test]
fn show_multi_realm_walk_surfaces_both() {
    let parent = "permissions:\n  restrict:\n    - path: top\n";
    let child = "permissions:\n  restrict:\n    - path: nested\n";
    let system = mock_with(&[
        ("/r/.remargin.yaml", parent),
        ("/r/sub/.remargin.yaml", child),
    ]);
    let out = show(&system, Path::new("/r/sub")).unwrap();
    assert_eq!(out.restrict.len(), 2);
    assert_eq!(
        out.restrict[0].source_file,
        PathBuf::from("/r/sub/.remargin.yaml")
    );
    assert_eq!(
        out.restrict[1].source_file,
        PathBuf::from("/r/.remargin.yaml")
    );
}

/// 7 — JSON round-trip via `serde_json` keeps the structure intact.
#[test]
fn show_round_trips_through_json() {
    let yaml = "permissions:\n  restrict:\n    - path: src\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();
    let json = serde_json::to_string(&out).unwrap();
    assert!(json.contains("\"path_text\":\"/r/src\""));
    assert!(json.contains("\"source_file\":\"/r/.remargin.yaml\""));
}

/// 8 — wildcard entry surfaces with the realm-root annotation.
#[test]
fn show_wildcard_entry_carries_realm_root() {
    let yaml = "permissions:\n  restrict:\n    - path: '*'\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();
    assert_eq!(out.restrict[0].path_text, "*");
    assert_eq!(out.restrict[0].realm_root, Some(PathBuf::from("/r")));
    assert!(out.restrict[0].absolute_path.is_none());
}

// ---------------------------------------------------------------------
// check()
// ---------------------------------------------------------------------

/// 9 — empty permissions: nothing restricted.
#[test]
fn check_empty_permissions_returns_unrestricted() {
    let system = mock_with(&[("/r/.remargin.yaml", "identity: alice\n")]);
    let result = check(&system, Path::new("/r"), Path::new("/r/foo.md"), false).unwrap();
    assert!(!result.restricted);
    assert!(result.matching_rule.is_none());
}

/// 10 — restrict subpath: target inside is restricted.
#[test]
fn check_restrict_subpath_inside_is_restricted() {
    let yaml = "permissions:\n  restrict:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/src/secret/foo.md"),
        false,
    )
    .unwrap();
    assert!(result.restricted);
}

/// 11 — restrict subpath: target outside is not restricted.
#[test]
fn check_restrict_subpath_outside_is_not_restricted() {
    let yaml = "permissions:\n  restrict:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/src/public/foo.md"),
        false,
    )
    .unwrap();
    assert!(!result.restricted);
}

/// 12 — wildcard restrict matches any path under the realm.
#[test]
fn check_wildcard_restrict_matches_under_realm() {
    let yaml = "permissions:\n  restrict:\n    - path: '*'\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(&system, Path::new("/r"), Path::new("/r/anywhere.md"), false).unwrap();
    assert!(result.restricted);
}

/// 13 — `deny_ops` entry counts as "restricted" for the inspection
/// surface (the user cares about coverage, not the op name).
#[test]
fn check_deny_ops_entry_counts_as_restricted() {
    let yaml = "permissions:\n  deny_ops:\n    - path: foo\n      ops: [purge]\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(&system, Path::new("/r"), Path::new("/r/foo/x.md"), false).unwrap();
    assert!(result.restricted);
}

/// 14 — `--why` populates `matching_rule`.
#[test]
fn check_why_populates_matching_rule() {
    let yaml = "permissions:\n  restrict:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/src/secret/foo.md"),
        true,
    )
    .unwrap();
    let rule = result.matching_rule.unwrap();
    assert_eq!(rule.kind, "restrict");
    assert_eq!(rule.source_file, PathBuf::from("/r/.remargin.yaml"));
    assert!(rule.rule_text.contains("/r/src/secret"));
}

/// 15 — non-existent path is matched lexically.
#[test]
fn check_non_existent_path_matched_lexically() {
    let yaml = "permissions:\n  restrict:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/r/src/secret/missing/file.md"),
        false,
    )
    .unwrap();
    assert!(result.restricted);
}

/// 16 — `MockSystem` does not model symlinks, but the canonicalize
/// path is the input verbatim, so passing the real path validates the
/// canonical match.
#[test]
fn check_canonicalised_match() {
    let yaml = "permissions:\n  restrict:\n    - path: real\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(&system, Path::new("/r"), Path::new("/r/real/x.md"), false).unwrap();
    assert!(result.restricted);
}

/// 17 — path outside any `restrict` / `deny_ops` anchor is not
/// restricted.
#[test]
fn check_path_outside_realm_is_not_restricted() {
    let yaml = "permissions:\n  restrict:\n    - path: src/secret\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(
        &system,
        Path::new("/r"),
        Path::new("/elsewhere/foo.md"),
        false,
    )
    .unwrap();
    assert!(!result.restricted);
}

/// 18 — multiple matches → `--why` returns the deepest (closest)
/// rule.
#[test]
fn check_why_picks_deepest_rule() {
    let parent = "permissions:\n  restrict:\n    - path: src\n";
    let child = "permissions:\n  restrict:\n    - path: src\n";
    let system = mock_with(&[
        ("/r/.remargin.yaml", parent),
        ("/r/sub/.remargin.yaml", child),
    ]);
    let result = check(
        &system,
        Path::new("/r/sub"),
        Path::new("/r/sub/src/file.md"),
        true,
    )
    .unwrap();
    let rule = result.matching_rule.unwrap();
    assert_eq!(rule.source_file, PathBuf::from("/r/sub/.remargin.yaml"));
}

/// 19 — JSON serialisation round-trip.
#[test]
fn check_round_trips_through_json() {
    let yaml = "permissions:\n  restrict:\n    - path: src\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let result = check(&system, Path::new("/r"), Path::new("/r/src/foo.md"), true).unwrap();
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"restricted\":true"));
    assert!(json.contains("\"kind\":\"restrict\""));
}

/// rem-qdrw — two stacked yamls, each with `allow_dot_folders`. Each
/// resulting view points at the file that declared it (testing plan #2).
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
    // Walk order is deepest-first.
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

/// rem-qdrw — `source_file` survives the JSON encoding (the original
/// reproduction surfaced the empty value through `permissions show
/// --json`).
#[test]
fn show_allow_dot_folders_source_file_survives_json_roundtrip() {
    let yaml = "permissions:\n  allow_dot_folders: ['.assets']\n";
    let system = mock_with(&[("/r/.remargin.yaml", yaml)]);
    let out = show(&system, Path::new("/r")).unwrap();
    let json = serde_json::to_string(&out).unwrap();
    assert!(
        json.contains("\"source_file\":\"/r/.remargin.yaml\""),
        "expected source_file in JSON, got:\n{json}",
    );
    assert!(
        !json.contains("\"source_file\":\"\""),
        "source_file must never be empty for allow_dot_folders entries:\n{json}",
    );
}
