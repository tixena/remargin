//! Tests for typed prompt operations and the YAML splice helpers.

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MemorySystem;

use super::{
    PromptListEntry, SystemPromptBlock, delete, find_block_range, list, remove_system_prompt, set,
    splice_system_prompt, starts_with_system_prompt_key,
};
use crate::config::{Mode, ResolvedConfig};
use crate::parser::AuthorType;

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn mkdir(system: &MemorySystem, path: &str) {
    system.create_dir_all(Path::new(path)).unwrap();
}

fn write_file(system: &MemorySystem, path: &str, content: &str) {
    if let Some(parent) = Path::new(path).parent() {
        system.create_dir_all(parent).unwrap();
    }
    system.write(Path::new(path), content.as_bytes()).unwrap();
}

fn read_file(system: &MemorySystem, path: &str) -> String {
    system.read_to_string(Path::new(path)).unwrap()
}

#[test]
fn splice_appends_to_file_without_block() {
    let existing = "identity: eduardo\ntype: human\n";
    let out = splice_system_prompt(
        existing,
        &SystemPromptBlock {
            name: "Reviewer",
            prompt: "Review code.",
            runner: "",
        },
    );
    let expected = "identity: eduardo\ntype: human\n\nsystem_prompt:\n  name: Reviewer\n  prompt: Review code.\n";
    assert_eq!(out.content, expected);
    assert!(!out.noop);
}

#[test]
fn splice_replaces_existing_block_in_middle() {
    let existing = "\
identity: eduardo
system_prompt:
  name: Old
  prompt: old
mode: open
";
    let out = splice_system_prompt(
        existing,
        &SystemPromptBlock {
            name: "New",
            prompt: "new",
            runner: "",
        },
    );
    let expected = "\
identity: eduardo
system_prompt:
  name: New
  prompt: new
mode: open
";
    assert_eq!(out.content, expected);
}

#[test]
fn splice_multiline_body_uses_block_scalar() {
    let out = splice_system_prompt(
        "",
        &SystemPromptBlock {
            name: "Multi",
            prompt: "line one\nline two\nline three",
            runner: "",
        },
    );
    let expected = "\
system_prompt:
  name: Multi
  prompt: |
    line one
    line two
    line three
";
    assert_eq!(out.content, expected);
}

#[test]
fn splice_special_chars_force_block_style() {
    let out = splice_system_prompt(
        "",
        &SystemPromptBlock {
            name: "X",
            prompt: "value with: colon and # hash",
            runner: "",
        },
    );
    assert!(out.content.contains("  prompt: |"));
    assert!(out.content.contains("    value with: colon and # hash"));
}

#[test]
fn splice_empty_body_writes_empty_quotes() {
    let out = splice_system_prompt(
        "",
        &SystemPromptBlock {
            name: "Empty",
            prompt: "",
            runner: "",
        },
    );
    assert!(out.content.contains("  prompt: \"\""));
}

#[test]
fn splice_omits_name_line_when_empty() {
    let out = splice_system_prompt(
        "",
        &SystemPromptBlock {
            name: "",
            prompt: "body",
            runner: "",
        },
    );
    assert!(!out.content.contains("  name:"));
    assert!(out.content.contains("  prompt: body"));
}

#[test]
fn splice_noop_when_identical() {
    let existing = "system_prompt:\n  name: Same\n  prompt: same\n";
    let out = splice_system_prompt(
        existing,
        &SystemPromptBlock {
            name: "Same",
            prompt: "same",
            runner: "",
        },
    );
    assert!(out.noop);
}

#[test]
fn splice_renders_runner_after_prompt() {
    let out = splice_system_prompt(
        "",
        &SystemPromptBlock {
            name: "R",
            prompt: "body",
            runner: "goose run -i -",
        },
    );
    let expected = "system_prompt:\n  name: R\n  prompt: body\n  runner: goose run -i -\n";
    assert_eq!(out.content, expected);
}

#[test]
fn splice_replacing_block_drops_stale_runner() {
    let existing = "system_prompt:\n  name: R\n  prompt: body\n  runner: goose run -i -\n";
    let out = splice_system_prompt(
        existing,
        &SystemPromptBlock {
            name: "R",
            prompt: "body",
            runner: "",
        },
    );
    assert!(!out.content.contains("runner:"));
    assert!(out.content.contains("  prompt: body"));
}

#[test]
fn remove_strips_block_keeps_other_fields() {
    let existing = "\
identity: eduardo
system_prompt:
  name: Reviewer
  prompt: Review.
mode: open
";
    let out = remove_system_prompt(existing);
    let expected = "identity: eduardo\nmode: open\n";
    assert_eq!(out.content, expected);
}

#[test]
fn remove_noop_when_block_absent() {
    let existing = "identity: eduardo\n";
    let out = remove_system_prompt(existing);
    assert!(out.noop);
    assert_eq!(out.content, existing);
}

#[test]
fn remove_trims_trailing_blank_lines() {
    let existing = "identity: eduardo\n\nsystem_prompt:\n  prompt: x\n";
    let out = remove_system_prompt(existing);
    assert!(!out.content.ends_with("\n\n"));
}

