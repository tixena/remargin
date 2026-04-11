//! MCP server tests.
//!
//! Tests use the `process_request` function to exercise the JSON-RPC layer
//! without actual stdin/stdout I/O.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System as _;
use os_shim::mock::MockSystem;
use serde_json::{Value, json};

use crate::config::{Mode, ResolvedConfig};
use crate::mcp;
use crate::parser::{self, AuthorType};

/// Document with two comments for expanded query tests.
const DOC_EXPANDED: &str = "\
---
title: Expanded
---

```remargin
---
id: ex1
author: alice
type: human
ts: 2026-04-06T10:00:00-04:00
to: [bob]
checksum: sha256:ex1
---
Pending comment from alice.
```

```remargin
---
id: ex2
author: bob
type: agent
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:ex2
ack:
  - alice@2026-04-06T13:00:00-04:00
---
Acked comment from bob.
```
";

/// A document with a comment in the middle for reply placement tests.
const DOC_WITH_COMMENT: &str = "\
---
title: Test
---

# Heading

Body paragraph one.

```remargin
---
id: aaa
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:abc123
---
Original comment.
```

Body paragraph two.
";

/// Create a default config for testing.
fn test_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("tester")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        unrestricted: false,
    }
}

/// Create a mock system with a document at the given path.
fn system_with_doc(base: &Path, filename: &str, content: &str) -> MockSystem {
    let path = base.join(filename);
    MockSystem::new()
        .with_file(&path, content.as_bytes())
        .unwrap()
}

/// Send a JSON-RPC request and parse the response.
fn call(
    system: &dyn os_shim::System,
    base_dir: &Path,
    config: &ResolvedConfig,
    request: &Value,
) -> Value {
    let request_str = serde_json::to_string(request).unwrap();
    let response_str = mcp::process_request(system, base_dir, config, &request_str)
        .unwrap()
        .unwrap();
    serde_json::from_str(&response_str).unwrap()
}

/// Extract the text content from an MCP tool result.
fn extract_tool_text(response: &Value) -> Value {
    let result = &response["result"];
    let content = result["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

/// Check that a response is an MCP tool error.
fn is_tool_error(response: &Value) -> bool {
    response["result"]["isError"].as_bool().unwrap_or(false)
}

#[test]
fn initialize_returns_capabilities() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            }
        }),
    );

    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    assert!(response["result"]["capabilities"]["tools"].is_object());
    assert_eq!(response["result"]["serverInfo"]["name"], "remargin");
}

#[test]
fn tools_list_returns_all_tools() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/list",
            "params": {}
        }),
    );

    let tools = response["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 21_usize);

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    assert!(names.contains(&"comment"));
    assert!(names.contains(&"batch"));
    assert!(names.contains(&"ack"));
    assert!(names.contains(&"react"));
    assert!(names.contains(&"delete"));
    assert!(names.contains(&"edit"));
    assert!(names.contains(&"comments"));
    assert!(names.contains(&"verify"));
    assert!(names.contains(&"migrate"));
    assert!(names.contains(&"lint"));
    assert!(names.contains(&"ls"));
    assert!(names.contains(&"get"));
    assert!(names.contains(&"write"));
    assert!(names.contains(&"metadata"));
    assert!(names.contains(&"query"));
    assert!(names.contains(&"rm"));
    assert!(names.contains(&"search"));
    assert!(names.contains(&"purge"));
    assert!(names.contains(&"sandbox_add"));
    assert!(names.contains(&"sandbox_remove"));
    assert!(names.contains(&"sandbox_list"));
}

#[test]
fn tools_list_all_have_input_schema() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/list",
            "params": {}
        }),
    );

    let tools = response["result"]["tools"].as_array().unwrap();
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool["inputSchema"].is_object(),
            "tool {name} missing inputSchema"
        );
    }
}

#[test]
fn comment_creates_and_returns_id() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome body text.\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "This is a test comment."
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["id"].is_string());
    assert!(!result["id"].as_str().unwrap().is_empty());
}

