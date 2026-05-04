//! Unit tests for the permissions schema and parent-walk resolver
//! (rem-yj1j.1 / T22).
//!
//! All tests run against `os_shim::mock::MockSystem` so no real
//! filesystem state is required.

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::config::Config;
use crate::config::permissions::Permissions;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::{
    ResolvedPermissions, RestrictPath, lint_permissions_in_parents, resolve_permissions,
};

// ---------------------------------------------------------------------
// Parser-level tests for the on-disk schema.
// ---------------------------------------------------------------------

/// A `.remargin.yaml` without a `permissions:` block must continue to
/// load and produce `Permissions::default()`. This is the back-compat
/// guarantee.
#[test]
fn config_without_permissions_block_defaults_to_empty() {
    let yaml = "identity: alice\n";
    let cfg: Config = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(cfg.permissions, Permissions::default());
}

#[test]
fn config_with_full_permissions_block_parses() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - ~/notes
  restrict:
    - path: '*'
      also_deny_bash:
        - rm
      cli_allowed: true
  deny_ops:
    - path: src/secret
      ops:
        - purge
  allow_dot_folders:
    - .github
";
    let cfg: Config = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(cfg.permissions.trusted_roots, vec![String::from("~/notes")]);
    assert_eq!(cfg.permissions.restrict.len(), 1);
    assert_eq!(cfg.permissions.restrict[0].path, "*");
    assert_eq!(
        cfg.permissions.restrict[0].also_deny_bash,
        vec![String::from("rm")]
    );
    assert!(cfg.permissions.restrict[0].cli_allowed);
    assert_eq!(cfg.permissions.deny_ops.len(), 1);
    assert_eq!(cfg.permissions.deny_ops[0].path, "src/secret");
    assert_eq!(cfg.permissions.deny_ops[0].ops, vec![OpName::Purge]);
    assert_eq!(
        cfg.permissions.allow_dot_folders,
        vec![String::from(".github")]
    );
}

#[test]
fn unknown_field_under_permissions_is_rejected() {
    let yaml = "\
identity: alice
permissions:
  bogus: true
";
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("bogus"), "error did not mention key: {err}");
}

#[test]
fn unknown_field_under_restrict_entry_is_rejected() {
    let yaml = "\
identity: alice
permissions:
  restrict:
    - path: src
      bogus: true
";
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("bogus"), "error did not mention key: {err}");
}

/// rem-welo: an unknown op name in `permissions.deny_ops.ops` is
/// rejected at parse time. The error names the offending typo and
/// lists the valid ops.
#[test]
fn unknown_op_in_deny_ops_is_rejected() {
    let yaml = "\
identity: alice
permissions:
  deny_ops:
    - path: src/secret
      ops: [purg, delete]
";
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("purg"), "error did not name typo: {err}");
    // serde_yaml's "expected one of …" enumerates every valid variant,
    // which is the user-visible "valid ops" list. Spot-check three.
    for op in ["purge", "delete", "sandbox-add"] {
        assert!(
            err.contains(op),
            "error did not list valid op `{op}`: {err}"
        );
    }
}

/// rem-welo: every variant in `OpName::ALL` parses successfully when
/// listed verbatim in a `.remargin.yaml`. Adding a new op variant
/// without forgetting its kebab-case form keeps this green.
#[test]
fn every_op_name_parses_in_deny_ops() {
    use core::fmt::Write as _;

    let mut ops_yaml = String::new();
    for op in OpName::ALL {
        let _ = writeln!(ops_yaml, "        - {}", op.as_str());
    }
    let yaml = format!(
        "identity: alice\npermissions:\n  deny_ops:\n    - path: src\n      ops:\n{ops_yaml}",
    );
    let cfg: Config = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(cfg.permissions.deny_ops.len(), 1);
    assert_eq!(cfg.permissions.deny_ops[0].ops.len(), OpName::ALL.len());
}

/// rem-welo: a typo in `deny_ops.ops` surfaces an error whose chain
/// names the source `.remargin.yaml` (acceptance: error message names
/// the file).
#[test]
fn deny_ops_unknown_op_in_resolver_names_source_file() {
    let yaml = "\
identity: alice
permissions:
  deny_ops:
    - path: src/secret
      ops: [purg]
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/realm/.remargin.yaml"),
        "error did not name file: {chain}"
    );
    assert!(chain.contains("purg"), "error did not name typo: {chain}");
}

