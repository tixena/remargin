//! Unit tests for `permissions::goose_pretool`. The two-channel verdict render and the
//! install/uninstall lifecycle can only be observed from the CLI and are not covered here.
//!
//! Every test feeds a synthetic goose `PreToolUse` envelope through
//! `goose_pretool()` against a `MemorySystem` realm. The core function is
//! pure, so the binary never spawns.

use std::path::Path;

use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use crate::permissions::goose_pretool::{BlockDecision, GooseVerdict, goose_pretool};
use crate::permissions::pretool::ToolPrefix;

fn mock_with(files: &[(&str, &str)]) -> MemorySystem {
    let mut system = MemorySystem::new();
    for (path, body) in files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    system
}

/// A realm at `/r` whose `secret/` subtree is remargin-managed.
fn realm() -> MemorySystem {
    mock_with(&[(
        "/r/.remargin.yaml",
        "permissions:\n  trusted_roots:\n    - path: secret\n",
    )])
}

fn event_json(tool_name: &str, working_dir: &str, tool_input: &Value) -> Vec<u8> {
    let envelope = json!({
        "event": "PreToolUse",
        "session_id": "test",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "matcher_context": Value::Null,
        "working_dir": working_dir,
    });
    serde_json::to_vec(&envelope).unwrap()
}

fn expect_block(verdict: GooseVerdict) -> String {
    assert!(
        matches!(verdict, GooseVerdict::Block { reason: _ }),
        "expected Block, got {verdict:?}",
    );
    let GooseVerdict::Block { reason } = verdict else {
        return String::new();
    };
    reason
}

fn assert_allow(verdict: &GooseVerdict) {
    assert_eq!(verdict, &GooseVerdict::Allow, "expected Allow");
}

/// A goose session reaches remargin's ops as `remargin__*` and has no tool
/// named `mcp__remargin__*` at all, so a reason spelling the op Claude
/// Code's way names something uncallable — the retry loop the guidance
/// exists to end. Asserting the absence is the load-bearing half: the goose
/// prefix is a substring of Claude's.
fn assert_goose_namespaced(reason: &str) {
    assert!(
        reason.contains(ToolPrefix::GOOSE.as_str()),
        "reason names no remargin op: {reason}",
    );
    assert!(
        !reason.contains(ToolPrefix::CLAUDE.as_str()),
        "reason carries Claude Code's tool prefix: {reason}",
    );
}

#[test]
fn text_editor_write_on_managed_path_blocks_with_write_guidance() {
    let stdin = event_json(
        "developer__text_editor",
        "/r",
        &json!({ "command": "write", "path": "/r/secret/foo.md", "file_text": "x" }),
    );
    let reason = expect_block(goose_pretool(&realm(), &stdin));
    assert!(reason.contains("remargin__write"), "reason: {reason}");
    assert!(reason.contains("/r/secret/foo.md"), "reason: {reason}");
    assert_goose_namespaced(&reason);
}

/// Edit-class verbs redirect to the edit op, not the whole-file write op.
#[test]
fn text_editor_str_replace_and_insert_block_with_edit_guidance() {
    for command in ["str_replace", "insert"] {
        let stdin = event_json(
            "developer__text_editor",
            "/r",
            &json!({ "command": command, "path": "/r/secret/foo.md" }),
        );
        let reason = expect_block(goose_pretool(&realm(), &stdin));
        assert!(
            reason.contains("remargin__edit"),
            "{command} reason: {reason}",
        );
        assert_goose_namespaced(&reason);
    }
}

/// `view` is the read-class verb and redirects to the read op.
#[test]
fn text_editor_view_on_managed_path_blocks_with_get_guidance() {
    let stdin = event_json(
        "developer__text_editor",
        "/r",
        &json!({ "command": "view", "path": "/r/secret/foo.md" }),
    );
    let reason = expect_block(goose_pretool(&realm(), &stdin));
    assert!(reason.contains("remargin__get"), "reason: {reason}");
    assert_goose_namespaced(&reason);
}

#[test]
fn text_editor_relative_path_is_rooted_at_working_dir() {
    let stdin = event_json(
        "developer__text_editor",
        "/r/secret",
        &json!({ "command": "write", "path": "foo.md" }),
    );
    let reason = expect_block(goose_pretool(&realm(), &stdin));
    assert!(reason.contains("/r/secret/foo.md"), "reason: {reason}");
}

#[test]
fn shell_word_inside_managed_subtree_blocks() {
    let stdin = event_json(
        "developer__shell",
        "/tmp",
        &json!({ "command": "cat /r/secret/foo.md" }),
    );
    let reason = expect_block(goose_pretool(&realm(), &stdin));
    assert!(reason.contains("remargin__get"), "reason: {reason}");
    assert_goose_namespaced(&reason);
}

/// In-realm `working_dir`: bare words pass, path-evidenced words block.
#[test]
fn shell_from_in_realm_working_dir_follows_per_word_scan() {
    let bare = event_json("developer__shell", "/r/secret", &json!({ "command": "ls" }));
    assert_allow(&goose_pretool(&realm(), &bare));

    let evidenced = event_json(
        "developer__shell",
        "/r/secret",
        &json!({ "command": "cat ./idea.md" }),
    );
    let reason = expect_block(goose_pretool(&realm(), &evidenced));
    assert_goose_namespaced(&reason);
}