#[test]
fn comments_lists_created_comment() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome text.\n");
    let config = test_config();

    // Create a comment first.
    let create_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "First comment"
                }
            }
        }),
    );
    let created_id = String::from(extract_tool_text(&create_resp)["id"].as_str().unwrap());

    // List comments.
    let list_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "comments",
                "arguments": {
                    "file": "doc.md"
                }
            }
        }),
    );
    let result = extract_tool_text(&list_resp);
    let comments = result["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1_usize);
    assert_eq!(comments[0]["id"].as_str().unwrap(), created_id);
    assert_eq!(comments[0]["author"], "tester");
    assert_eq!(comments[0]["content"], "First comment");
    // Line number should be present and positive (comment is appended after body text).
    assert!(
        comments[0]["line"].as_u64().unwrap() > 0,
        "line number should be a positive integer"
    );
}

#[test]
fn comment_missing_required_field_returns_error() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn batch_creates_multiple_comments() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nBody text.\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "batch",
                "arguments": {
                    "file": "doc.md",
                    "operations": [
                        { "content": "First batch comment" },
                        { "content": "Second batch comment" },
                        { "content": "Third batch comment" }
                    ]
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let ids = result["ids"].as_array().unwrap();
    assert_eq!(ids.len(), 3_usize);
}

#[test]
fn search_finds_text_in_document() {
    let base = Path::new("/docs");
    let system = system_with_doc(
        base,
        "doc.md",
        "# Hello\n\nThe notification system works.\n",
    );
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {
                    "pattern": "notification"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let matches = result["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1_usize);
    assert_eq!(matches[0]["line"], 3_i32);
    assert_eq!(matches[0]["location"], "body");
    assert!(
        matches[0]["text"]
            .as_str()
            .unwrap()
            .contains("notification")
    );
}

#[test]
fn ls_lists_directory() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/notes.md"), b"# Notes\n")
        .unwrap()
        .with_file(Path::new("/docs/readme.md"), b"# Readme\n")
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "ls",
                "arguments": {
                    "path": "."
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let entries = result["entries"].as_array().unwrap();
    assert!(entries.len() >= 2_usize);
}

#[test]
fn get_reads_file_content() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "notes.md", "Line 1\nLine 2\nLine 3\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get",
                "arguments": {
                    "path": "notes.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("Line 1"));
    assert!(content.contains("Line 3"));
}

#[test]
fn unknown_method_returns_error() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "nonexistent/method",
            "params": {}
        }),
    );

    assert!(response["error"].is_object());
    assert_eq!(response["error"]["code"], -32_601_i32);
}

#[test]
fn notification_returns_no_response() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let request = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    let request_str = serde_json::to_string(&request).unwrap();
    let response = mcp::process_request(&system, base, &config, &request_str).unwrap();
    assert!(response.is_none());
}

#[test]
fn unknown_tool_returns_error() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "nonexistent_tool",
                "arguments": {}
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn lint_returns_ok_for_valid_document() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "clean.md", "# Clean\n\nNo issues here.\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "lint",
                "arguments": {
                    "file": "clean.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["ok"].as_bool().unwrap());
    assert!(result["errors"].as_array().unwrap().is_empty());
}

#[test]
fn verify_checks_checksum_integrity() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nText.\n");
    let config = test_config();

    // Create a comment.
    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "Verified comment"
                }
            }
        }),
    );

    // Verify.
    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "verify",
                "arguments": {
                    "file": "doc.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);
    assert!(results[0]["checksum_ok"].as_bool().unwrap());
    assert_eq!(results[0]["signature"], "not_checked");
}

#[test]
fn purge_dry_run_reports_count() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nBody.\n");
    let config = test_config();

    // Create two comments.
    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": { "file": "doc.md", "content": "Comment A" }
            }
        }),
    );
    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": { "file": "doc.md", "content": "Comment B" }
            }
        }),
    );

    // Purge dry run.
    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 3_i32,
            "method": "tools/call",
            "params": {
                "name": "purge",
                "arguments": { "file": "doc.md", "dry_run": true }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["comments_removed"], 2_i32);
}

#[test]
fn metadata_returns_document_info() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome text.\n");
    let config = test_config();

    // Create a comment so metadata has something to report.
    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": { "file": "doc.md", "content": "Test" }
            }
        }),
    );

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "metadata",
                "arguments": { "path": "doc.md" }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["comment_count"], 1_i32);
    assert_eq!(result["pending_count"], 1_i32);
    assert!(result["line_count"].as_u64().unwrap() > 0_u64);
}