// ---------------------------------------------------------------------
// Resolver-level tests against the spec's acceptance scenarios.
// ---------------------------------------------------------------------

fn write_yaml(system: MockSystem, path: &str, body: &str) -> MockSystem {
    system.with_file(Path::new(path), body.as_bytes()).unwrap()
}

/// Scenario 1: no `.remargin.yaml` anywhere → empty resolved.
#[test]
fn no_config_anywhere_returns_default() {
    let system = MockSystem::new().with_dir(Path::new("/tmp/empty")).unwrap();

    let resolved = resolve_permissions(&system, Path::new("/tmp/empty")).unwrap();
    assert_eq!(resolved, ResolvedPermissions::default());
}

/// Scenario 2: file exists but no `permissions:` block.
#[test]
fn config_without_permissions_block_resolves_empty() {
    let system = write_yaml(
        MockSystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        "identity: alice\n",
    );

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert!(resolved.trusted_roots.is_empty());
    assert!(resolved.restrict.is_empty());
    assert!(resolved.deny_ops.is_empty());
    assert!(resolved.allow_dot_folders.is_empty());
}

/// Scenario 3 (rem-egp9): a single file with all five keys
/// populated. `trusted_roots` declared inside the realm's parent
/// directory satisfies the containment rule.
#[test]
fn single_file_full_permissions_block_resolves_with_provenance() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - /realm/notes
  restrict:
    - path: src
      cli_allowed: true
  deny_ops:
    - path: src/secret
      ops:
        - purge
  allow_dot_folders:
    - .github
";
    let system = write_yaml(
        MockSystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();

    let source = PathBuf::from("/realm/.remargin.yaml");
    assert_eq!(resolved.trusted_roots.len(), 1);
    assert_eq!(
        resolved.trusted_roots[0].path,
        PathBuf::from("/realm/notes")
    );
    assert_eq!(resolved.trusted_roots[0].source_file, source);

    assert_eq!(resolved.restrict.len(), 1);
    assert_eq!(
        resolved.restrict[0].path,
        RestrictPath::Absolute(PathBuf::from("/realm/src"))
    );
    assert!(resolved.restrict[0].cli_allowed);
    assert_eq!(resolved.restrict[0].source_file, source);

    assert_eq!(resolved.deny_ops.len(), 1);
    assert_eq!(
        resolved.deny_ops[0].path,
        PathBuf::from("/realm/src/secret")
    );
    assert_eq!(resolved.deny_ops[0].ops, vec![OpName::Purge]);
    assert_eq!(resolved.deny_ops[0].source_file, source);

    assert_eq!(resolved.allow_dot_folders.len(), 1);
    assert_eq!(
        resolved.allow_dot_folders[0].names,
        vec![String::from(".github")],
    );
    assert_eq!(resolved.allow_dot_folders[0].source_file, source);
}

