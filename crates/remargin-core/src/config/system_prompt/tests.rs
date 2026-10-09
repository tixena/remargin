//! Tests for the system-prompt walk: nearest ancestor, the default fallback, naming and the
//! runner.

use std::path::Path;

use os_shim::mock::MemorySystem;

use super::{DEFAULT_PROMPT_BODY, resolve_system_prompt};

#[test]
fn nearest_ancestor_wins() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a/b/c"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt:\n  name: outer\n  prompt: outer body\n",
        )
        .unwrap()
        .with_file(
            Path::new("/vault/a/b/.remargin.yaml"),
            b"system_prompt:\n  name: inner\n  prompt: inner body\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/c/file.md")).unwrap();
    assert_eq!(resolved.prompt, "inner body");
    assert_eq!(resolved.name, "inner");
    assert_eq!(
        resolved.source.as_deref(),
        Some(Path::new("/vault/a/b/.remargin.yaml"))
    );
    assert!(!resolved.is_default);
}

#[test]
fn walk_skips_configs_without_system_prompt() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a/b/c"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"identity: someone\ntype: human\n",
        )
        .unwrap()
        .with_file(
            Path::new("/vault/.remargin.yaml"),
            b"system_prompt:\n  name: vault\n  prompt: vault body\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/c/file.md")).unwrap();
    assert_eq!(resolved.prompt, "vault body");
    assert_eq!(resolved.name, "vault");
}

#[test]
fn walk_exhausts_to_default() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a/b/c"))
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/c/file.md")).unwrap();
    assert_eq!(resolved.prompt, DEFAULT_PROMPT_BODY);
    assert_eq!(resolved.name, "default");
    assert!(resolved.source.is_none());
    assert!(resolved.is_default);
}

#[test]
fn name_absent_falls_back_to_folder_basename() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/remargin"))
        .unwrap()
        .with_file(
            Path::new("/vault/remargin/.remargin.yaml"),
            b"system_prompt:\n  prompt: body\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/remargin/file.md")).unwrap();
    assert_eq!(resolved.name, "remargin");
}

#[test]
fn explicit_name_overrides_folder() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/remargin"))
        .unwrap()
        .with_file(
            Path::new("/vault/remargin/.remargin.yaml"),
            b"system_prompt:\n  name: SWE reviewer\n  prompt: body\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/remargin/file.md")).unwrap();
    assert_eq!(resolved.name, "SWE reviewer");
}

#[test]
fn directory_input_starts_walk_at_directory() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a/b"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/b/.remargin.yaml"),
            b"system_prompt:\n  name: here\n  prompt: body\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b")).unwrap();
    assert_eq!(resolved.name, "here");
}

#[test]
fn empty_prompt_returned_verbatim() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt:\n  name: empty\n  prompt: \"\"\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap();
    assert_eq!(resolved.prompt, "");
    assert!(!resolved.is_default);
}

#[test]
fn malformed_yaml_errors_with_path() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt: [oops\n",
        )
        .unwrap();

    let err = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/vault/a/.remargin.yaml"), "{chain}");
}

#[test]
fn legacy_config_without_system_prompt_continues_walk() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a/b"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/b/.remargin.yaml"),
            b"identity: foo\ntype: agent\nmode: open\n",
        )
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt:\n  name: outer\n  prompt: outer body\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/file.md")).unwrap();
    assert_eq!(resolved.name, "outer");
}

#[test]
fn vault_root_explicit_prompt_not_default() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a"))
        .unwrap()
        .with_file(
            Path::new("/vault/.remargin.yaml"),
            b"system_prompt:\n  name: vault-default\n  prompt: vault body\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap();
    assert_eq!(resolved.name, "vault-default");
    assert!(!resolved.is_default);
    assert!(resolved.source.is_some());
}

#[test]
fn runner_carried_on_hit_and_null_when_absent() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a"))
        .unwrap()
        .with_dir(Path::new("/vault/b"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt:\n  name: with-runner\n  prompt: body\n  runner: goose run -i -\n",
        )
        .unwrap()
        .with_file(
            Path::new("/vault/b/.remargin.yaml"),
            b"system_prompt:\n  name: plain\n  prompt: body\n",
        )
        .unwrap();

    let with = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap();
    assert_eq!(with.runner.as_deref(), Some("goose run -i -"));
    let without = resolve_system_prompt(&system, Path::new("/vault/b/file.md")).unwrap();
    assert_eq!(without.runner, None);
}

#[test]
fn default_fallback_has_no_runner() {
    let system = MemorySystem::new().with_dir(Path::new("/vault/a")).unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap();
    assert!(resolved.is_default);
    assert_eq!(resolved.runner, None);
}

#[test]
fn multiline_runner_errors() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt:\n  name: bad\n  prompt: body\n  runner: |\n    one\n    two\n",
        )
        .unwrap();

    let err = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("runner must be a single line"), "{chain}");
}

#[test]
fn render_resolved_prompt_includes_runner_line() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt:\n  name: with-runner\n  prompt: body\n  runner: goose run -i -\n",
        )
        .unwrap();

    let resolved = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap();
    let text = super::render_resolved_prompt(Path::new("/vault/a/file.md"), &resolved);
    assert!(text.contains("Runner:  goose run -i -"), "{text}");
}

#[test]
fn missing_prompt_field_errors() {
    let system = MemorySystem::new()
        .with_dir(Path::new("/vault/a"))
        .unwrap()
        .with_file(
            Path::new("/vault/a/.remargin.yaml"),
            b"system_prompt:\n  name: incomplete\n",
        )
        .unwrap();

    let err = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("/vault/a/.remargin.yaml"), "{chain}");
}