#[test]
fn key_matcher_accepts_canonical() {
    assert!(starts_with_system_prompt_key("system_prompt:"));
    assert!(starts_with_system_prompt_key("system_prompt :"));
}

#[test]
fn key_matcher_rejects_indented() {
    assert!(!starts_with_system_prompt_key("  system_prompt:"));
}

#[test]
fn key_matcher_rejects_substring() {
    assert!(!starts_with_system_prompt_key("system_prompt_foo:"));
}

#[test]
fn find_block_range_stops_at_next_top_level_key() {
    let s = "system_prompt:\n  prompt: x\nidentity: eduardo\n";
    let range = find_block_range(s).unwrap();
    let block = &s[range.start..range.end];
    assert_eq!(block, "system_prompt:\n  prompt: x\n");
}

#[test]
fn find_block_range_extends_to_eof() {
    let s = "system_prompt:\n  prompt: x";
    let range = find_block_range(s).unwrap();
    let block = &s[range.start..range.end];
    assert_eq!(block, "system_prompt:\n  prompt: x");
}

#[test]
fn set_creates_yaml_when_absent() {
    let system = MemorySystem::new();
    mkdir(&system, "/vault/foo");
    let out = set(
        &system,
        Path::new("/vault/foo"),
        Some("Reviewer"),
        "Review.",
        None,
        &open_config(),
    )
    .unwrap();
    assert!(out.created);
    assert!(!out.noop);
    let body = read_file(&system, "/vault/foo/.remargin.yaml");
    assert!(body.contains("system_prompt:"));
    assert!(body.contains("Reviewer"));
}

#[test]
fn set_preserves_identity_field() {
    let system = MemorySystem::new();
    write_file(
        &system,
        "/vault/foo/.remargin.yaml",
        "identity: eduardo\ntype: human\n",
    );
    set(
        &system,
        Path::new("/vault/foo"),
        Some("Reviewer"),
        "Review.",
        None,
        &open_config(),
    )
    .unwrap();
    let body = read_file(&system, "/vault/foo/.remargin.yaml");
    assert!(body.starts_with("identity: eduardo\n"));
    assert!(body.contains("system_prompt:"));
}

#[test]
fn set_replaces_existing_block() {
    let system = MemorySystem::new();
    write_file(
        &system,
        "/vault/foo/.remargin.yaml",
        "system_prompt:\n  name: Old\n  prompt: old\n",
    );
    set(
        &system,
        Path::new("/vault/foo"),
        Some("New"),
        "new",
        None,
        &open_config(),
    )
    .unwrap();
    let body = read_file(&system, "/vault/foo/.remargin.yaml");
    assert!(body.contains("name: New"));
    assert!(!body.contains("Old"));
}

#[test]
fn set_refuses_non_directory() {
    let system = MemorySystem::new();
    write_file(&system, "/vault/foo/a.md", "x");
    let err = set(
        &system,
        Path::new("/vault/foo/a.md"),
        Some("X"),
        "y",
        None,
        &open_config(),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not a directory") || msg.contains("does not exist"),
        "unexpected error: {msg}"
    );
}

#[test]
fn set_writes_runner_line() {
    let system = MemorySystem::new();
    mkdir(&system, "/vault/foo");
    set(
        &system,
        Path::new("/vault/foo"),
        Some("Reviewer"),
        "Review.",
        Some("goose run -i -"),
        &open_config(),
    )
    .unwrap();
    let body = read_file(&system, "/vault/foo/.remargin.yaml");
    assert!(body.contains("  runner: goose run -i -"), "{body}");
}

#[test]
fn set_without_runner_clears_existing_runner() {
    let system = MemorySystem::new();
    write_file(
        &system,
        "/vault/foo/.remargin.yaml",
        "system_prompt:\n  name: R\n  prompt: body\n  runner: goose run -i -\n",
    );
    set(
        &system,
        Path::new("/vault/foo"),
        Some("R"),
        "body",
        None,
        &open_config(),
    )
    .unwrap();
    let body = read_file(&system, "/vault/foo/.remargin.yaml");
    assert!(!body.contains("runner:"), "{body}");
}

