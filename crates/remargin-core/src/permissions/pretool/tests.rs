//! Unit tests for `permissions::pretool`. Every test feeds a synthetic
//! stdin envelope through `pretool()` against a `MemorySystem` realm and
//! asserts the resulting `PretoolOutcome`. The core function is pure
//! so the binary never spawns.

use std::path::{Path, PathBuf};

use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use crate::permissions::pretool::{
    Decision, DecisionInner, PermissionDecision, PretoolOutcome, ToolPrefix, ToolTarget, decide,
    pretool,
};

fn mock_with(files: &[(&str, &str)]) -> MemorySystem {
    let mut system = MemorySystem::new();
    for (path, body) in files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    system
}

fn event_json(tool_name: &str, cwd: &str, tool_input: &Value) -> Vec<u8> {
    let envelope = json!({
        "session_id": "test",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": cwd,
        "hook_event_name": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
    });
    serde_json::to_vec(&envelope).unwrap()
}

fn restrict_yaml(path: &str) -> String {
    format!("permissions:\n  trusted_roots:\n    - path: {path}\n")
}

fn restrict_with_extra_bash(path: &str, verb: &str) -> String {
    format!("permissions:\n  trusted_roots:\n    - path: {path}\n      also_deny_bash: [{verb}]\n")
}

/// A realm whose only restriction is a `deny_ops` entry on `path` — no
/// `trusted_roots`. Exercises the shared predicate's `deny_ops` branch.
fn deny_ops_yaml(path: &str, op: &str) -> String {
    format!("permissions:\n  deny_ops:\n    - path: {path}\n      ops: [{op}]\n")
}

fn expect_deny(outcome: PretoolOutcome) -> Decision {
    assert!(
        matches!(outcome, PretoolOutcome::Deny(_)),
        "expected Deny, got {outcome:?}",
    );
    let PretoolOutcome::Deny(decision) = outcome else {
        return Decision {
            hook_specific_output: DecisionInner {
                hook_event_name: "PreToolUse",
                permission_decision: PermissionDecision::Deny,
                permission_decision_reason: String::new(),
            },
        };
    };
    decision
}

fn expect_fail(outcome: PretoolOutcome) -> String {
    assert!(
        matches!(outcome, PretoolOutcome::Fail(_)),
        "expected Fail, got {outcome:?}",
    );
    let PretoolOutcome::Fail(reason) = outcome else {
        return String::new();
    };
    reason
}

fn deny_reason(decision: &Decision) -> &str {
    decision
        .hook_specific_output
        .permission_decision_reason
        .as_str()
}

#[test]
fn read_on_unrestricted_path_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/public/foo.md" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn read_on_restricted_path_denies_with_get_message() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/secret/foo.md" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(matches!(
        decision.hook_specific_output.permission_decision,
        PermissionDecision::Deny
    ));
    assert!(deny_reason(&decision).contains("mcp__remargin__get"));
    assert!(deny_reason(&decision).contains("/r/secret/foo.md"));
}

#[test]
fn write_on_restricted_path_denies_with_write_message() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Write",
        "/r",
        &json!({ "file_path": "/r/secret/foo.md", "content": "x" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__write"));
}

#[test]
fn edit_on_restricted_path_denies_with_edit_message() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Edit",
        "/r",
        &json!({ "file_path": "/r/secret/foo.md", "old_string": "a", "new_string": "b" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__edit"));
}

/// The input field is `notebook_path`, not `file_path`.
#[test]
fn notebook_edit_on_restricted_path_denies_with_notebook_message() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "NotebookEdit",
        "/r",
        &json!({ "notebook_path": "/r/secret/foo.ipynb", "new_source": "x" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__write"));
    assert!(deny_reason(&decision).contains("notebook"));
}

/// The verb is not a gate: `echo` naming a word inside the realm denies as `cat` would.
#[test]
fn bash_verb_not_a_gate_word_into_realm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "echo \"/r/secret/foo\"" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_mutator_referencing_restricted_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/secret/foo" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("/r/secret"));
    assert!(deny_reason(&decision).contains("shell command"));
}

#[test]
fn bash_mutator_on_unrestricted_path_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/public/foo" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn bash_mutator_with_no_path_reference_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /tmp/x" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn bash_per_realm_extra_verb_triggers_check() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_extra_bash("secret", "curl"),
    )]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "curl /r/secret/upload" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// With no `path` the search root is the event cwd, an ancestor of the trusted root, so it
/// denies.
#[test]
fn glob_no_path_resolves_cwd_ancestor_of_root_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Glob", "/r", &json!({ "pattern": "**/*.md" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__ls"));
}

#[test]
fn unknown_tool_name_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("FooBar", "/r", &json!({ "anything": 1_i32 }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn multi_edit_on_restricted_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "MultiEdit",
        "/r",
        &json!({ "file_path": "/r/secret/foo.md", "edits": [] }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("remargin-managed"));
    assert!(deny_reason(&decision).contains("/r/secret/foo.md"));
}

#[test]
fn grep_on_restricted_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Grep",
        "/r",
        &json!({ "pattern": "foo", "path": "/r/secret" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn glob_on_restricted_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Glob",
        "/r",
        &json!({ "pattern": "**/*.md", "path": "/r/secret" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// The missing optional `path` yields a decision, never a `Fail`.
#[test]
fn grep_no_path_resolves_cwd_ancestor_of_root_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Grep", "/r", &json!({ "pattern": "foo" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__search"));
}

