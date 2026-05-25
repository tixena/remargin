//! Unit tests for `permissions::pretool`. Every test feeds a synthetic
//! stdin envelope through `pretool()` against a `MockSystem` realm and
//! asserts the resulting `PretoolOutcome`. The core function is pure
//! so the binary never spawns.

use std::path::Path;

use os_shim::mock::MockSystem;
use serde_json::{Value, json};

use crate::permissions::pretool::{
    Decision, DecisionInner, PermissionDecision, PretoolOutcome, pretool,
};

fn mock_with(files: &[(&str, &str)]) -> MockSystem {
    let mut system = MockSystem::new();
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

/// Test 1: `Read` on unrestricted path → `SilentAllow`.
#[test]
fn read_on_unrestricted_path_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/public/foo.md" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Test 2: `Read` on restricted path → `Deny` with the `Read` message.
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

/// Test 3: `Write` on restricted path → `Deny` with the `Write` message.
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

/// Test 4: `Edit` on restricted path → `Deny` with the `Edit` message.
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

/// Test 5: `NotebookEdit` on restricted path — note the input field is
/// `notebook_path`, not `file_path`.
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

/// Test 6: `Bash` with a non-mutator verb (`echo`) → `SilentAllow`
/// even if the command mentions a restricted path.
#[test]
fn bash_non_mutator_verb_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "echo \"/r/secret/foo\"" }),
    );
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Test 7: `Bash` mutator that mentions a restricted path → `Deny`.
#[test]
fn bash_mutator_referencing_restricted_path_denies() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/secret/foo" }));
    let decision = expect_deny(pretool(&system, &stdin));
    assert!(deny_reason(&decision).contains("/r/secret"));
    assert!(deny_reason(&decision).contains("shell command"));
}

/// Test 8: `Bash` mutator that does NOT mention a restricted path →
/// `SilentAllow`.
#[test]
fn bash_mutator_on_unrestricted_path_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /r/public/foo" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Test 9: `Bash` mutator with no path reference → `SilentAllow`.
#[test]
fn bash_mutator_with_no_path_reference_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rm /tmp/x" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Test 10: per-realm `also_deny_bash` extra triggers the check.
#[test]
fn bash_per_realm_extra_verb_triggers_check() {
    let system = mock_with(&[(
        "/r/.remargin.yaml",
        &restrict_with_extra_bash("secret", "curl"),
    )]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "curl /r/secret/upload" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// Test 11: `Glob` / `Grep` are never gated.
#[test]
fn glob_tool_is_not_gated() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Glob", "/r", &json!({ "pattern": "**/*.md" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Test 12: unknown `tool_name` → `SilentAllow`.
#[test]
fn unknown_tool_name_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("FooBar", "/r", &json!({ "anything": 1_i32 }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Test 13: no `.remargin.yaml` anywhere in the cwd's ancestry →
/// `SilentAllow` (nothing is managed here).
#[test]
fn no_remargin_yaml_in_ancestry_silent_allows() {
    let system = MockSystem::new();
    let stdin = event_json("Read", "/tmp", &json!({ "file_path": "/tmp/foo.md" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}

/// Test 14: malformed stdin JSON → `Fail`.
#[test]
fn malformed_stdin_fails() {
    let system = MockSystem::new();
    let reason = expect_fail(pretool(&system, b"not json"));
    assert!(reason.contains("malformed PreToolUse event"));
}

/// Test 15: missing `tool_name` → `Fail`.
#[test]
fn missing_tool_name_fails() {
    let system = MockSystem::new();
    let reason = expect_fail(pretool(&system, b"{}"));
    assert!(reason.contains("missing field"));
}

/// Test 16: missing `tool_input.file_path` for `Read` → `Fail`.
#[test]
fn read_missing_file_path_fails() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({}));
    let reason = expect_fail(pretool(&system, &stdin));
    assert!(reason.contains("missing tool_input.file_path"));
}

/// Test 17: a relative `file_path` is resolved against the event's
/// `cwd` and then checked.
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

/// Test 18: same-path `Bash` command in many forms all `Deny`.
#[test]
fn bash_path_reference_in_many_forms_all_deny() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    for command in [
        "rm /r/secret/foo",
        "cd /r/secret && rm foo",
        "rm \"/r/secret/foo\"",
    ] {
        let stdin = event_json("Bash", "/r", &json!({ "command": command }));
        let outcome = pretool(&system, &stdin);
        assert!(
            matches!(outcome, PretoolOutcome::Deny(_)),
            "expected Deny for {command}, got {outcome:?}"
        );
    }
}

/// Test 19: wildcard restrict (`*`) catches any path in the realm.
#[test]
fn wildcard_restrict_denies_anywhere_in_realm() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("'*'"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/anything.md" }));
    assert!(matches!(pretool(&system, &stdin), PretoolOutcome::Deny(_)));
}

/// Test 20: idempotent — same input twice returns the same outcome.
#[test]
fn identical_input_yields_identical_outcome() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Read", "/r", &json!({ "file_path": "/r/secret/foo.md" }));
    let first = pretool(&system, &stdin);
    let second = pretool(&system, &stdin);
    assert_eq!(first, second);
}

/// Verb extractor skips env-var assignments so `FOO=bar cat /x`
/// resolves to `cat` (not `FOO=bar`).
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
fn bash_rtk_ls_non_mutator_restricted_path_silent_allows() {
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json("Bash", "/r", &json!({ "command": "rtk ls /r/secret/" }));
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
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
fn bash_bare_proxy_not_peeled() {
    // Without `rtk` in front, `proxy` is treated as the verb itself; it
    // is not in BASH_MUTATORS so the gate silent-allows even though the
    // restricted path is present.
    let system = mock_with(&[("/r/.remargin.yaml", &restrict_yaml("secret"))]);
    let stdin = event_json(
        "Bash",
        "/r",
        &json!({ "command": "proxy sed /r/secret/foo.md" }),
    );
    let outcome = pretool(&system, &stdin);
    assert_eq!(outcome, PretoolOutcome::SilentAllow);
}
