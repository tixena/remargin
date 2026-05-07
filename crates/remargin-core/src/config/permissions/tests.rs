//! Schema + resolver tests.

use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;

use crate::config::Config;
use crate::config::permissions::Permissions;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::{
    RestrictPath, lint_permissions_in_parents, resolve_permissions, resolve_trusted_roots_for_cwd,
};

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
fn old_trusted_roots_field_is_rejected_post_eradication() {
    // The old deny-list-era `trusted_roots:` block must surface as a
    // hard parse error so users migrating an old config see the break
    // and migrate to `restrict:` instead.
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - /some/path
";
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("trusted_roots") || err.contains("unknown field"),
        "expected unknown-field error for `trusted_roots`, got: {err}",
    );
}

#[test]
fn unknown_field_under_restrict_entry_is_rejected() {
    let yaml = "\
permissions:
  restrict:
    - path: '*'
      bogus_inside_entry: true
";
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    let _err: serde_yaml::Error = result.unwrap_err();
}

#[test]
fn unknown_op_in_deny_ops_is_rejected() {
    let yaml = "\
identity: alice
permissions:
  deny_ops:
    - path: src
      ops:
        - delte
";
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("delte") || err.contains("unknown variant"),
        "expected unknown-variant error: {err}",
    );
}

#[test]
fn every_op_name_parses_in_deny_ops() {
    for op in OpName::ALL {
        let yaml = format!(
            "\
permissions:
  deny_ops:
    - path: src
      ops: [{}]
",
            op.as_str(),
        );
        let _: Config = serde_yaml::from_str(&yaml).unwrap();
    }
}

#[test]
fn deny_ops_unknown_op_in_resolver_names_source_file() {
    let yaml = "permissions:\n  deny_ops:\n    - path: src\n      ops: [delte]\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/realm/.remargin.yaml"));
    assert!(chain.contains("delte") || chain.contains("unknown variant"));
}

// ---------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------

fn write_yaml(system: MockSystem, path: &str, body: &str) -> MockSystem {
    system.with_file(Path::new(path), body.as_bytes()).unwrap()
}

#[test]
fn no_config_anywhere_returns_default() {
    let system = MockSystem::new().with_dir(Path::new("/realm")).unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert!(resolved.allow_dot_folders.is_empty());
    assert!(resolved.deny_ops.is_empty());
    assert!(resolved.restrict.is_empty());
}

#[test]
fn config_without_permissions_block_resolves_empty() {
    let system = write_yaml(
        MockSystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        "identity: alice\n",
    );
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert!(resolved.restrict.is_empty());
    assert!(resolved.deny_ops.is_empty());
    assert!(resolved.allow_dot_folders.is_empty());
}

#[test]
fn single_file_full_permissions_block_resolves_with_provenance() {
    let yaml = "\
identity: alice
permissions:
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

#[test]
fn wildcard_restrict_resolves_to_realm_root() {
    let yaml = "permissions:\n  restrict:\n    - path: '*'\n";
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

#[test]
fn relative_restrict_path_resolves_against_source_dir() {
    let yaml = "permissions:\n  restrict:\n    - path: src/secret\n";
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

#[test]
fn two_file_accumulation_preserves_order_and_provenance() {
    let parent = "permissions:\n  restrict:\n    - path: top\n";
    let child = "permissions:\n  restrict:\n    - path: nested\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved.restrict.len(), 2);
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

#[test]
fn malformed_yaml_surfaces_path_in_error() {
    let bad = "permissions:\n  restrict: : :\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), bad.as_bytes())
        .unwrap();
    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/realm/.remargin.yaml"));
}

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
    assert!(chain.contains("bogus"), "{chain}");
}

#[test]
fn also_deny_bash_and_cli_allowed_preserved() {
    let yaml = "\
permissions:
  restrict:
    - path: '*'
      also_deny_bash: ['rm', 'mv']
      cli_allowed: true
";
    let system = write_yaml(
        MockSystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.restrict[0].also_deny_bash,
        vec![String::from("rm"), String::from("mv")]
    );
    assert!(resolved.restrict[0].cli_allowed);
}

#[test]
fn deny_ops_accumulate_across_files_without_dedup() {
    let parent = "permissions:\n  deny_ops:\n    - path: top\n      ops: [purge]\n";
    let child = "permissions:\n  deny_ops:\n    - path: nested\n      ops: [delete]\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved.deny_ops.len(), 2);
}

#[test]
fn restrict_order_is_deepest_first() {
    let parent = "permissions:\n  restrict:\n    - path: top\n";
    let child = "permissions:\n  restrict:\n    - path: nested\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(
        resolved.restrict[0].source_file,
        PathBuf::from("/realm/sub/.remargin.yaml")
    );
}

#[test]
fn allow_dot_folders_accumulate_across_files() {
    let parent = "permissions:\n  allow_dot_folders: ['.git']\n";
    let child = "permissions:\n  allow_dot_folders: ['.cache']\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved.allow_dot_folders.len(), 2);
    assert_eq!(
        resolved.allow_dot_folders[0].names,
        vec![String::from(".cache")]
    );
    assert_eq!(
        resolved.allow_dot_folders[1].names,
        vec![String::from(".git")]
    );
    assert_eq!(
        resolved.allow_dot_folder_names(),
        vec![String::from(".cache"), String::from(".git")]
    );
}

#[test]
fn lint_permissions_collects_findings_across_parents() {
    let parent = "permissions:\n  deny_ops:\n    - path: top\n      ops: [delte]\n";
    let child = "permissions:\n  deny_ops:\n    - path: nested\n      ops: [purg]\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let findings = lint_permissions_in_parents(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(findings.len(), 2);
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
}

#[test]
fn lint_permissions_returns_empty_when_clean() {
    let yaml = "permissions:\n  deny_ops:\n    - path: src/secret\n      ops: [purge, delete]\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let findings = lint_permissions_in_parents(&system, Path::new("/realm")).unwrap();
    assert!(findings.is_empty());
}

#[test]
fn absolute_restrict_path_preserved() {
    let yaml = "permissions:\n  restrict:\n    - path: /etc/secret\n";
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

// ---------------------------------------------------------------------
// resolve_trusted_roots_for_cwd: derives the allow-listed path set
// from `restrict` entries; falls back to `[cwd]` in open mode.
// ---------------------------------------------------------------------

#[test]
fn trusted_roots_cwd_fallback_when_no_restrict_anywhere() {
    let system = MockSystem::new().with_dir(Path::new("/somewhere")).unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/somewhere")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/somewhere")]);
}

#[test]
fn trusted_roots_use_restrict_paths_when_declared() {
    let yaml = "permissions:\n  restrict:\n    - path: '*'\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/realm")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/realm")]);
}

#[test]
fn trusted_roots_collect_multiple_entries() {
    let yaml = "permissions:\n  restrict:\n    - path: a\n    - path: b\n";
    let system = MockSystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved,
        vec![PathBuf::from("/realm/a"), PathBuf::from("/realm/b")]
    );
}