/// The cwd fallback takes part in restriction, not only in fail-open.
#[test]
fn grep_no_path_resolves_cwd_inside_wildcard_realm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Grep", "/r/sub", &json!({ "pattern": "foo" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn web_fetch_tool_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "WebFetch",
        "/r",
        &json!({ "url": "https://example.com", "prompt": "x" }),
    );
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Scope is resolved from the target, so a cwd outside every realm does not open the path.
#[test]
fn cwd_outside_realm_absolute_target_inside_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Read",
        "/home/x",
        &json!({ "file_path": "/r/secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// The realm above the target governs; the cwd's realm never enters the walk.
#[test]
fn target_in_other_realm_uses_that_realms_config() {
    let system = mock_with(&[
        ("/r1/.remargin.yaml", &restrict_yaml("apub")),
        ("/r2/.remargin.yaml", &restrict_yaml("secret")),
    ]);
    let stdin = event_json(
        "Read",
        "/r1/sub",
        &json!({ "file_path": "/r2/secret/a.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn no_realm_above_target_silent_allows() {
    let system = MemorySystem::new();
    let stdin = event_json("Read", "/anywhere", &json!({ "file_path": "/tmp/a.md" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// Only the inner realm declares the root, so the deny proves the nearest realm governs.
#[test]
fn nested_realms_nearest_above_target_governs() {
    let system = mock_with(&[
        ("/r/.remargin.yaml", &restrict_yaml("outer")),
        ("/r/inner/.remargin.yaml", &restrict_yaml("sec")),
    ]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/inner/sec/a.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// A relative target is rooted at the cwd, then scope is resolved from the absolute path.
#[test]
fn relative_target_rooted_at_cwd_then_resolved() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))])
        .with_dir(Path::new("/r/sub"))
        .unwrap();
    let stdin = event_json("Read", "/r/sub", &json!({ "file_path": "../secret/a.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn malformed_stdin_fails() {
    let system = MemorySystem::new();
    let reason = expect_fail(pretool(&system, b"not json"));
    assert!(reason.contains("malformed PreToolUse event"));
}

/// A failed resolution surfaces as `Fail`, not as a silent allow.
#[test]
fn out_of_realm_trusted_root_fails_loud() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("/other/secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/foo.md" }));
    let reason = expect_fail(pretool(&system, &stdin));
    assert!(reason.contains("permissions resolve failed"), "{reason}");
    assert!(reason.contains("/other/secret"), "{reason}");
}

#[test]
fn missing_tool_name_fails() {
    let system = MemorySystem::new();
    let reason = expect_fail(pretool(&system, b"{}"));
    assert!(reason.contains("missing field"));
}

#[test]
fn read_missing_file_path_fails() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({}));
    let reason = expect_fail(pretool(&system, &stdin));
    assert!(reason.contains("missing tool_input.file_path"));
}

#[test]
fn relative_file_path_resolves_against_event_cwd() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))])
        .with_dir(Path::new("/r/sub"))
        .unwrap();
    let stdin = event_json(
        "Read",
        "/r/sub",
        &json!({ "file_path": "../secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// The realm path never appears verbatim: only tracking `cd` resolves the bare `rm foo` into it.
#[test]
fn bash_cd_reconstructed_target_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "cd secret && rm foo" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn wildcard_restrict_denies_anywhere_in_realm() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/anything.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn identical_input_yields_identical_outcome() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/secret/foo.md" }));
    let first = pretool(&system, &stdin);
    let second = pretool(&system, &stdin);
    assert_eq!(first, second);
}

#[test]
fn bash_verb_extractor_skips_env_var_prefix() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "FOO=bar  rm /r/secret/x" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_bare_mutator_on_restricted_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "sed /r/secret/foo.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_rtk_wrapped_mutator_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "rtk sed /r/secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_rtk_proxy_wrapped_mutator_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "rtk proxy sed /r/secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_rtk_git_status_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rtk git status" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn bash_rtk_ls_non_mutator_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rtk ls /tmp" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn bash_rtk_ls_restricted_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rtk ls /r/secret/" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_env_prefix_then_rtk_wrapper_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "FOO=bar rtk sed /r/secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_rtk_alone_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rtk" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn bash_rtk_proxy_alone_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rtk proxy" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn bash_rtk_rtk_degenerate_nesting_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "rtk rtk sed /r/secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_rtk_gain_meta_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rtk gain" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

#[test]
fn bash_rtk_wrapped_with_per_realm_extra_denies() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_extra_bash("secret", "sed"),
    )]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "rtk sed /r/secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_bare_proxy_still_denies_on_path() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "proxy sed /r/secret/foo.md" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

fn assert_bash_deny_contains(command: &str, needles: &[&str]) {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": command }));
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    for needle in needles {
        assert!(
            reason.contains(needle),
            "expected `{needle}` in deny reason for `{command}`, got: {reason}",
        );
    }
}

fn assert_bash_deny_lacks(command: &str, needle: &str) {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": command }));
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    assert!(
        !reason.contains(needle),
        "expected `{needle}` NOT in deny reason for `{command}`, got: {reason}",
    );
}

#[test]
fn bash_sed_verb_guidance() {
    assert_bash_deny_contains(
        "sed /r/secret/foo.md",
        &["mcp__remargin__get", "mcp__remargin__write"],
    );
    assert_bash_deny_lacks("sed /r/secret/foo.md", "no direct shell substitute");
}