/// Scenario 4: wildcard `*` resolves to `RestrictPath::Wildcard`
/// anchored at the declaring file's parent.
#[test]
fn wildcard_restrict_resolves_to_realm_root() {
    let yaml = "\
identity: alice
permissions:
  restrict:
    - path: '*'
";
    let system = write_yaml(
        MockSystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(resolved.restrict.len(), 1);
    assert_eq!(
        resolved.restrict[0].path,
        RestrictPath::Wildcard {
            realm_root: PathBuf::from("/realm"),
        }
    );
}

/// Scenario 5: relative restrict path resolves against the source
/// file's parent directory.
#[test]
fn relative_restrict_path_resolves_against_source_dir() {
    let yaml = "\
identity: alice
permissions:
  restrict:
    - path: src/secret
";
    let system = write_yaml(
        MockSystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.restrict[0].path,
        RestrictPath::Absolute(PathBuf::from("/realm/src/secret"))
    );
}

/// Scenario 6: parent + child both with restrict accumulate; deepest
/// file appears first; each entry remembers its source.
#[test]
fn two_file_accumulation_preserves_order_and_provenance() {
    let parent = "\
identity: alice
permissions:
  restrict:
    - path: top
";
    let child = "\
identity: alice
permissions:
  restrict:
    - path: nested
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();

    assert_eq!(resolved.restrict.len(), 2);
    // Deepest first.
    assert_eq!(
        resolved.restrict[0].path,
        RestrictPath::Absolute(PathBuf::from("/realm/sub/nested"))
    );
    assert_eq!(
        resolved.restrict[0].source_file,
        PathBuf::from("/realm/sub/.remargin.yaml")
    );
    assert_eq!(
        resolved.restrict[1].path,
        RestrictPath::Absolute(PathBuf::from("/realm/top"))
    );
    assert_eq!(
        resolved.restrict[1].source_file,
        PathBuf::from("/realm/.remargin.yaml")
    );
}

/// Scenario 7 (rem-egp9): `~`-prefixed `trusted_roots` expand against
/// the active `HOME` environment variable. Containment requires the
/// declared entry to live below the declaring `.remargin.yaml`'s
/// parent — a `~/notes` declaration belongs on a config file under
/// `~/`, so the test now declares the realm at `/home/alice` and
/// resolves from there.
#[test]
fn trusted_root_with_tilde_expands_against_home() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - ~/notes
";
    let system = MockSystem::new()
        .with_env("HOME", "/home/alice")
        .unwrap()
        .with_dir(Path::new("/home/alice"))
        .unwrap()
        .with_file(Path::new("/home/alice/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/home/alice")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].path,
        PathBuf::from("/home/alice/notes")
    );
}

/// Scenario 8 substitute (rem-egp9): `MockSystem` does not model
/// symlinks; `canonicalize` returns the absolute input verbatim. The
/// containment rule requires the declared entry to live below the
/// declaring `.remargin.yaml` — declare a sub-tree of the realm and
/// verify it survives the canonicalize-then-fall-back path.
#[test]
fn trusted_root_absolute_path_preserved_via_canonicalize() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - /realm/notes
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/notes"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].path,
        PathBuf::from("/realm/notes")
    );
}

/// Scenario 9: malformed YAML surfaces an error that names the file.
#[test]
fn malformed_yaml_surfaces_path_in_error() {
    let bad = "permissions:\n  trusted_roots: : :\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), bad.as_bytes())
        .unwrap();

    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/realm/.remargin.yaml"),
        "error did not name file: {chain}"
    );
}

/// Scenario 10: unknown field under `permissions:` is rejected by the
/// resolver too (the projection struct uses the same on-disk schema).
#[test]
fn unknown_field_under_permissions_block_rejected_by_resolver() {
    let yaml = "permissions:\n  bogus: true\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("bogus"),
        "error did not mention bogus key: {chain}"
    );
}

/// Scenario 11: `also_deny_bash` and `cli_allowed` carry through the
/// resolver verbatim.
#[test]
fn also_deny_bash_and_cli_allowed_preserved() {
    let yaml = "\
identity: alice
permissions:
  restrict:
    - path: src
      also_deny_bash: [rm, 'git rm']
      cli_allowed: true
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.restrict[0].also_deny_bash,
        vec![String::from("rm"), String::from("git rm")]
    );
    assert!(resolved.restrict[0].cli_allowed);
}

/// Scenario 12: `deny_ops` accumulate across files without dedup.
#[test]
fn deny_ops_accumulate_across_files_without_dedup() {
    let parent = "\
identity: alice
permissions:
  deny_ops:
    - path: src/foo
      ops: [purge]
";
    let child = "\
identity: alice
permissions:
  deny_ops:
    - path: src/foo
      ops: [delete]
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved.deny_ops.len(), 2);
    assert_eq!(resolved.deny_ops[0].ops, vec![OpName::Delete]);
    assert_eq!(resolved.deny_ops[1].ops, vec![OpName::Purge]);
}

/// Scenario 13: walk order — closest file first.
#[test]
fn restrict_order_is_deepest_first() {
    let parent = "\
identity: alice
permissions:
  restrict:
    - path: top
";
    let child = "\
identity: alice
permissions:
  restrict:
    - path: nested
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(
        resolved.restrict[0].path,
        RestrictPath::Absolute(PathBuf::from("/realm/sub/nested"))
    );
}

