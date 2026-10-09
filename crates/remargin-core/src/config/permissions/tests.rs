//! Schema + resolver tests.

use std::path::{Path, PathBuf};

use os_shim::mock::MemorySystem;

use crate::config::Config;
use crate::config::permissions::op_name::OpName;
use crate::config::permissions::resolve::PermissionsLintError;
use crate::config::permissions::resolve::ResolvedAllowDotFolders;
use crate::config::permissions::resolve::ResolvedDenyOps;
use crate::config::permissions::resolve::ResolvedTrustedRoot;
use crate::config::permissions::resolve::{
    TrustedRootPath, lint_permissions_in_parents, resolve_permissions,
    resolve_trusted_roots_for_cwd,
};
use crate::config::permissions::{DenyOpsItem, Permissions, TrustedRootEntry};

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
    let roots = cfg.permissions.trusted_roots.as_ref().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path(), "*");
    assert_eq!(roots[0].also_deny_bash(), &[String::from("rm")]);
    assert!(roots[0].cli_allowed());
    assert_eq!(cfg.permissions.deny_ops.len(), 1);
    assert_eq!(cfg.permissions.deny_ops[0].path, "src/secret");
    assert_eq!(
        cfg.permissions.deny_ops[0].ops,
        vec![DenyOpsItem::Bare(OpName::Purge)],
    );
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
fn trusted_roots_field_parses() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
    - /some/path
    - ~/notes
";
    let cfg: Config = serde_yaml::from_str(yaml).unwrap();
    let paths: Vec<&str> = cfg
        .permissions
        .trusted_roots
        .as_ref()
        .unwrap()
        .iter()
        .map(TrustedRootEntry::path)
        .collect();
    assert_eq!(paths, vec!["/some/path", "~/notes"]);
}

#[test]
fn unknown_field_under_restrict_entry_is_rejected() {
    let yaml = "\
permissions:
  trusted_roots:
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
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/realm/.remargin.yaml"));
    assert!(chain.contains("delte") || chain.contains("unknown variant"));
}

fn write_yaml(system: MemorySystem, path: &str, body: &str) -> MemorySystem {
    system.with_file(Path::new(path), body.as_bytes()).unwrap()
}

#[test]
fn no_config_anywhere_returns_default() {
    let system = MemorySystem::new().with_dir(Path::new("/realm")).unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.allow_dot_folders,
        [] as [ResolvedAllowDotFolders; 0]
    );
    assert_eq!(resolved.deny_ops, [] as [ResolvedDenyOps; 0]);
    assert_eq!(resolved.trusted_roots, [] as [ResolvedTrustedRoot; 0]);
}

#[test]
fn config_without_permissions_block_resolves_empty() {
    let system = write_yaml(
        MemorySystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        "identity: alice\n",
    );
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(resolved.trusted_roots, [] as [ResolvedTrustedRoot; 0]);
    assert_eq!(resolved.deny_ops, [] as [ResolvedDenyOps; 0]);
    assert_eq!(
        resolved.allow_dot_folders,
        [] as [ResolvedAllowDotFolders; 0]
    );
}

#[test]
fn single_file_full_permissions_block_resolves_with_provenance() {
    let yaml = "\
identity: alice
permissions:
  trusted_roots:
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
        MemorySystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );

    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    let source = PathBuf::from("/realm/.remargin.yaml");

    assert_eq!(resolved.trusted_roots.len(), 1);
    assert_eq!(
        resolved.trusted_roots[0].path,
        TrustedRootPath::Absolute(PathBuf::from("/realm/src"))
    );
    assert!(resolved.trusted_roots[0].cli_allowed);
    assert_eq!(resolved.trusted_roots[0].source_file, source);

    assert_eq!(resolved.deny_ops.len(), 1);
    assert_eq!(
        resolved.deny_ops[0].path,
        PathBuf::from("/realm/src/secret")
    );
    assert_eq!(resolved.deny_ops[0].ops.len(), 1);
    assert_eq!(resolved.deny_ops[0].ops[0].name, OpName::Purge);
    assert_eq!(resolved.deny_ops[0].ops[0].exceptions, [] as [String; 0]);
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
    let yaml = "permissions:\n  trusted_roots:\n    - path: '*'\n";
    let system = write_yaml(
        MemorySystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(resolved.trusted_roots.len(), 1);
    assert_eq!(
        resolved.trusted_roots[0].path,
        TrustedRootPath::Wildcard {
            realm_root: PathBuf::from("/realm"),
        }
    );
}