#[test]
fn bash_awk_verb_guidance() {
    assert_bash_deny_contains("awk '{print}' /r/secret/foo.md", &["mcp__remargin__get"]);
}

fn assert_bash_deny_with_extra_contains(command: &str, extra_verb: &str, needles: &[&str]) {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_extra_bash("secret", extra_verb),
    )]);
    let stdin = event_json("Bash", "/r", &json!({ "command": command }));
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    for needle in needles {
        assert!(
            reason.contains(needle),
            "expected `{needle}` in deny reason for `{command}`, got: {reason}",
        );
    }
}

#[test]
fn bash_cat_verb_guidance() {
    assert_bash_deny_with_extra_contains("cat /r/secret/foo.md", "cat", &["mcp__remargin__get"]);
}

#[test]
fn bash_head_verb_guidance() {
    assert_bash_deny_with_extra_contains(
        "head /r/secret/foo.md",
        "head",
        &["mcp__remargin__get", "start_line"],
    );
}

#[test]
fn bash_tail_verb_guidance() {
    assert_bash_deny_with_extra_contains("tail /r/secret/foo.md", "tail", &["mcp__remargin__get"]);
}

#[test]
fn bash_grep_verb_guidance() {
    assert_bash_deny_with_extra_contains(
        "grep foo /r/secret/foo.md",
        "grep",
        &["mcp__remargin__search"],
    );
}

#[test]
fn bash_find_verb_guidance() {
    assert_bash_deny_contains("find /r/secret/", &["mcp__remargin__query"]);
}

#[test]
fn bash_mv_verb_guidance() {
    assert_bash_deny_contains("mv /r/secret/foo.md /tmp/x", &["mcp__remargin__mv"]);
}

#[test]
fn bash_rm_verb_guidance() {
    assert_bash_deny_contains(
        "rm /r/secret/foo.md",
        &["mcp__remargin__rm", "mcp__remargin__purge"],
    );
}

#[test]
fn bash_cp_verb_guidance() {
    assert_bash_deny_contains("cp /r/secret/foo.md /tmp/x", &["mcp__remargin__cp"]);
}

#[test]
fn bash_tee_verb_guidance() {
    assert_bash_deny_contains("tee /r/secret/foo.md", &["mcp__remargin__write"]);
}

#[test]
fn bash_vim_verb_guidance() {
    assert_bash_deny_contains(
        "vim /r/secret/foo.md",
        &["mcp__remargin__write", "mcp__remargin__edit"],
    );
}

#[test]
fn bash_git_verb_guidance() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_extra_bash("secret", "git"),
    )]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "git add /r/secret/foo.md" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    assert!(reason.contains("mcp__remargin__"));
    assert!(reason.contains("git"));
    assert!(reason.contains("human's job"));
}

#[test]
fn bash_unknown_mutator_falls_back_to_no_equivalent_message() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_extra_bash("secret", "weirdtool"),
    )]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "weirdtool /r/secret/foo.md" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    assert!(reason.contains("/r"), "reason: {reason}");
    assert!(reason.contains("outside"), "reason: {reason}");
    assert!(!reason.contains("unrestrict"), "reason: {reason}");
}

#[test]
fn bash_rtk_wrapped_sed_shows_sed_guidance() {
    assert_bash_deny_contains(
        "rtk sed /r/secret/foo.md",
        &["mcp__remargin__get", "mcp__remargin__write"],
    );
}

fn cli_deny_yaml() -> &'static str {
    "permissions:\n  cli_allowed: false\n"
}

fn cli_allow_yaml() -> &'static str {
    "permissions:\n  cli_allowed: true\n"
}

#[test]
fn bash_cli_denied_blocks_remargin_verb() {
    let system = mock_with(&[("/r/.remargin.yaml", cli_deny_yaml())]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "remargin write x" }));
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    assert!(reason.contains("cli_allowed: false"), "reason: {reason}");
    assert!(reason.contains("mcp__remargin__"), "reason: {reason}");
}

#[test]
fn bash_cli_allowed_permits_remargin_verb() {
    let system = mock_with(&[("/r/.remargin.yaml", cli_allow_yaml())]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "remargin write x" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// The default deny carries the opt-in hint, not the explicit-`false` message.
#[test]
fn bash_cli_default_deny_blocks_remargin_verb() {
    let system = mock_with(&[]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "remargin ls" }));
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    assert!(reason.contains("cli_allowed: true"), "reason: {reason}");
    assert!(!reason.contains("cli_allowed: false"), "reason: {reason}");
}

#[test]
fn bash_cli_denied_with_env_prefix_and_rtk_proxy_wrapper() {
    let system = mock_with(&[("/r/.remargin.yaml", cli_deny_yaml())]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "FOO=bar rtk proxy remargin ls" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("cli_allowed: false"));
}