/// Scenario 14: `allow_dot_folders` accumulate across files.
#[test]
fn allow_dot_folders_accumulate_across_files() {
    let parent = "\
identity: alice
permissions:
  allow_dot_folders: ['.git']
";
    let child = "\
identity: alice
permissions:
  allow_dot_folders: ['.cache']
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved.allow_dot_folders.len(), 2);
    // Walk order: deepest file first.
    assert_eq!(
        resolved.allow_dot_folders[0].names,
        vec![String::from(".cache")],
    );
    assert_eq!(
        resolved.allow_dot_folders[0].source_file,
        PathBuf::from("/realm/sub/.remargin.yaml"),
    );
    assert_eq!(
        resolved.allow_dot_folders[1].names,
        vec![String::from(".git")],
    );
    assert_eq!(
        resolved.allow_dot_folders[1].source_file,
        PathBuf::from("/realm/.remargin.yaml"),
    );
    assert_eq!(
        resolved.allow_dot_folder_names(),
        vec![String::from(".cache"), String::from(".git")],
    );
}

/// Scenario 15 (rem-egp9): a `trusted_roots` entry that points
/// outside the declaring directory is rejected at parse time by the
/// containment rule, regardless of whether the path exists on disk.
/// Best-effort canonicalization survives for paths INSIDE the
/// declaring folder; see `trusted_root_nonexistent_subpath_kept_best_effort`.
#[test]
fn trusted_root_outside_declaring_folder_is_rejected() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - /does/not/exist
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/does/not/exist") && chain.contains("/realm/.remargin.yaml"),
        "expected containment-violation error citing path + source: {chain}",
    );
}

/// rem-egp9: a `trusted_roots` entry that lives INSIDE the declaring
/// folder but does not yet exist on disk is kept as a best-effort
/// canonical — same fallback the rest of the resolver uses (rem-lwxa
/// flags such paths separately).
#[test]
fn trusted_root_nonexistent_subpath_kept_best_effort() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - /realm/subdir-not-on-disk
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].path,
        PathBuf::from("/realm/subdir-not-on-disk")
    );
}