#[test]
fn relative_restrict_path_resolves_against_source_dir() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    let system = write_yaml(
        MemorySystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].path,
        TrustedRootPath::Absolute(PathBuf::from("/realm/src/secret"))
    );
}

#[test]
fn two_file_accumulation_preserves_order_and_provenance() {
    let parent = "permissions:\n  trusted_roots:\n    - path: top\n";
    let child = "permissions:\n  trusted_roots:\n    - path: nested\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(resolved.trusted_roots.len(), 2);
    assert_eq!(
        resolved.trusted_roots[0].path,
        TrustedRootPath::Absolute(PathBuf::from("/realm/sub/nested"))
    );
    assert_eq!(
        resolved.trusted_roots[0].source_file,
        PathBuf::from("/realm/sub/.remargin.yaml")
    );
    assert_eq!(
        resolved.trusted_roots[1].path,
        TrustedRootPath::Absolute(PathBuf::from("/realm/top"))
    );
    assert_eq!(
        resolved.trusted_roots[1].source_file,
        PathBuf::from("/realm/.remargin.yaml")
    );
}

#[test]
fn malformed_yaml_surfaces_path_in_error() {
    let bad = "permissions:\n  trusted_roots: : :\n";
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
  trusted_roots:
    - path: '*'
      also_deny_bash: ['rm', 'mv']
      cli_allowed: true
";
    let system = write_yaml(
        MemorySystem::new().with_dir(Path::new("/realm")).unwrap(),
        "/realm/.remargin.yaml",
        yaml,
    );
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].also_deny_bash,
        vec![String::from("rm"), String::from("mv")]
    );
    assert!(resolved.trusted_roots[0].cli_allowed);
}

#[test]
fn deny_ops_accumulate_across_files_without_dedup() {
    let parent = "permissions:\n  deny_ops:\n    - path: top\n      ops: [purge]\n";
    let child = "permissions:\n  deny_ops:\n    - path: nested\n      ops: [delete]\n";
    let system = MemorySystem::new()
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
    let parent = "permissions:\n  trusted_roots:\n    - path: top\n";
    let child = "permissions:\n  trusted_roots:\n    - path: nested\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].source_file,
        PathBuf::from("/realm/sub/.remargin.yaml")
    );
}

#[test]
fn allow_dot_folders_accumulate_across_files() {
    let parent = "permissions:\n  allow_dot_folders: ['.git']\n";
    let child = "permissions:\n  allow_dot_folders: ['.cache']\n";
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
fn lint_permissions_flags_legacy_to_field_as_hard_finding() {
    let yaml = "permissions:\n  deny_ops:\n    - path: .\n      ops: [purge]\n      to: [eduardo-burgos]\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let findings = lint_permissions_in_parents(&system, Path::new("/realm")).unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.message.contains("legacy `to:`") && f.message.contains("exceptions")),
        "expected migration-recipe finding; got {findings:#?}",
    );
}

#[test]
fn deny_ops_full_record_with_exceptions_parses() {
    let yaml = "\
permissions:
  deny_ops:
    - path: .
      ops:
        - sign
        - name: purge
          exceptions: [eduardo-burgos, remargin_dev_agent]
        - delete
        - name: write
          exceptions: [eduardo-burgos]
";
    let cfg: Config = serde_yaml::from_str(yaml).unwrap();
    let entry = &cfg.permissions.deny_ops[0];
    assert_eq!(entry.ops.len(), 4);
    assert_eq!(entry.ops[0].name(), &OpName::Sign);
    assert_eq!(entry.ops[0].exceptions(), [] as [String; 0]);
    assert_eq!(entry.ops[1].name(), &OpName::Purge);
    assert_eq!(
        entry.ops[1].exceptions(),
        &[
            String::from("eduardo-burgos"),
            String::from("remargin_dev_agent")
        ],
    );
    assert_eq!(entry.ops[2].name(), &OpName::Delete);
    assert_eq!(entry.ops[3].name(), &OpName::Write);
    assert_eq!(entry.ops[3].exceptions(), &[String::from("eduardo-burgos")]);
}

#[test]
fn deny_ops_legacy_to_field_fails_to_parse() {
    let yaml = "permissions:\n  deny_ops:\n    - path: .\n      ops: [purge]\n      to: [eduardo-burgos]\n";
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    let _err = result.unwrap_err();
}

#[test]
fn lint_permissions_returns_empty_when_clean() {
    let yaml = "permissions:\n  deny_ops:\n    - path: src/secret\n      ops: [purge, delete]\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let findings = lint_permissions_in_parents(&system, Path::new("/realm")).unwrap();
    assert_eq!(findings, [] as [PermissionsLintError; 0]);
}