#[test]
fn bash_cli_denied_non_remargin_verb_unaffected() {
    let system = mock_with(&[("/r/.remargin.yaml", cli_deny_yaml())]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "ls /r" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_cli_denied_bare_remargin_no_args() {
    let system = mock_with(&[("/r/.remargin.yaml", cli_deny_yaml())]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "remargin" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("cli_allowed: false"));
}

#[test]
fn bash_cli_denied_child_policy_applies_to_cwd_in_child() {
    let system = mock_with(&[("/r/sub/.remargin.yaml", cli_deny_yaml())])
        .with_dir(Path::new("/r/sub"))
        .unwrap();
    let stdin = event_json("Bash", "/r/sub", &json!({ "command": "remargin write x" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("cli_allowed: false"));
}

/// The child's deny is not in the parent cwd's walk, so the parent's allow governs.
#[test]
fn bash_cli_denied_child_policy_does_not_affect_parent_cwd() {
    let system = mock_with(&[
        ("/r/.remargin.yaml", cli_allow_yaml()),
        ("/r/sub/.remargin.yaml", cli_deny_yaml()),
    ])
    .with_dir(Path::new("/r/sub"))
    .unwrap();
    let stdin = event_json("Bash", "/r", &json!({ "command": "remargin write x" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

fn assert_realm_bash_denies(command: &str) {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": command }));
    assert!(
        matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)),
        "`{command}` must deny",
    );
}

/// `rm` runs after a non-mutator verb in a `&&` chain.
#[test]
fn regression_logical_and_chained_mutator_denies() {
    assert_realm_bash_denies("ls && rm /r/secret/x");
}

/// `tee` writes the realm path after a pipe.
#[test]
fn regression_pipe_into_tee_denies() {
    assert_realm_bash_denies("echo hi | tee /r/secret/x");
}

/// A subshell does not hide the verb.
#[test]
fn regression_subshell_cd_then_rm_denies() {
    assert_realm_bash_denies("(cd /r/secret && rm x)");
}

/// `cat` reads the realm path; read verbs deny too.
#[test]
fn regression_cat_read_of_realm_path_denies() {
    assert_realm_bash_denies("cat /r/secret/secret");
}

/// The shell strips the quotes, rejoining `/r/secret/foo`.
#[test]
fn regression_quoted_realm_prefix_denies() {
    assert_realm_bash_denies("rm \"/r/\"secret/foo");
}

/// The glob would expand into the realm; coverage is glob-aware.
#[test]
fn regression_glob_realm_segment_denies() {
    assert_realm_bash_denies("rm /r/sec*ret/foo");
}

/// Real filesystem, because `MemorySystem` does not model symlinks.
#[cfg(unix)]
#[test]
fn regression_symlink_into_realm_via_bash_denies() {
    use std::fs;
    use std::os::unix::fs::symlink;

    use os_shim::real::RealSystem;
    use tempfile::TempDir;

    let realm = TempDir::new().unwrap();
    let realm_path = realm.path().canonicalize().unwrap();
    fs::create_dir_all(realm_path.join("src/secret")).unwrap();
    fs::write(realm_path.join("src/secret/foo"), "x").unwrap();
    fs::write(
        realm_path.join(".remargin.yaml"),
        "permissions:\n  trusted_roots:\n    - path: src/secret\n",
    )
    .unwrap();
    symlink(realm_path.join("src/secret"), realm_path.join("alias")).unwrap();

    let cwd = realm_path.display().to_string();
    let command = format!("rm {cwd}/alias/foo");
    let stdin = event_json("Bash", &cwd, &json!({ "command": command }));
    assert!(matches!(
        pretool(&RealSystem::new(), &stdin),
        PretoolOutcome::Deny(_)
    ));
}

/// A link to a link to a realm target resolves through every hop.
#[cfg(unix)]
#[test]
fn regression_symlink_chain_into_realm_via_bash_denies() {
    use std::fs;
    use std::os::unix::fs::symlink;

    use os_shim::real::RealSystem;
    use tempfile::TempDir;

    let realm = TempDir::new().unwrap();
    let realm_path = realm.path().canonicalize().unwrap();
    fs::create_dir_all(realm_path.join("src/secret")).unwrap();
    fs::write(realm_path.join("src/secret/foo"), "x").unwrap();
    fs::write(
        realm_path.join(".remargin.yaml"),
        "permissions:\n  trusted_roots:\n    - path: src/secret\n",
    )
    .unwrap();
    symlink(realm_path.join("src/secret"), realm_path.join("hop2")).unwrap();
    symlink(realm_path.join("hop2"), realm_path.join("hop1")).unwrap();

    let cwd = realm_path.display().to_string();
    let command = format!("cat {cwd}/hop1/foo");
    let stdin = event_json("Bash", &cwd, &json!({ "command": command }));
    assert!(matches!(
        pretool(&RealSystem::new(), &stdin),
        PretoolOutcome::Deny(_)
    ));
}

/// Only following the link out of the realm avoids a false deny on the link's own path.
#[cfg(unix)]
#[test]
fn regression_symlink_outside_realm_via_bash_silent_allows() {
    use std::fs;
    use std::os::unix::fs::symlink;

    use os_shim::real::RealSystem;
    use tempfile::TempDir;

    let realm = TempDir::new().unwrap();
    let realm_path = realm.path().canonicalize().unwrap();
    let outside = TempDir::new().unwrap();
    let outside_path = outside.path().canonicalize().unwrap();
    fs::create_dir_all(outside_path.join("target")).unwrap();
    fs::write(outside_path.join("target/foo"), "x").unwrap();
    fs::write(
        realm_path.join(".remargin.yaml"),
        "permissions:\n  trusted_roots:\n    - path: '*'\n",
    )
    .unwrap();
    symlink(outside_path.join("target"), realm_path.join("alias")).unwrap();

    // cwd sits outside the realm so the bare verb word cannot itself
    // resolve under the wildcard root; only the symlinked argument matters.
    let cwd = outside_path.display().to_string();
    let realm_str = realm_path.display().to_string();
    let command = format!("rm {realm_str}/alias/foo");
    let stdin = event_json("Bash", &cwd, &json!({ "command": command }));
    assert_eq!(
        pretool(&RealSystem::new(), &stdin),
        PretoolOutcome::SilentAllow
    );
}

/// `~` expands through the `HOME` env seam, so no real filesystem is needed.
#[test]
fn bash_tilde_word_expanding_into_realm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))])
        .with_env("HOME", "/r")
        .unwrap();
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "sed -i s/a/b/ ~/secret/x" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// Reads into the realm deny too, with the search-op guidance.
#[test]
fn plan_grep_read_denies_with_search_guidance() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "grep -r foo /r/secret" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__search"));
}

#[test]
fn plan_logical_and_chain_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "ls && rm /r/secret/x" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn plan_pipe_into_tee_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "echo hi | tee /r/secret/x" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn plan_subshell_cd_rm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "(cd /r/secret && rm x)" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn plan_cd_then_bare_rm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "cd /r/secret && rm x" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn plan_quoted_prefix_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm \"/r/\"secret/x" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn plan_no_realm_path_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/tmp", &json!({ "command": "cat /tmp/x" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn plan_python_c_literal_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "python -c \"open('/r/secret/f','w')\"" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn plan_cli_denied_remargin_verb_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", cli_deny_yaml())]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "remargin get x" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("cli_allowed: false"));
}