#[test]
fn set_rejects_multiline_runner() {
    let system = MemorySystem::new();
    mkdir(&system, "/vault/foo");
    let err = set(
        &system,
        Path::new("/vault/foo"),
        Some("R"),
        "body",
        Some("line one\nline two"),
        &open_config(),
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("runner must be a single line"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn set_refuses_missing_folder() {
    let system = MemorySystem::new();
    let err = set(
        &system,
        Path::new("/missing"),
        Some("X"),
        "y",
        None,
        &open_config(),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("does not exist"), "unexpected error: {msg}");
}

#[test]
fn delete_strips_block_preserves_identity() {
    let system = MemorySystem::new();
    write_file(
        &system,
        "/vault/foo/.remargin.yaml",
        "identity: eduardo\nsystem_prompt:\n  prompt: x\n",
    );
    let out = delete(&system, Path::new("/vault/foo"), &open_config()).unwrap();
    assert!(!out.absent);
    let body = read_file(&system, "/vault/foo/.remargin.yaml");
    assert!(body.contains("identity: eduardo"));
    assert!(!body.contains("system_prompt"));
}

#[test]
fn delete_idempotent_on_missing_block() {
    let system = MemorySystem::new();
    write_file(&system, "/vault/foo/.remargin.yaml", "identity: eduardo\n");
    let out = delete(&system, Path::new("/vault/foo"), &open_config()).unwrap();
    assert!(out.absent);
}

#[test]
fn delete_idempotent_on_missing_file() {
    let system = MemorySystem::new();
    mkdir(&system, "/vault/foo");
    let out = delete(&system, Path::new("/vault/foo"), &open_config()).unwrap();
    assert!(out.absent);
}

#[test]
fn delete_leaves_empty_file_in_place() {
    let system = MemorySystem::new();
    write_file(
        &system,
        "/vault/foo/.remargin.yaml",
        "system_prompt:\n  prompt: x\n",
    );
    let out = delete(&system, Path::new("/vault/foo"), &open_config()).unwrap();
    assert!(!out.absent);
    assert!(out.left_empty);
    assert!(
        system
            .exists(Path::new("/vault/foo/.remargin.yaml"))
            .unwrap()
    );
}

#[test]
fn list_finds_declared_prompts() {
    let system = MemorySystem::new();
    write_file(
        &system,
        "/vault/a/.remargin.yaml",
        "system_prompt:\n  name: A\n  prompt: aa\n",
    );
    write_file(
        &system,
        "/vault/b/c/.remargin.yaml",
        "system_prompt:\n  prompt: cc\n",
    );
    write_file(
        &system,
        "/vault/b/identity-only/.remargin.yaml",
        "identity: eduardo\n",
    );
    let out = list(&system, Path::new("/vault")).unwrap();
    let sources: Vec<PathBuf> = out.iter().map(|e| e.source.clone()).collect();
    assert!(
        sources
            .iter()
            .any(|p| p.as_path() == Path::new("/vault/a/.remargin.yaml"))
    );
    assert!(
        sources
            .iter()
            .any(|p| p.as_path() == Path::new("/vault/b/c/.remargin.yaml"))
    );
    assert!(
        !sources
            .iter()
            .any(|p| p.as_path() == Path::new("/vault/b/identity-only/.remargin.yaml"))
    );
    let a: &PromptListEntry = out
        .iter()
        .find(|e| e.source.as_path() == Path::new("/vault/a/.remargin.yaml"))
        .unwrap();
    assert_eq!(a.name.as_deref(), Some("A"));
    assert_eq!(a.prompt, "aa");
    let c: &PromptListEntry = out
        .iter()
        .find(|e| e.source.as_path() == Path::new("/vault/b/c/.remargin.yaml"))
        .unwrap();
    assert_eq!(c.name, None);
    assert_eq!(c.prompt, "cc");
}

#[test]
fn list_carries_runner_when_declared() {
    let system = MemorySystem::new();
    write_file(
        &system,
        "/vault/a/.remargin.yaml",
        "system_prompt:\n  name: A\n  prompt: aa\n  runner: goose run -i -\n",
    );
    write_file(
        &system,
        "/vault/b/.remargin.yaml",
        "system_prompt:\n  name: B\n  prompt: bb\n",
    );
    let out = list(&system, Path::new("/vault")).unwrap();
    let a = out
        .iter()
        .find(|e| e.source.as_path() == Path::new("/vault/a/.remargin.yaml"))
        .unwrap();
    assert_eq!(a.runner.as_deref(), Some("goose run -i -"));
    let b = out
        .iter()
        .find(|e| e.source.as_path() == Path::new("/vault/b/.remargin.yaml"))
        .unwrap();
    assert_eq!(b.runner, None);
}

#[test]
fn list_empty_when_no_declarations() {
    let system = MemorySystem::new();
    mkdir(&system, "/vault/foo");
    let out = list(&system, Path::new("/vault")).unwrap();
    assert!(out.is_empty());
}

#[test]
fn diff_accepts_only_prompt_change() {
    let old_yaml = "identity: eduardo\nsystem_prompt:\n  prompt: a\n";
    let new_yaml = "identity: eduardo\nsystem_prompt:\n  prompt: b\n";
    super::diff_only_system_prompt(old_yaml, new_yaml).unwrap();
}

#[test]
fn diff_rejects_unrelated_field_change() {
    let old_yaml = "identity: eduardo\nsystem_prompt:\n  prompt: a\n";
    let new_yaml = "identity: jorge\nsystem_prompt:\n  prompt: a\n";
    let err = super::diff_only_system_prompt(old_yaml, new_yaml).unwrap_err();
    assert!(format!("{err:#}").contains("non-prompt field changed"));
}

#[test]
fn diff_treats_empty_old_as_empty_map() {
    let new_yaml = "system_prompt:\n  prompt: x\n";
    super::diff_only_system_prompt("", new_yaml).unwrap();
}