/// rem-welo: `lint_permissions_in_parents` reports unknown op names
/// without short-circuiting the walk. A typo in a child `.remargin.yaml`
/// AND an unrelated typo in the parent both surface in one pass.
#[test]
fn lint_permissions_collects_findings_across_parents() {
    let parent = "\
identity: alice
permissions:
  deny_ops:
    - path: top
      ops: [delte]
";
    let child = "\
identity: alice
permissions:
  deny_ops:
    - path: nested
      ops: [purg]
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();

    let findings = lint_permissions_in_parents(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(findings.len(), 2);
    // Walk order: deepest first.
    assert_eq!(
        findings[0].source_file,
        PathBuf::from("/realm/sub/.remargin.yaml")
    );
    assert!(findings[0].message.contains("purg"));
    assert_eq!(
        findings[1].source_file,
        PathBuf::from("/realm/.remargin.yaml")
    );
    assert!(findings[1].message.contains("delte"));
    // Locations should be populated by `serde_yaml`.
    for finding in &findings {
        assert!(finding.line.is_some(), "missing line: {finding:?}");
        assert!(finding.column.is_some(), "missing column: {finding:?}");
    }
}

/// rem-welo: a clean realm produces zero findings.
#[test]
fn lint_permissions_returns_empty_when_clean() {
    let yaml = "\
identity: alice
permissions:
  deny_ops:
    - path: src/secret
      ops: [purge, delete]
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let findings = lint_permissions_in_parents(&system, Path::new("/realm")).unwrap();
    assert!(findings.is_empty());
}

// ---------------------------------------------------------------------
// rem-egp9 — `trusted_roots` narrowing + containment + CWD fallback
// ---------------------------------------------------------------------

/// rem-egp9: child `trusted_roots` that intersect the parent's set
/// survive (narrowing in action). Parent declares the realm and a
/// sibling; child declares a subfolder of the realm. The result is
/// the child's narrower set. Both entries satisfy containment (each
/// is below its declaring folder) AND the child's entry is a subset
/// of some parent entry.
#[test]
fn trusted_roots_child_subset_narrows_to_child() {
    use crate::config::permissions::resolve::resolve_trusted_roots_for_cwd;
    let parent = "permissions:\n  trusted_roots:\n    - /realm\n";
    let child = "permissions:\n  trusted_roots:\n    - /realm/sub/inner\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub/inner"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();

    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/realm/sub/inner")]);
}

/// rem-egp9: child `trusted_roots` entry that is NOT a subset of the
/// parent's set is rejected at parse time, with an error citing the
/// offending path and source file. Parent trusts only `/realm/safe`;
/// child wants to trust `/realm/sub/d`, which is a sibling of `safe`,
/// not inside it — intersection violation.
#[test]
fn trusted_roots_child_not_subset_is_parse_error() {
    let parent = "permissions:\n  trusted_roots:\n    - /realm/safe\n";
    let child = "permissions:\n  trusted_roots:\n    - /realm/sub/d\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub/d"))
        .unwrap()
        .with_dir(Path::new("/realm/safe"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();

    let err = resolve_permissions(&system, Path::new("/realm/sub")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/realm/sub/d") && chain.contains("/realm/sub/.remargin.yaml"),
        "expected intersection-violation error citing path + source: {chain}",
    );
}

/// rem-egp9: a `trusted_roots` entry that escapes the declaring
/// folder via a path outside the realm (e.g. `/etc/passwd`) is
/// rejected at parse time with a clear containment-violation error.
#[test]
fn trusted_roots_outside_declaring_folder_rejected() {
    let yaml = "permissions:\n  trusted_roots:\n    - /etc/passwd\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/etc/passwd") && chain.contains("/realm/.remargin.yaml"),
        "expected containment-violation error citing path + source: {chain}",
    );
}

/// rem-egp9: no `.remargin.yaml` anywhere on the walk → `trusted_roots`
/// resolves to `[cwd]` (open semantics fallback).
#[test]
fn trusted_roots_cwd_fallback_with_no_config() {
    use crate::config::permissions::resolve::resolve_trusted_roots_for_cwd;
    let system = MockSystem::new().with_dir(Path::new("/somewhere")).unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/somewhere")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/somewhere")]);
}

/// rem-egp9: `.remargin.yaml` exists but does not declare
/// `trusted_roots:` → fallback to `[cwd]`.
#[test]
fn trusted_roots_cwd_fallback_with_config_lacking_trusted_roots() {
    use crate::config::permissions::resolve::resolve_trusted_roots_for_cwd;
    let yaml = "identity: alice\npermissions:\n  restrict:\n    - path: src\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/realm")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/realm")]);
}

/// rem-egp9: empty intersection is valid YAML (every layer narrowed
/// to nothing). The resolver returns an empty `trusted_roots` set;
/// `resolve_trusted_roots_for_cwd` does NOT trigger the CWD fallback
/// (the user explicitly declared nothing should be trusted at this
/// level). `op_guard` / per-op sandbox refuse mutating ops accordingly.
#[test]
fn trusted_roots_empty_intersection_kept_empty() {
    use crate::config::permissions::resolve::resolve_trusted_roots_for_cwd;
    let parent = "permissions:\n  trusted_roots:\n    - /realm/a\n";
    let child = "permissions:\n  trusted_roots: []\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    // `permissions: `trusted_roots`: []` declares the empty set
    // explicitly. Since the walk had a non-empty `trusted_roots`
    // declaration somewhere (the parent), the fallback does not
    // re-trigger; the result is the parent's set (the empty array
    // contributed nothing to intersect).
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/realm/a")]);
}

/// Bonus: an absolute `restrict.path` is preserved (rather than being
/// joined under the source dir).
#[test]
fn absolute_restrict_path_preserved() {
    let yaml = "\
identity: alice
permissions:
  restrict:
    - path: /etc/secret
";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.restrict[0].path,
        RestrictPath::Absolute(PathBuf::from("/etc/secret"))
    );
}