#[test]
fn plan_two_realm_pipeline_denies() {
    let system = mock_with(&[
        ("/r1/.remargin.yaml", &restrict_yaml("'*'")),
        ("/r2/.remargin.yaml", &restrict_yaml("'*'")),
    ]);
    let stdin = event_json("Bash", "/", &json!({ "command": "cat /r1/a | tee /r2/b" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn deny_ops_only_path_edit_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &deny_ops_yaml("x.md", "edit"))]);
    let stdin = event_json(
        "Edit",
        "/r",
        &json!({ "file_path": "/r/x.md", "old_string": "a", "new_string": "b" }),
    );
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// The Bash branch shares the `deny_ops` check with the Path branch.
#[test]
fn deny_ops_only_path_bash_rm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &deny_ops_yaml("x.md", "edit"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/x.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn wildcard_trusted_roots_bash_rm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "rm /r/x.md" }));
    expect_deny(pretool(&system, &stdin));
}

#[test]
fn path_in_neither_bash_rm_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &deny_ops_yaml("x.md", "edit"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/y.md" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// A wildcard realm re-allowing the named dot folders. Empty
/// `allow_dot_folders` (`""`) emits `allow_dot_folders: []`.
fn restrict_with_dot_folders(root: &str, allow_dot_folders: &str) -> String {
    format!(
        "permissions:\n  trusted_roots:\n    - path: {root}\n  allow_dot_folders: [{allow_dot_folders}]\n"
    )
}

#[test]
fn allow_dot_folders_allowed_dot_folder_read_silent_allows() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_dot_folders("'*'", "'.obsidian'"),
    )]);
    let stdin = event_json(
        "Read",
        "/r",
        &json!({ "file_path": "/r/.obsidian/workspace.json" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn allow_dot_folders_unlisted_dot_folder_read_denies() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_dot_folders("'*'", "'.obsidian'"),
    )]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/.git/config" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn allow_dot_folders_empty_list_dot_folder_read_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_with_dot_folders("'*'", ""))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/.obsidian/x" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn allow_dot_folders_non_dot_path_still_denies() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_dot_folders("'*'", "'.obsidian'"),
    )]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/notes/a.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// The cwd sits outside the realm, so only the argument resolves into it, not the bare verb.
#[test]
fn allow_dot_folders_allowed_dot_folder_bash_silent_allows() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_dot_folders("'*'", "'.obsidian'"),
    )]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "rm /r/.obsidian/x" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// The hook restricts a dot-folder path exactly when `hook_covered_rules` emits no re-allow
/// for it.
#[test]
fn allow_dot_folders_hook_matches_hook_covered_reallow() {
    use std::path::PathBuf;

    use crate::config::permissions::resolve::{ResolvedTrustedRoot, TrustedRootPath};
    use crate::permissions::claude_sync::hook_covered_rules;

    let allow = [String::from(".obsidian")];
    let entry = ResolvedTrustedRoot {
        also_deny_bash: Vec::new(),
        cli_allowed: true,
        path: TrustedRootPath::Wildcard {
            realm_root: PathBuf::from("/r"),
        },
        source_file: PathBuf::from("/r/.remargin.yaml"),
    };
    let rules = hook_covered_rules(&entry, Path::new("/r"), &allow);

    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_dot_folders("'*'", "'.obsidian'"),
    )]);

    for path in [
        "/r/.obsidian/workspace.json",
        "/r/.git/config",
        "/r/.cache/blob",
    ] {
        let folder = path.strip_prefix("/r/").unwrap().split('/').next().unwrap();
        let reallowed = rules.allow.contains(&format!("Read(/r/{folder}/**)"));

        let stdin = event_json("Read", "/r", &json!({ "file_path": path }));
        let hook_restricted = matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_));

        assert_eq!(
            hook_restricted, !reallowed,
            "hook and hook_covered_rules disagree on {path}: restricted={hook_restricted}, reallowed={reallowed}",
        );
    }
}