#[test]
fn response_includes_jsonrpc_version() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 42_i32,
            "method": "initialize",
            "params": {}
        }),
    );

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 42_i32);
}

#[test]
fn response_preserves_string_id() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": "request-abc",
            "method": "initialize",
            "params": {}
        }),
    );

    assert_eq!(response["id"], "request-abc");
}

#[test]
fn reply_placed_after_parent_not_appended() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    // Reply to comment "aaa" without explicit positioning.
    let reply_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "This is a reply.",
                    "reply_to": "aaa"
                }
            }
        }),
    );
    let reply_id = String::from(extract_tool_text(&reply_resp)["id"].as_str().unwrap());

    // List comments to get line numbers.
    let list_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "comments",
                "arguments": { "file": "doc.md" }
            }
        }),
    );
    let result = extract_tool_text(&list_resp);
    let comments = result["comments"].as_array().unwrap();

    let parent = comments.iter().find(|c| c["id"] == "aaa").unwrap();
    let reply = comments.iter().find(|c| c["id"] == reply_id).unwrap();

    let parent_line = parent["line"].as_u64().unwrap();
    let reply_line = reply["line"].as_u64().unwrap();

    // Reply must appear right after the parent, not at the end of the document.
    assert!(
        reply_line > parent_line,
        "reply (line {reply_line}) should be after parent (line {parent_line})"
    );
    // "Body paragraph two" is after the parent comment. The reply should be
    // between the parent and that trailing body text — not appended after it.
    // The parent is at roughly line 9. The reply should be near line 20,
    // not at the very end (which would be ~30+).
    assert!(
        reply_line < parent_line + 20,
        "reply (line {reply_line}) should be near parent (line {parent_line}), not appended to end"
    );
}

#[test]
fn reply_ignores_explicit_after_line() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    // Reply to "aaa" but also pass after_line=1 — reply_to should win.
    let reply_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "Reply with conflicting position.",
                    "reply_to": "aaa",
                    "after_line": 1_i32
                }
            }
        }),
    );
    let reply_id = String::from(extract_tool_text(&reply_resp)["id"].as_str().unwrap());

    let list_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "comments",
                "arguments": { "file": "doc.md" }
            }
        }),
    );
    let result = extract_tool_text(&list_resp);
    let comments = result["comments"].as_array().unwrap();

    let parent = comments.iter().find(|c| c["id"] == "aaa").unwrap();
    let reply = comments.iter().find(|c| c["id"] == reply_id).unwrap();

    let parent_line = parent["line"].as_u64().unwrap();
    let reply_line = reply["line"].as_u64().unwrap();

    // reply_to takes priority over after_line — reply is after parent, not at line 1.
    assert!(
        reply_line > parent_line,
        "reply (line {reply_line}) should be after parent (line {parent_line}), not at line 1"
    );
}

#[test]
fn non_reply_still_appends() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    // Comment without reply_to or explicit position — should append.
    let resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "A standalone comment."
                }
            }
        }),
    );
    let new_id = String::from(extract_tool_text(&resp)["id"].as_str().unwrap());

    let list_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "comments",
                "arguments": { "file": "doc.md" }
            }
        }),
    );
    let result = extract_tool_text(&list_resp);
    let comments = result["comments"].as_array().unwrap();

    let parent = comments.iter().find(|c| c["id"] == "aaa").unwrap();
    let new_comment = comments.iter().find(|c| c["id"] == new_id).unwrap();

    let parent_line = parent["line"].as_u64().unwrap();
    let new_line = new_comment["line"].as_u64().unwrap();

    // Non-reply appends to end — should be well past the parent and trailing body.
    assert!(
        new_line > parent_line,
        "appended comment (line {new_line}) should be after parent (line {parent_line})"
    );
}