#[test]
fn unmanaged_path_allows_on_both_gated_tools() {
    let system = realm();
    let editor = event_json(
        "developer__text_editor",
        "/r",
        &json!({ "command": "write", "path": "/r/public/foo.md" }),
    );
    assert_allow(&goose_pretool(&system, &editor));

    let shell = event_json(
        "developer__shell",
        "/tmp",
        &json!({ "command": "ls /r/public" }),
    );
    assert_allow(&goose_pretool(&system, &shell));
}

/// Intercepting remargin's own tools would leave no way to touch managed content.
#[test]
fn remargin_mcp_tools_are_never_intercepted() {
    let system = realm();
    for tool in ["remargin__write", "mcp__remargin__write"] {
        let stdin = event_json(tool, "/r/secret", &json!({ "path": "/r/secret/foo.md" }));
        assert_allow(&goose_pretool(&system, &stdin));
    }
}

/// goose treats a silent hook as permission to proceed, so an unreadable payload blocks.
#[test]
fn truncated_payload_blocks() {
    let reason = expect_block(goose_pretool(&realm(), b"{\"tool_name\": \"developer__"));
    assert!(reason.contains("remargin"), "reason: {reason}");
}

#[test]
fn empty_payload_blocks() {
    let _reason = expect_block(goose_pretool(&realm(), b""));
}

#[test]
fn gated_tool_missing_required_field_blocks() {
    let system = realm();
    let editor_no_path = event_json(
        "developer__text_editor",
        "/r",
        &json!({ "command": "write" }),
    );
    let reason = expect_block(goose_pretool(&system, &editor_no_path));
    assert!(reason.contains("path"), "reason: {reason}");

    let editor_no_command = event_json(
        "developer__text_editor",
        "/r",
        &json!({ "path": "/r/x.md" }),
    );
    let _editor = expect_block(goose_pretool(&system, &editor_no_command));

    let shell_no_command = event_json("developer__shell", "/r", &json!({}));
    let _shell = expect_block(goose_pretool(&system, &shell_no_command));
}

#[test]
fn unrecognized_text_editor_command_blocks() {
    let stdin = event_json(
        "developer__text_editor",
        "/r",
        &json!({ "command": "teleport", "path": "/r/public/foo.md" }),
    );
    let reason = expect_block(goose_pretool(&realm(), &stdin));
    assert!(reason.contains("teleport"), "reason: {reason}");
}

/// Without `working_dir` a relative target cannot be rooted.
#[test]
fn gated_tool_without_working_dir_blocks() {
    let envelope = json!({
        "event": "PreToolUse",
        "tool_name": "developer__shell",
        "tool_input": { "command": "ls" },
    });
    let stdin = serde_json::to_vec(&envelope).unwrap();
    let reason = expect_block(goose_pretool(&realm(), &stdin));
    assert!(reason.contains("working_dir"), "reason: {reason}");
}

/// An unknown extension reaching a managed path is the same reach under another name.
#[test]
fn ungated_tool_naming_a_managed_path_blocks() {
    let system = realm();
    for key in ["path", "file_path"] {
        let stdin = event_json("other__tool", "/r", &json!({ key: "/r/secret/foo.md" }));
        let reason = expect_block(goose_pretool(&system, &stdin));
        assert!(reason.contains("/r/secret/foo.md"), "reason: {reason}");
    }

    let shell_shaped = event_json("other__tool", "/tmp", &json!({ "command": "rm /r/secret" }));
    let _reason = expect_block(goose_pretool(&system, &shell_shaped));
}

#[test]
fn ungated_tool_without_a_gated_shape_allows() {
    let stdin = event_json("other__tool", "/r/secret", &json!({ "query": "hello" }));
    assert_allow(&goose_pretool(&realm(), &stdin));
}

/// Each deny family builds its own string, so each is checked for goose's namespacing.
#[test]
fn every_deny_family_names_goose_namespaced_ops() {
    let system = realm();
    let cases = [
        event_json(
            "developer__text_editor",
            "/r",
            &json!({ "command": "write", "path": "/r/secret/foo.md" }),
        ),
        event_json(
            "developer__shell",
            "/tmp",
            &json!({ "command": "cat /r/secret/foo.md" }),
        ),
        event_json(
            "developer__shell",
            "/tmp",
            &json!({ "command": "rm -rf /r/secret" }),
        ),
    ];
    for stdin in &cases {
        assert_goose_namespaced(&expect_block(goose_pretool(&system, stdin)));
    }

    let cli_denied = mock_with(&[("/r/.remargin.yaml", "permissions:\n  cli_allowed: false\n")]);
    let stdin = event_json(
        "developer__shell",
        "/r",
        &json!({ "command": "remargin write /r/x.md" }),
    );
    let reason = expect_block(goose_pretool(&cli_denied, &stdin));
    assert!(reason.contains("cli_allowed: false"), "reason: {reason}");
    assert_goose_namespaced(&reason);
}

#[test]
fn cli_default_deny_names_goose_namespaced_ops() {
    let stdin = event_json(
        "developer__shell",
        "/tmp",
        &json!({ "command": "remargin ls" }),
    );
    let reason = expect_block(goose_pretool(&mock_with(&[]), &stdin));
    assert!(reason.contains("cli_allowed: true"), "reason: {reason}");
    assert!(!reason.contains("cli_allowed: false"), "reason: {reason}");
    assert_goose_namespaced(&reason);
}

/// The stdout channel carries goose's documented block object verbatim.
#[test]
fn block_decision_serializes_to_goose_shape() {
    let payload = serde_json::to_value(BlockDecision::new("nope")).unwrap();
    assert_eq!(payload, json!({ "decision": "block", "reason": "nope" }));
}