#[test]
fn bash_wildcard_bare_verb_ls_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "ls" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_wildcard_bare_git_status_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "git status" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_wildcard_cargo_build_release_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "cargo build --release" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_wildcard_real_path_argument_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "rm /r/x.md" }));
    expect_deny(pretool(&system, &stdin));
}

/// After a tracked `cd` a bare argument resolves to a real managed path.
#[test]
fn bash_wildcard_cd_then_bare_rm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "cd /r && rm foo" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn msg_cat_carries_get_with_path() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "cat /r/secret/x.md" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(
        deny_reason(&decision).contains("mcp__remargin__get path=/r/secret/x.md"),
        "reason: {}",
        deny_reason(&decision),
    );
}

#[test]
fn msg_grep_carries_search_with_pattern_and_path() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "grep -r foo /r/secret" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(
        deny_reason(&decision).contains("mcp__remargin__search pattern=foo path=/r/secret"),
        "reason: {}",
        deny_reason(&decision),
    );
}

#[test]
fn msg_glob_tool_carries_ls_with_path() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Glob",
        "/r",
        &json!({ "pattern": "**/*.md", "path": "/r/secret" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(
        deny_reason(&decision).contains("mcp__remargin__ls path=/r/secret"),
        "reason: {}",
        deny_reason(&decision),
    );
}

#[test]
fn msg_rm_carries_rm_with_path() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/secret/x.md" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(
        deny_reason(&decision).contains("mcp__remargin__rm path=/r/secret/x.md"),
        "reason: {}",
        deny_reason(&decision),
    );
}

#[test]
fn msg_edit_tool_carries_edit_with_path() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Edit",
        "/r",
        &json!({ "file_path": "/r/secret/x.md", "old_string": "a", "new_string": "b" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(
        deny_reason(&decision).contains("mcp__remargin__edit path=/r/secret/x.md"),
        "reason: {}",
        deny_reason(&decision),
    );
}

#[test]
fn msg_no_equivalent_names_realm_and_stays_outside() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "cargo build /r/out.md" }));
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    assert!(reason.contains("/r"), "reason: {reason}");
    assert!(reason.contains("outside"), "reason: {reason}");
    assert!(!reason.contains("unrestrict"), "reason: {reason}");
}

#[test]
fn msg_no_denial_is_bare() {
    for command in [
        "rm /r/secret/x.md",
        "cat /r/secret/x.md",
        "cargo build /r/secret/out",
        "mv /r/secret/a.md /tmp/b",
    ] {
        let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
        let stdin = event_json("Bash", "/r", &json!({ "command": command }));
        let decision = expect_deny(pretool(&system, &stdin));
        let reason = deny_reason(&decision);
        assert!(!reason.is_empty(), "empty reason for `{command}`");
        assert!(
            reason.contains("mcp__remargin__") || reason.contains("realm"),
            "reason for `{command}` names neither an op nor the realm rule: {reason}",
        );
    }
}

/// The `mv` deny carries both source and destination arguments.
#[test]
fn msg_mv_carries_src_and_dst() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "mv /r/secret/a.md /tmp/b" }),
    );
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(
        deny_reason(&decision).contains("mcp__remargin__mv src=/r/secret/a.md dst=/tmp/b"),
        "reason: {}",
        deny_reason(&decision),
    );
}

/// A realm locked to an empty allow-set (`trusted_roots: []`).
fn locked_empty_yaml() -> &'static str {
    "permissions:\n  trusted_roots: []\n"
}

#[test]
fn locked_empty_realm_read_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", locked_empty_yaml())]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/foo.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn locked_empty_realm_bash_rm_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", locked_empty_yaml())]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/foo.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// The deepest existing ancestor is canonicalized, so a symlinked prefix resolves into the realm.
#[cfg(unix)]
#[test]
fn canonicalize_prefix_symlink_new_file_write_denies() {
    use std::fs;
    use std::os::unix::fs::symlink;

    use os_shim::real::RealSystem;
    use tempfile::TempDir;

    let realm = TempDir::new().unwrap();
    let realm_path = realm.path().canonicalize().unwrap();
    fs::create_dir_all(realm_path.join("src/secret")).unwrap();
    fs::write(
        realm_path.join(".remargin.yaml"),
        "permissions:\n  trusted_roots:\n    - path: src/secret\n",
    )
    .unwrap();
    symlink(realm_path.join("src/secret"), realm_path.join("alias")).unwrap();

    let cwd = realm_path.display().to_string();
    let target = realm_path.join("alias/new.md").display().to_string();
    let stdin = event_json(
        "Write",
        &cwd,
        &json!({ "file_path": target, "content": "x" }),
    );
    assert!(matches!(
        pretool(&RealSystem::new(), &stdin),
        PretoolOutcome::Deny(_)
    ));
}

/// With no governing realm a canonicalize failure stays an allow.
#[cfg(unix)]
#[test]
fn canonicalize_fail_no_realm_silent_allows() {
    use os_shim::real::RealSystem;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let cwd = dir_path.display().to_string();
    let target = dir_path.join("does/not/exist.md").display().to_string();
    let stdin = event_json(
        "Write",
        &cwd,
        &json!({ "file_path": target, "content": "x" }),
    );
    assert_eq!(
        pretool(&RealSystem::new(), &stdin),
        PretoolOutcome::SilentAllow
    );
}

#[test]
fn malformed_event_json_fails() {
    let system = MemorySystem::new();
    let reason = expect_fail(pretool(&system, b"{ not valid json"));
    assert!(
        reason.contains("malformed PreToolUse event"),
        "reason: {reason}"
    );
}