#[test]
fn in_realm_absolute_restrict_path_preserved() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: /realm/etc/secret\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].path,
        TrustedRootPath::Absolute(PathBuf::from("/realm/etc/secret"))
    );
}

/// Resolution fails closed, naming the yaml, the entry as written and the resolved anchor.
#[test]
fn out_of_realm_absolute_entry_fails_resolution() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: /other/secret\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/realm/.remargin.yaml"), "{chain}");
    assert!(chain.contains("/other/secret"), "{chain}");
}

/// The check runs on the resolved anchor, so a `../` climb is caught without looking absolute.
#[test]
fn dotdot_escape_entry_fails_resolution() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: ../sibling\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/realm/.remargin.yaml"), "{chain}");
    assert!(chain.contains("../sibling"), "{chain}");
    assert!(chain.contains("/sibling"), "{chain}");
}

#[test]
fn tilde_expansion_escape_fails_resolution() {
    let yaml = "permissions:\n  trusted_roots:\n    - ~/notes\n";
    let system = MemorySystem::new()
        .with_env("HOME", "/home/alice")
        .unwrap()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let err = resolve_permissions(&system, Path::new("/realm")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/realm/.remargin.yaml"), "{chain}");
    assert!(chain.contains("/home/alice/notes"), "{chain}");
}

#[test]
fn tilde_expansion_inside_realm_resolves() {
    let yaml = "permissions:\n  trusted_roots:\n    - ~/notes\n";
    let system = MemorySystem::new()
        .with_env("HOME", "/realm/home/alice")
        .unwrap()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.trusted_roots[0].path,
        TrustedRootPath::Absolute(PathBuf::from("/realm/home/alice/notes"))
    );
}

/// Lint reports the out-of-realm entry, naming the entry as written.
#[test]
fn lint_reports_out_of_realm_trusted_root() {
    let yaml = "permissions:\n  trusted_roots:\n    - path: /other/secret\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let findings = lint_permissions_in_parents(&system, Path::new("/realm")).unwrap();
    assert!(
        findings.iter().any(|f| f.message.contains("/other/secret")
            && f.message.contains("outside the realm")),
        "expected out-of-realm finding; got {findings:#?}",
    );
}

#[test]
fn trusted_roots_cwd_fallback_when_none_declared() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/somewhere"))
        .unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/somewhere")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/somewhere")]);
}

#[test]
fn trusted_roots_use_declared_paths() {
    let yaml = "permissions:\n  trusted_roots:\n    - /realm/a\n    - /realm/b\n";
    let system = MemorySystem::new()
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

#[test]
fn trusted_roots_expand_tilde_against_mock_home() {
    let yaml = "permissions:\n  trusted_roots:\n    - ~/notes\n";
    let system = MemorySystem::new()
        .with_env("HOME", "/realm/home/alice")
        .unwrap()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/realm")).unwrap();
    assert_eq!(resolved, vec![PathBuf::from("/realm/home/alice/notes")]);
}

#[test]
fn permissions_block_with_no_trusted_roots_key_parses_to_none() {
    let yaml = "permissions:\n  allow_dot_folders: ['.git']\n";
    let cfg: Config = serde_yaml::from_str(yaml).unwrap();
    assert!(cfg.permissions.trusted_roots.is_none());
}

#[test]
fn permissions_block_with_empty_trusted_roots_list_parses_to_some_empty() {
    let yaml = "permissions:\n  trusted_roots: []\n";
    let cfg: Config = serde_yaml::from_str(yaml).unwrap();
    let roots = cfg.permissions.trusted_roots.as_ref().unwrap();
    assert_eq!(roots.as_slice(), []);
}

#[test]
fn resolver_records_lock_when_trusted_roots_explicitly_empty() {
    let yaml = "permissions:\n  trusted_roots: []\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(
        resolved.trusted_roots_lock,
        Some(PathBuf::from("/realm/.remargin.yaml"))
    );
    assert_eq!(resolved.trusted_roots, [] as [ResolvedTrustedRoot; 0]);
    assert!(!resolved.trusted_roots_unconstrained());
}

#[test]
fn resolver_leaves_lock_unset_when_key_absent() {
    let yaml = "permissions:\n  allow_dot_folders: ['.git']\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert!(resolved.trusted_roots_lock.is_none());
    assert!(resolved.trusted_roots_unconstrained());
}