#[test]
fn non_reply_with_after_line_respected() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    // Non-reply with after_line=5 — should place near line 5.
    let resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "Placed after line 5.",
                    "after_line": 5_i32
                }
            }
        }),
    );
    let new_id = String::from(extract_tool_text(&resp)["id"].as_str().unwrap());

    let list_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "comments",
                "arguments": { "file": "doc.md" }
            }
        }),
    );
    let result = extract_tool_text(&list_resp);
    let comments = result["comments"].as_array().unwrap();
    let new_comment = comments.iter().find(|c| c["id"] == new_id).unwrap();
    let new_line = new_comment["line"].as_u64().unwrap();

    // Should be placed near line 5, not at the end.
    assert!(
        new_line < 15,
        "comment with after_line=5 placed at line {new_line}, expected near line 6"
    );
}

#[test]
fn tool_result_includes_elapsed_ms() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome text.\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comments",
                "arguments": {
                    "file": "doc.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(
        result.get("elapsed_ms").is_some(),
        "tool result should include elapsed_ms"
    );
    assert!(
        result["elapsed_ms"].is_u64(),
        "elapsed_ms should be a non-negative integer"
    );
}

#[test]
fn mcp_query_comment_id_finds_doc() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_dir(Path::new("/docs/sub"))
        .unwrap()
        .with_file(Path::new("/docs/sub/a.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap()
        .with_file(Path::new("/docs/sub/b.md"), b"# No comments\n")
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "query",
                "arguments": {
                    "comment_id": "aaa"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);
    assert!(results[0]["path"].as_str().unwrap().contains("a.md"));
}

#[test]
fn mcp_query_comment_id_not_found_returns_empty() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "query",
                "arguments": {
                    "comment_id": "nonexistent"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert!(results.is_empty());
}

#[test]
fn mcp_query_expanded_returns_comments() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_EXPANDED.as_bytes())
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "query",
                "arguments": {
                    "expanded": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);

    let comments = results[0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2_usize);

    // Verify first comment fields.
    assert_eq!(comments[0]["id"].as_str().unwrap(), "ex1");
    assert_eq!(comments[0]["author"].as_str().unwrap(), "alice");
    assert_eq!(comments[0]["author_type"].as_str().unwrap(), "human");
    assert_eq!(
        comments[0]["content"].as_str().unwrap(),
        "Pending comment from alice."
    );
    assert!(
        comments[0]["to"]
            .as_array()
            .unwrap()
            .contains(&json!("bob"))
    );
    assert!(comments[0]["ack"].as_array().unwrap().is_empty());

    // Verify second comment.
    assert_eq!(comments[1]["id"].as_str().unwrap(), "ex2");
    assert_eq!(comments[1]["author_type"].as_str().unwrap(), "agent");
    assert_eq!(comments[1]["ack"].as_array().unwrap().len(), 1_usize);
}

#[test]
fn mcp_query_summary_omits_comments() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_EXPANDED.as_bytes())
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "query",
                "arguments": { "summary": true }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);

    // With summary mode, there should be no comments key.
    assert!(results[0].get("comments").is_none());
}

#[test]
fn mcp_ack_without_file_resolves_from_tree() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap();
    let config = test_config();

    // Ack comment "aaa" without specifying file.
    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "ack",
                "arguments": {
                    "ids": ["aaa"]
                }
            }
        }),
    );

    assert!(!is_tool_error(&response), "expected success but got error");
    let result = extract_tool_text(&response);
    assert_eq!(result["acknowledged"], json!(["aaa"]));
}

#[test]
fn mcp_ack_without_file_scopes_to_path() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_dir(Path::new("/docs/sub"))
        .unwrap()
        .with_file(Path::new("/docs/sub/a.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap();
    let config = test_config();

    // Ack with path scoping to subdirectory.
    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "ack",
                "arguments": {
                    "ids": ["aaa"],
                    "path": "sub"
                }
            }
        }),
    );

    assert!(!is_tool_error(&response), "expected success but got error");
}

#[test]
fn mcp_ack_without_file_not_found_returns_error() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "ack",
                "arguments": {
                    "ids": ["nonexistent"]
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let result = &response["result"];
    let content = result["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("not found"),
        "expected 'not found' in error: {text}"
    );
}