#[test]
fn normal_realm_resolvable_path_unchanged() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);

    let restricted = event_json("Read", "/r", &json!({ "file_path": "/r/secret/a.md" }));
    assert!(matches!(
        pretool(&system, &restricted),
        PretoolOutcome::Deny(_)
    ));

    let allowed = event_json("Read", "/r", &json!({ "file_path": "/r/public/a.md" }));
    assert_eq!(pretool(&system, &allowed), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_rm_ancestor_of_root_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// The recursive-force flags do not change the analysis.
#[test]
fn bash_rm_rf_ancestor_of_root_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm -rf /r" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_ls_ancestor_of_root_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "ls /r" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_cat_ancestor_of_root_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "cat /r" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// A sibling of the trusted root is not an ancestor of it.
#[test]
fn bash_rm_sibling_of_root_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/other" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_mv_ancestor_source_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "mv /r /tmp/x" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// Every path word is checked, so an ancestor destination denies too.
#[test]
fn bash_mv_ancestor_destination_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "mv /tmp/x /r" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// A redirect write is destructive even though `echo` reads nothing.
#[test]
fn bash_redirect_write_to_ancestor_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "echo x > /r" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// A glued redirect (`>>/r`) is detected the same way → `Deny`.
#[test]
fn bash_glued_append_redirect_to_ancestor_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "echo x >>/r" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// Reading the ancestor and redirecting outside the realm writes nothing into it.
#[test]
fn bash_read_ancestor_redirect_outside_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "cat /r > /tmp/x" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// The message never offers a single-file redirect for a directory that is not a managed file.
#[test]
fn bash_ancestor_deny_message_names_realm_no_false_redirect() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r" }));
    let decision = expect_deny(pretool(&system, &stdin));
    let reason = deny_reason(&decision);
    assert!(reason.contains("/r"), "reason: {reason}");
    assert!(reason.contains("managed subtree"), "reason: {reason}");
    assert!(reason.contains("outside"), "reason: {reason}");
    assert!(
        !reason.contains("mcp__remargin__rm path="),
        "reason offers a false single-file redirect: {reason}",
    );
}

/// A glob below the realm root still resolves the realm; a glob at the root level is out of
/// scope.
#[test]
fn bash_glob_below_root_ancestor_word_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("a/secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm -rf /r/a*" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn grep_ancestor_path_denies_with_search() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Grep", "/r", &json!({ "pattern": "foo", "path": "/r" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__search"));
}