#[test]
fn resolver_records_deepest_lock_first_in_walk() {
    let parent = "permissions:\n  trusted_roots: []\n";
    let child = "permissions:\n  trusted_roots: []\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), parent.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/sub/.remargin.yaml"), child.as_bytes())
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(
        resolved.trusted_roots_lock,
        Some(PathBuf::from("/realm/sub/.remargin.yaml"))
    );
}

#[test]
fn resolve_trusted_roots_for_cwd_locked_returns_empty() {
    let yaml = "permissions:\n  trusted_roots: []\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    let resolved = resolve_trusted_roots_for_cwd(&system, Path::new("/realm")).unwrap();
    assert_eq!(resolved, [] as [PathBuf; 0]);
}

#[test]
fn cli_allowed_default_deny_when_absent() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), b"identity: alice\n")
        .unwrap();
    let resolved = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert!(
        resolved.cli_allowed.is_none(),
        "expected None (not declared)"
    );
    assert!(!resolved.cli_allowed(), "effective default must be false");
}

#[test]
fn cli_allowed_nearest_wins_deny() {
    let root_yaml = "identity: alice\n";
    let mid_yaml = "identity: alice\n";
    let deep_yaml = "permissions:\n  cli_allowed: false\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm/a/aa"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), root_yaml.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/a/.remargin.yaml"), mid_yaml.as_bytes())
        .unwrap()
        .with_file(
            Path::new("/realm/a/aa/.remargin.yaml"),
            deep_yaml.as_bytes(),
        )
        .unwrap();

    let from_aa = resolve_permissions(&system, Path::new("/realm/a/aa")).unwrap();
    assert_eq!(from_aa.cli_allowed, Some(false));
    assert!(!from_aa.cli_allowed());

    let from_a = resolve_permissions(&system, Path::new("/realm/a")).unwrap();
    assert!(from_a.cli_allowed.is_none());
    assert!(!from_a.cli_allowed());

    let from_root = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert!(from_root.cli_allowed.is_none());
    assert!(!from_root.cli_allowed());
}

#[test]
fn cli_allowed_root_allow_inherited() {
    let root_yaml = "permissions:\n  cli_allowed: true\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm/sub"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), root_yaml.as_bytes())
        .unwrap();

    let from_root = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(from_root.cli_allowed, Some(true));
    assert!(from_root.cli_allowed());

    let from_sub = resolve_permissions(&system, Path::new("/realm/sub")).unwrap();
    assert_eq!(from_sub.cli_allowed, Some(true));
    assert!(from_sub.cli_allowed());
}

#[test]
fn cli_allowed_deeper_override_re_allows() {
    let root_yaml = "permissions:\n  cli_allowed: true\n";
    let mid_yaml = "permissions:\n  cli_allowed: false\n";
    let deep_yaml = "permissions:\n  cli_allowed: true\n";
    let system = MemorySystem::new()
        .with_dir(Path::new("/realm/a/aa"))
        .unwrap()
        .with_file(Path::new("/realm/.remargin.yaml"), root_yaml.as_bytes())
        .unwrap()
        .with_file(Path::new("/realm/a/.remargin.yaml"), mid_yaml.as_bytes())
        .unwrap()
        .with_file(
            Path::new("/realm/a/aa/.remargin.yaml"),
            deep_yaml.as_bytes(),
        )
        .unwrap();

    let from_root = resolve_permissions(&system, Path::new("/realm")).unwrap();
    assert_eq!(from_root.cli_allowed, Some(true));
    assert!(from_root.cli_allowed());

    let from_a = resolve_permissions(&system, Path::new("/realm/a")).unwrap();
    assert_eq!(from_a.cli_allowed, Some(false));
    assert!(!from_a.cli_allowed());

    let from_aa = resolve_permissions(&system, Path::new("/realm/a/aa")).unwrap();
    assert_eq!(from_aa.cli_allowed, Some(true));
    assert!(from_aa.cli_allowed());
}

#[test]
fn permissions_block_parses_cli_allowed() {
    let yaml_allow = "permissions:\n  cli_allowed: true\n";
    let yaml_deny = "permissions:\n  cli_allowed: false\n";
    let yaml_absent = "permissions:\n  allow_dot_folders: ['.git']\n";

    let cfg_allow: Config = serde_yaml::from_str(yaml_allow).unwrap();
    assert_eq!(cfg_allow.permissions.cli_allowed, Some(true));

    let cfg_deny: Config = serde_yaml::from_str(yaml_deny).unwrap();
    assert_eq!(cfg_deny.permissions.cli_allowed, Some(false));

    let cfg_absent: Config = serde_yaml::from_str(yaml_absent).unwrap();
    assert!(cfg_absent.permissions.cli_allowed.is_none());
}