#[test]
fn mcp_ack_without_file_ambiguous_returns_error() {
    let base = Path::new("/docs");
    // Two documents with the same comment ID.
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap()
        .with_file(Path::new("/docs/b.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "ack",
                "arguments": {
                    "ids": ["aaa"]
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let result = &response["result"];
    let content = result["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("ambiguous"),
        "expected 'ambiguous' in error: {text}"
    );
}

#[test]
fn mcp_comment_auto_ack() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    // Reply to aaa with auto_ack.
    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "Reply with auto-ack.",
                    "reply_to": "aaa",
                    "auto_ack": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["id"].is_string());

    // Verify the parent was acked.
    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "tester");
}

#[test]
fn mcp_comment_auto_ack_without_reply_to_errors() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nBody text.\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "Top-level with auto-ack.",
                    "auto_ack": true
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let result = &response["result"];
    let content = result["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("--auto-ack requires --reply-to"),
        "unexpected error: {text}"
    );
}

#[test]
fn mcp_batch_auto_ack_per_op() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "batch",
                "arguments": {
                    "file": "doc.md",
                    "operations": [
                        { "content": "Independent comment." },
                        { "content": "Reply with ack.", "reply_to": "aaa", "auto_ack": true },
                        { "content": "Reply without ack.", "reply_to": "aaa" }
                    ]
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let ids = result["ids"].as_array().unwrap();
    assert_eq!(ids.len(), 3_usize);

    // Verify parent aaa was acked exactly once (from op1).
    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "tester");
}

#[test]
fn mcp_rm_deletes_file() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/target.md"), b"# To delete")
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "rm",
                "arguments": {
                    "path": "target.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["deleted"].as_str().unwrap(), "target.md");
    assert!(result["existed"].as_bool().unwrap());
    system
        .read_to_string(Path::new("/docs/target.md"))
        .unwrap_err();
}

#[test]
fn mcp_rm_idempotent() {
    let base = Path::new("/docs");
    let system = MockSystem::new().with_dir(Path::new("/docs")).unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "rm",
                "arguments": {
                    "path": "nonexistent.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["deleted"].as_str().unwrap(), "nonexistent.md");
    assert!(!result["existed"].as_bool().unwrap());
}

#[test]
fn mcp_rm_missing_path_param() {
    let base = Path::new("/docs");
    let system = MockSystem::new();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "rm",
                "arguments": {}
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn mcp_write_raw_param() {
    let base = Path::new("/docs");
    let system = MockSystem::new().with_dir(Path::new("/docs")).unwrap();
    let config = test_config();
    let raw_content = r#"{"nodes":[{"id":"abc"}]}"#;

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "write",
                "arguments": {
                    "path": "design.pen",
                    "content": raw_content,
                    "create": true,
                    "raw": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["written"].as_str().unwrap(), "design.pen");
    assert!(result["raw"].as_bool().unwrap());

    let on_disk = system
        .read_to_string(Path::new("/docs/design.pen"))
        .unwrap();
    assert_eq!(on_disk, raw_content);
}

#[test]
fn mcp_write_raw_rejected_for_md() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/doc.md"), b"# Hello")
        .unwrap();
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "write",
                "arguments": {
                    "path": "doc.md",
                    "content": "raw content",
                    "raw": true
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn mcp_write_binary_param() {
    let base = Path::new("/docs");
    let system = MockSystem::new().with_dir(Path::new("/docs")).unwrap();
    let config = test_config();
    let content_bytes = b"binary MCP content";
    let b64 = BASE64_STANDARD.encode(content_bytes);

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "write",
                "arguments": {
                    "path": "output.png",
                    "content": b64,
                    "create": true,
                    "binary": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["written"].as_str().unwrap(), "output.png");
    assert!(result["binary"].as_bool().unwrap());
    assert!(result["raw"].as_bool().unwrap());

    let on_disk = system
        .read_to_string(Path::new("/docs/output.png"))
        .unwrap();
    assert_eq!(on_disk.as_bytes(), content_bytes);
}

#[test]
fn mcp_write_binary_rejected_for_md() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/doc.md"), b"# Hello")
        .unwrap();
    let config = test_config();
    let b64 = BASE64_STANDARD.encode(b"binary md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "write",
                "arguments": {
                    "path": "doc.md",
                    "content": b64,
                    "binary": true
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}