#[test]
fn glob_ancestor_path_denies_with_ls() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Glob", "/r", &json!({ "pattern": "**/*.md", "path": "/r" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("mcp__remargin__ls"));
}

#[test]
fn grep_sibling_path_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Grep",
        "/r",
        &json!({ "pattern": "foo", "path": "/r/other" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// `Read` touches only the named path, so the ancestor rule does not apply.
#[test]
fn read_ancestor_realm_file_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/file.md" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn bash_rm_wildcard_realm_root_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "rm /r" }));
    expect_deny(pretool(&system, &stdin));
}

#[test]
fn bash_ls_wildcard_realm_root_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "ls /r" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn grep_wildcard_realm_root_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Grep", "/r", &json!({ "pattern": "foo", "path": "/r" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn bash_rm_wildcard_subpath_still_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "rm /r/x.md" }));
    expect_deny(pretool(&system, &stdin));
}

/// A candidate above the realm root is undetectable: the realm's config lives below it.
#[test]
fn bash_rm_above_realm_root_undetected_silent_allows() {
    let system = mock_with(&[("/parent/realm/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/x", &json!({ "command": "rm -rf /parent" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// A realm restricting `path` whose folder policy also denies the CLI.
fn restrict_with_cli_denied(path: &str) -> String {
    format!("permissions:\n  cli_allowed: false\n  trusted_roots:\n    - path: {path}\n")
}

/// A realm restricting `path` whose folder policy opts the CLI back in.
fn restrict_with_cli_allowed(path: &str) -> String {
    format!("permissions:\n  cli_allowed: true\n  trusted_roots:\n    - path: {path}\n")
}

#[test]
fn in_realm_cwd_bare_relative_read_is_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r/secret",
        &json!({ "command": "grep pattern idea.md" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn in_realm_cwd_path_evidenced_read_still_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r/secret", &json!({ "command": "cat ./idea.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

#[test]
fn in_realm_cwd_pathless_walker_is_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r/secret", &json!({ "command": "rg pattern" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// Bypasses comment preservation — the sharpest edge of the accepted gap.
#[test]
fn in_realm_cwd_bare_relative_write_is_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r/secret",
        &json!({ "command": "sed -i s,x,y, idea.md" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn in_realm_cwd_bare_git_is_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r/secret",
        &json!({ "command": "git commit -m msg" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

/// From an in-realm cwd the CLI still answers to the folder-level `cli_allowed` policy.
#[test]
fn in_realm_cwd_remargin_cli_stays_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_with_cli_allowed("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r/secret",
        &json!({ "command": "remargin comments idea.md" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);

    let denied = mock_with(&[("/r/.remargin.yaml", &restrict_with_cli_denied("secret"))]);
    let denied_stdin = event_json(
        "Bash",
        "/r/secret",
        &json!({ "command": "remargin comments idea.md" }),
    );
    let decision = expect_deny(pretool(&denied, &denied_stdin));
    assert!(deny_reason(&decision).contains("cli_allowed: false"));
}

#[test]
fn in_realm_cwd_compound_with_remargin_prefix_is_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_with_cli_allowed("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r/secret",
        &json!({ "command": "remargin ls . && grep x idea.md" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn in_realm_cwd_at_wildcard_realm_root_bare_read_is_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "grep pattern idea.md" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn in_realm_cwd_at_wildcard_realm_root_pathless_walker_is_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rg pattern" }));
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);
}

#[test]
fn in_realm_cwd_at_wildcard_realm_root_remargin_cli_stays_allowed() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_with_cli_allowed("'*'"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "remargin comments idea.md" }),
    );
    assert_eq!(pretool(&system, &stdin), PretoolOutcome::SilentAllow);

    let denied = mock_with(&[("/r/.remargin.yaml", &restrict_with_cli_denied("'*'"))]);
    let denied_stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "remargin comments idea.md" }),
    );
    let decision = expect_deny(pretool(&denied, &denied_stdin));
    assert!(deny_reason(&decision).contains("cli_allowed: false"));
}

#[test]
fn in_realm_cwd_regression_guards_unchanged() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);

    let bare_stdin = event_json("Bash", "/r", &json!({ "command": "grep pattern idea.md" }));
    assert_eq!(pretool(&system, &bare_stdin), PretoolOutcome::SilentAllow);

    let explicit_stdin = event_json(
        "Bash",
        "/tmp",
        &json!({ "command": "cat /r/secret/foo.md" }),
    );
    assert!(matches!(
        pretool(&system, &explicit_stdin),
        PretoolOutcome::Deny(_)
    ));

    let cd_stdin = event_json(
        "Bash",
        "/x",
        &json!({ "command": "cd /r/secret && grep x idea.md" }),
    );
    assert!(matches!(
        pretool(&system, &cd_stdin),
        PretoolOutcome::Deny(_)
    ));
}

fn reason_under(
    system: &MemorySystem,
    target: &ToolTarget,
    cwd: &str,
    prefix: ToolPrefix,
) -> String {
    let decision = expect_deny(decide(system, target, Path::new(cwd), prefix));
    deny_reason(&decision).to_owned()
}

/// Renders one deny under both hosts and pins the only sanctioned
/// difference: substituting the tool prefix in the Claude render must
/// reproduce the goose render exactly. Wording that drifted per host, or a
/// message that hard-coded a prefix instead of taking the parameter, fails
/// here.
fn assert_hosts_differ_only_by_prefix(system: &MemorySystem, target: &ToolTarget, cwd: &str) {
    let claude = reason_under(system, target, cwd, ToolPrefix::CLAUDE);
    let goose = reason_under(system, target, cwd, ToolPrefix::GOOSE);
    assert_eq!(
        claude.replace(ToolPrefix::CLAUDE.as_str(), ToolPrefix::GOOSE.as_str()),
        goose,
        "host renders diverge beyond the prefix\nclaude: {claude}\ngoose:  {goose}",
    );
    assert!(
        !goose.contains(ToolPrefix::CLAUDE.as_str()),
        "goose render carries Claude Code's tool prefix: {goose}",
    );
}

fn path_target(path: &str, tool_name: &str) -> ToolTarget {
    ToolTarget::Path {
        path: PathBuf::from(path),
        tool_name: String::from(tool_name),
    }
}

fn bash_target(command: &str) -> ToolTarget {
    ToolTarget::BashCommand {
        command: String::from(command),
    }
}

/// Every per-tool deny message renders per host off the one registry.
#[test]
fn per_tool_messages_render_per_host() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    for tool in [
        "Read",
        "Write",
        "Edit",
        "MultiEdit",
        "NotebookEdit",
        "Grep",
        "Glob",
        "SomeUnknownTool",
    ] {
        assert_hosts_differ_only_by_prefix(&system, &path_target("/r/secret/foo.md", tool), "/r");
    }
}

/// Includes a verb outside the vocabulary, so the no-equivalent fallback is covered.
#[test]
fn shell_verb_guidance_renders_per_host() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    for verb in [
        "sed", "awk", "cat", "less", "more", "head", "tail", "grep", "rg", "ag", "find", "ls",
        "mv", "rm", "cp", "tee", "dd", "vim", "nvim", "nano", "code", "git", "make",
    ] {
        let target = bash_target(&format!("{verb} /r/secret/foo.md"));
        assert_hosts_differ_only_by_prefix(&system, &target, "/tmp");
    }
}

/// Each whole-command deny builds its own string, so each needs the prefix threaded.
#[test]
fn whole_command_denies_render_per_host() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    assert_hosts_differ_only_by_prefix(&system, &bash_target("rm -rf /r"), "/tmp");

    let cli_denied = mock_with(&[("/r/.remargin.yaml", "permissions:\n  cli_allowed: false\n")]);
    assert_hosts_differ_only_by_prefix(&cli_denied, &bash_target("remargin write /r/x.md"), "/r");
}

/// `pretool` must produce exactly what `decide` produces under the Claude prefix.
#[test]
fn claude_entry_point_renders_the_claude_prefix() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/secret/foo.md" }));
    let through_hook = expect_deny(pretool(&system, &stdin));
    assert_eq!(
        deny_reason(&through_hook),
        reason_under(
            &system,
            &path_target("/r/secret/foo.md", "Read"),
            "/r",
            ToolPrefix::CLAUDE,
        ),
    );
}
