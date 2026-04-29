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
use crate::operations::{CreateCommentParams, create_comment};
use crate::parser::{self, AuthorType};
use crate::writer::InsertPosition;

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

/// A four-shape fixture used by the `pending_for_me` / `pending_broadcast`
/// tests (rem-4j91). Covers: fresh broadcast (no acks), broadcast the
/// caller already acked, directed-to-caller, and directed-to-someone-else.
const DOC_FOUR_SHAPES: &str = "\
---
title: Four Shapes
---

```remargin
---
id: brd_open
author: bot
type: agent
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:b0
---
Fresh broadcast, zero acks.
```

```remargin
---
id: brd_mine
author: bot
type: agent
ts: 2026-04-06T09:30:00-04:00
checksum: sha256:b1
ack:
  - tester@2026-04-06T10:00:00-04:00
---
Broadcast already acked by tester.
```

```remargin
---
id: dir_me
author: bob
type: human
ts: 2026-04-06T10:00:00-04:00
to: [tester]
checksum: sha256:dm
---
Directed to tester.
```

```remargin
---
id: dir_other
author: alice
type: human
ts: 2026-04-06T10:30:00-04:00
to: [bob]
checksum: sha256:do
---
Directed to bob.
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
checksum: sha256:3e5121224e71bb75be3d2a2ac568d2117b6cd3aa10a54f7abc9b19cdb1976b2e
---
Original comment.
```

Body paragraph two.
";

/// rem-5oqx: doc with two top-level sections each containing a sibling
/// heading whose label collides — used to exercise the path-disambiguation
/// resolver and the multi-anchor `batch` flow.
const DOC_WITH_HEADINGS: &str = "\
---
title: Headings
---

# Activity epic tests

## A10. MCP / CLI parity

Body for A10.

# Permissions epic tests

## P11. MCP / CLI parity

Body for P11.

## P3. deny_ops

Body for P3.
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
        source_path: None,
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
#[expect(
    clippy::cognitive_complexity,
    reason = "roll-call of every registered tool; adding one `names.contains` per tool is the clearest form"
)]
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
    assert_eq!(tools.len(), 30_usize);

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    assert!(names.contains(&"comment"));
    assert!(names.contains(&"batch"));
    assert!(names.contains(&"ack"));
    assert!(names.contains(&"activity"));
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
    assert!(names.contains(&"mv"));
    assert!(names.contains(&"permissions_show"));
    assert!(names.contains(&"permissions_check"));
    assert!(names.contains(&"plan"));
    assert!(names.contains(&"query"));
    assert!(names.contains(&"restrict"));
    assert!(names.contains(&"unprotect"));
    assert!(names.contains(&"rm"));
    assert!(names.contains(&"search"));
    assert!(names.contains(&"sign"));
    assert!(names.contains(&"purge"));
    assert!(names.contains(&"sandbox_add"));
    assert!(names.contains(&"sandbox_remove"));
    assert!(names.contains(&"sandbox_list"));
    assert!(names.contains(&"identity_create"));
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
    // No signature block on the freshly-written comment + no registry in
    // the test config → status is `missing` (neutral in open mode).
    assert_eq!(results[0]["signature"], "missing");
    assert!(result["ok"].as_bool().unwrap(), "verify should pass");
}

// Note: the purge `dry_run` smoke test was removed in rem-0ry along
// with the flag itself; `plan` with op="purge" is the preview path.

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
    // File-level fields from rem-lqz are always present.
    assert_eq!(result["binary"], false);
    assert_eq!(result["mime"], "text/markdown");
    assert!(result["path"].is_string());
    assert!(result["size_bytes"].is_number());
}

#[test]
fn get_binary_returns_base64_and_mime() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "pic.png", "fake-png-bytes");
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
                "arguments": { "path": "pic.png", "binary": true }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["binary"], true);
    assert_eq!(result["mime"], "image/png");
    assert!(result["path"].is_string());
    assert!(result["size_bytes"].is_number());
    // base64 of "fake-png-bytes"
    assert_eq!(result["content"], "ZmFrZS1wbmctYnl0ZXM=");
}

#[test]
fn get_binary_rejects_markdown() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# hi\n");
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
                "arguments": { "path": "doc.md", "binary": true }
            }
        }),
    );

    // Error surfaces as an `isError: true` tool response, not a JSON-RPC error.
    let is_error = response["result"]["isError"].as_bool().unwrap_or(false);
    assert!(is_error, "binary get on .md should be an error response");
}

#[test]
fn metadata_binary_file_omits_markdown_fields() {
    let base = Path::new("/docs");
    // Content is irrelevant for PNG metadata: only the extension drives
    // mime/binary detection. Use an ASCII placeholder to keep the helper's
    // &str signature happy.
    let system = system_with_doc(base, "pic.png", "fake-png-bytes");
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
                "name": "metadata",
                "arguments": { "path": "pic.png" }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["binary"], true);
    assert_eq!(result["mime"], "image/png");
    assert!(result["path"].is_string());
    assert!(result["size_bytes"].is_number());
    // Markdown-shaped fields must be absent.
    assert!(result.get("comment_count").is_none());
    assert!(result.get("line_count").is_none());
    assert!(result.get("pending_count").is_none());
    assert!(result.get("frontmatter").is_none());
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

#[test]
fn mcp_write_partial_params_splice_range() {
    // rem-24p: MCP `write` accepts start_line/end_line and splices the
    // provided content into that range, mirroring CLI --lines semantics.
    let base = Path::new("/docs");
    let original = "\
---
title: Test
description: ''
author: eduardo
created: 2026-04-18T00:00:00+00:00
remargin_pending: 0
remargin_pending_for: []
remargin_last_activity: null
---

body A
body B
body C
";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/doc.md"), original.as_bytes())
        .unwrap();
    let config = test_config();

    // Lines 11/12/13 are `body A`, `body B`, `body C` — replace line 12.
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
                    "content": "BODY B NEW",
                    "start_line": 12_i32,
                    "end_line": 12_i32
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["written"].as_str().unwrap(), "doc.md");

    let on_disk = system.read_to_string(Path::new("/docs/doc.md")).unwrap();
    assert!(
        on_disk.contains("body A\nBODY B NEW\nbody C"),
        "partial write did not splice correctly: {on_disk}"
    );
}

#[test]
fn mcp_write_partial_rejects_missing_end_line() {
    // Both start_line and end_line must be provided together — a lone
    // start_line is a nonsense request.
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/doc.md"), b"A\nB\nC\n")
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
                    "content": "x",
                    "start_line": 1_i32
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn mcp_write_reports_noop_true_on_identical_content() {
    // rem-1f2: the `write` tool response must carry `noop: true` when
    // the proposed content is byte-identical to what's on disk so
    // agents can branch on it (e.g. skip follow-up verification).
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/notes.txt"), b"hello\n")
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
                    "path": "notes.txt",
                    "content": "hello\n",
                    "raw": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["noop"].as_bool(), Some(true));
    assert_eq!(result["written"].as_str(), Some("notes.txt"));
}

#[test]
fn mcp_write_reports_noop_false_on_real_change() {
    // Mirror test: a real byte change produces `noop: false` so the
    // flag is reliable as a branch condition.
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/notes.txt"), b"hello\n")
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
                    "path": "notes.txt",
                    "content": "hello world\n",
                    "raw": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["noop"].as_bool(), Some(false));
}

#[test]
fn mcp_reply_prepends_parent_author_to_list() {
    // Parity test for rem-kja: the MCP `comment` tool inherits the
    // "parent author always first in `to:`" invariant from operations.
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    // Reply to `aaa` (authored by `eduardo`) with explicit to=[bob].
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
                    "content": "MCP reply with extra recipient.",
                    "reply_to": "aaa",
                    "to": ["bob"]
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let new_id = result["id"].as_str().unwrap();

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let reply = doc.find_comment(new_id).unwrap();
    assert_eq!(
        reply.to,
        vec![String::from("eduardo"), String::from("bob")],
        "MCP comment handler should prepend parent author to explicit to",
    );
}

/// Seed a document through the real `operations::create_comment` path so
/// comment checksums are valid. Returns the generated comment id so tests
/// can reference it in plan requests.
fn seed_real_comment(base: &Path, filename: &str) -> (MockSystem, ResolvedConfig, String) {
    let path = base.join(filename);
    let system = MockSystem::new()
        .with_file(&path, b"# Plan fixture\n\nBody text.\n")
        .unwrap();
    let config = test_config();
    let id = create_comment(
        &system,
        &path,
        &config,
        &CreateCommentParams::new("seed comment", &InsertPosition::Append),
    )
    .unwrap();
    (system, config, id)
}

#[test]
fn mcp_plan_ack_returns_report_without_touching_disk() {
    let base = Path::new("/docs");
    let (system, config, id) = seed_real_comment(base, "doc.md");

    // Capture on-disk bytes before the call so we can assert idempotence.
    let before_bytes = system.read_to_string(&base.join("doc.md")).unwrap();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "ack",
                    "file": "doc.md",
                    "ids": [id],
                    "identity": "bob",
                    "type": "human"
                }
            }
        }),
    );

    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "ack");
    assert_eq!(report["would_commit"], true);
    assert_eq!(report["noop"], false);
    assert!(report["checksum_before"].is_string());
    assert!(report["checksum_after"].is_string());
    assert_ne!(report["checksum_before"], report["checksum_after"]);
    // ack mutates the `ack` metadata list; the comment content is
    // unchanged so its content-derived checksum stays identical, and the
    // diff classes it as `preserved`.
    assert_eq!(report["comments"]["preserved"].as_array().unwrap().len(), 1);

    // Disk is untouched: plan is side-effect-free.
    let after_bytes = system.read_to_string(&base.join("doc.md")).unwrap();
    assert_eq!(before_bytes, after_bytes);
}

#[test]
fn mcp_plan_delete_reports_modified_ranges() {
    let base = Path::new("/docs");
    let (system, config, id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "delete",
                    "file": "doc.md",
                    "ids": [id]
                }
            }
        }),
    );

    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "delete");
    assert_eq!(report["would_commit"], true);
    assert_eq!(report["comments"]["destroyed"].as_array().unwrap().len(), 1);
}

#[test]
fn mcp_plan_react_adds_emoji() {
    let base = Path::new("/docs");
    let (system, config, id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "react",
                    "file": "doc.md",
                    "id": id,
                    "emoji": "+1"
                }
            }
        }),
    );

    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "react");
    assert_eq!(report["would_commit"], true);
    // React touches the reactions map on the comment (metadata, not
    // content), so the diff reports it as preserved rather than
    // modified — content-derived checksums are unchanged.
    assert_eq!(report["comments"]["preserved"].as_array().unwrap().len(), 1);
    assert_ne!(report["checksum_before"], report["checksum_after"]);
}

#[test]
fn mcp_plan_rejects_missing_comment_id() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "ack",
                    "file": "doc.md",
                    "ids": ["does-not-exist"]
                }
            }
        }),
    );

    assert!(
        is_tool_error(&response),
        "expected projection failure for missing comment id: {response}"
    );
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("not found"),
        "expected not-found message, got: {msg}"
    );
}

#[test]
fn mcp_plan_write_markdown_create_projects_without_writing_disk() {
    // rem-bhk: `plan write` now projects the same PlanReport the CLI
    // emits, without touching disk. Use `create: true` against a fresh
    // filename so the preservation check has no prior comments to
    // enforce.
    let base = Path::new("/docs");
    // Seed a sibling file so `/docs` exists as a directory in the
    // `MockSystem`; the sandbox resolver needs the parent to be present
    // even when the target file is still missing.
    let system = MockSystem::new()
        .with_file(base.join("seed.md"), b"# seed\n")
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
                "name": "plan",
                "arguments": {
                    "op": "write",
                    "file": "new.md",
                    "content": "# Brand new doc\n\nBody text.\n",
                    "create": true
                }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan write (create) should succeed: {response}"
    );
    let report_text = response["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(report_text).unwrap();
    assert_eq!(report["op"], "write");
    assert!(!report["noop"].as_bool().unwrap());

    assert!(
        system.read_to_string(&base.join("new.md")).is_err(),
        "plan write must not write disk"
    );
}

#[test]
fn mcp_plan_write_raw_non_markdown_returns_unsupported_reject_reason() {
    // `raw` / `binary` writes to non-markdown files produce a degraded
    // `WriteProjection::Unsupported` report with `reject_reason` and
    // `would_commit: false`. `.md` + `raw` is a hard error in
    // `validate_write_opts` (symmetric with CLI), so exercise the
    // reachable branch with a `.txt` path.
    let base = Path::new("/docs");
    let path = base.join("data.txt");
    let system = MockSystem::new().with_file(&path, b"old bytes\n").unwrap();
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
                "name": "plan",
                "arguments": {
                    "op": "write",
                    "file": "data.txt",
                    "content": "new raw bytes",
                    "raw": true
                }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan write raw (non-md) should return a report, not an error: {response}"
    );
    let report_text = response["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(report_text).unwrap();
    assert!(!report["would_commit"].as_bool().unwrap());
    assert!(report["reject_reason"].is_string());
}

#[test]
fn mcp_plan_comment_projects_new_comment() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");
    let before_bytes = system.read_to_string(&base.join("doc.md")).unwrap();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "comment",
                    "file": "doc.md",
                    "content": "Projected via MCP."
                }
            }
        }),
    );

    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "comment");
    assert_eq!(
        report["comments"]["added"].as_array().unwrap().len(),
        1,
        "expected 1 added comment, got report: {report:#}"
    );

    // Disk untouched.
    let after_bytes = system.read_to_string(&base.join("doc.md")).unwrap();
    assert_eq!(before_bytes, after_bytes);
}

#[test]
fn mcp_plan_comment_reply_auto_acks_parent() {
    let base = Path::new("/docs");
    let (system, config, parent_id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "comment",
                    "file": "doc.md",
                    "content": "reply",
                    "reply_to": parent_id,
                    "auto_ack": true
                }
            }
        }),
    );

    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "comment");
    assert_eq!(report["comments"]["added"].as_array().unwrap().len(), 1);
    // Parent stays in the `preserved` set (its content-checksum is
    // unchanged; only the ack list flipped).
    let preserved_has_parent = report["comments"]["preserved"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some(parent_id.as_str()));
    assert!(preserved_has_parent, "expected parent in preserved set");
}

#[test]
fn mcp_plan_edit_changes_content_and_clears_acks() {
    let base = Path::new("/docs");
    let (system, config, id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "edit",
                    "file": "doc.md",
                    "id": id,
                    "content": "Rewritten via plan."
                }
            }
        }),
    );

    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "edit");
    // Edit recomputes the content-derived checksum, so the comment moves
    // to the `modified` set.
    assert_eq!(report["comments"]["modified"].as_array().unwrap().len(), 1);
}

#[test]
fn mcp_plan_edit_missing_comment_errors() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "edit",
                    "file": "doc.md",
                    "id": "missing",
                    "content": "noop"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn mcp_plan_batch_projects_two_sub_ops() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");
    let before_bytes = system.read_to_string(&base.join("doc.md")).unwrap();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": {
                    "op": "batch",
                    "file": "doc.md",
                    "ops": [
                        { "content": "first new" },
                        { "content": "second new" }
                    ]
                }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan batch should succeed: {response}"
    );
    let report_text = response["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(report_text).unwrap();
    assert_eq!(report["op"], "batch");
    assert_eq!(
        report["comments"]["added"].as_array().unwrap().len(),
        2,
        "two sub-ops must produce two added comment ids"
    );

    let after_bytes = system.read_to_string(&base.join("doc.md")).unwrap();
    assert_eq!(before_bytes, after_bytes, "plan batch must not write disk");
}

#[test]
fn mcp_plan_batch_requires_ops_array() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": { "op": "batch", "file": "doc.md" }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("ops"),
        "error message must mention missing `ops` array: {msg}"
    );
}

#[test]
fn mcp_plan_purge_destroys_every_comment_id() {
    let base = Path::new("/docs");
    let (system, config, id) = seed_real_comment(base, "doc.md");
    let before_bytes = system.read_to_string(&base.join("doc.md")).unwrap();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": { "op": "purge", "file": "doc.md" }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan purge should succeed: {response}"
    );
    let report_text = response["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(report_text).unwrap();
    assert_eq!(report["op"], "purge");
    let destroyed: Vec<String> = report["comments"]["destroyed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| String::from(v.as_str().unwrap()))
        .collect();
    assert!(
        destroyed.contains(&id),
        "purge must destroy the seeded comment: {destroyed:?}"
    );

    let after_bytes = system.read_to_string(&base.join("doc.md")).unwrap();
    assert_eq!(before_bytes, after_bytes, "plan purge must not write disk");
}

#[test]
fn mcp_plan_migrate_no_op_without_legacy_comments() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": { "op": "migrate", "file": "doc.md" }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan migrate should succeed: {response}"
    );
    let report_text = response["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(report_text).unwrap();
    assert_eq!(report["op"], "migrate");
    assert!(
        report["noop"].as_bool().unwrap(),
        "migrate on a doc with no legacy markers must be a noop"
    );
}

#[test]
fn mcp_plan_sandbox_add_rewrites_frontmatter() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");
    let before_bytes = system.read_to_string(&base.join("doc.md")).unwrap();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": { "op": "sandbox-add", "file": "doc.md" }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan sandbox-add should succeed: {response}"
    );
    let report_text = response["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(report_text).unwrap();
    assert_eq!(report["op"], "sandbox-add");
    assert!(
        !report["noop"].as_bool().unwrap(),
        "sandbox-add against a clean doc must land a non-noop plan"
    );

    let after_bytes = system.read_to_string(&base.join("doc.md")).unwrap();
    assert_eq!(
        before_bytes, after_bytes,
        "plan sandbox-add must not write disk"
    );
}

#[test]
fn mcp_plan_sandbox_remove_noop_when_not_present() {
    let base = Path::new("/docs");
    let (system, config, _id) = seed_real_comment(base, "doc.md");

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "plan",
                "arguments": { "op": "sandbox-remove", "file": "doc.md" }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan sandbox-remove should succeed: {response}"
    );
    let report_text = response["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(report_text).unwrap();
    assert_eq!(report["op"], "sandbox-remove");
    assert!(
        report["noop"].as_bool().unwrap(),
        "sandbox-remove on a doc without the caller's entry must be a noop"
    );
}

#[test]
fn mcp_plan_rejects_unknown_op() {
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
                "name": "plan",
                "arguments": { "op": "nope" }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("unknown op"),
        "expected unknown-op message, got: {msg}"
    );
}

// ---------- rem-x2bw: identity-declaration schema parity ----------

/// Every mutating tool that accepts a per-tool identity declaration must
/// advertise the four-field contract (`config_path`, `identity`, `type`,
/// `key`) and the top-level `not/allOf` exclusivity clause. Read-only
/// tools MUST NOT.
#[test]
fn identity_declaration_schema_present_on_mutating_tools() {
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
    let mutating = [
        "ack",
        "batch",
        "comment",
        "delete",
        "edit",
        "migrate",
        "plan",
        "purge",
        "react",
        "sandbox_add",
        "sandbox_list",
        "sandbox_remove",
        "sign",
        "write",
    ];
    for name in mutating {
        let maybe_tool = tools.iter().find(|t| t["name"] == name);
        assert!(maybe_tool.is_some(), "missing tool {name}");
        let tool = maybe_tool.unwrap();
        let props = &tool["inputSchema"]["properties"];
        for field in ["config_path", "identity", "key", "type"] {
            assert!(
                props[field].is_object(),
                "tool {name} missing identity-declaration field {field}"
            );
        }
        let not = &tool["inputSchema"]["not"];
        assert!(
            not.is_object(),
            "tool {name} missing top-level `not` exclusivity clause"
        );
        let required = not["allOf"][0]["required"].as_array().unwrap();
        assert!(
            required.iter().any(|v| v == "config_path"),
            "tool {name} exclusivity clause missing config_path"
        );
    }

    let read_only = [
        "comments", "get", "lint", "ls", "metadata", "query", "search",
    ];
    for name in read_only {
        let maybe_tool = tools.iter().find(|t| t["name"] == name);
        assert!(maybe_tool.is_some(), "missing tool {name}");
        let tool = maybe_tool.unwrap();
        let props = &tool["inputSchema"]["properties"];
        for field in ["config_path", "identity", "key", "type"] {
            assert!(
                props.get(field).is_none_or(Value::is_null),
                "read-only tool {name} must not advertise identity-declaration field {field}"
            );
        }
    }
}

/// No MCP tool schema may surface a `mode` or `dry_run` field. Mode is a
/// tree property (rem-wws) and `dry_run` migrated to `plan` (rem-0ry).
#[test]
fn no_mode_or_dry_run_in_any_schema() {
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
        let schema_str = serde_json::to_string(&tool["inputSchema"]).unwrap();
        assert!(
            !schema_str.contains("\"mode\""),
            "tool {name} schema still carries a `mode` field: {schema_str}"
        );
        assert!(
            !schema_str.contains("\"dry_run\""),
            "tool {name} schema still carries a `dry_run` field: {schema_str}"
        );
    }
}

/// Passing both `config_path` and `identity` to the same call must be
/// rejected — schema-level `not/allOf` says so, and the handler re-checks.
#[test]
fn comment_rejects_config_path_with_identity_declaration() {
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
                    "file": "doc.md",
                    "content": "hi",
                    "config_path": "/does/not/matter.yaml",
                    "identity": "bob"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("config_path conflicts with identity"),
        "expected exclusivity diagnostic, got: {msg}"
    );
}

/// `config_path` alone loads the target file and adopts its identity for
/// this call without touching any other caller field.
#[test]
fn comment_with_config_path_loads_alternate_identity() {
    let base = Path::new("/docs");
    let config_yaml =
        b"identity: alt-alice\ntype: human\nassets_dir: assets\nmode: open\n" as &[u8];
    let system = MockSystem::new()
        .with_file(base.join("doc.md"), b"# Hello\n\nBody.\n")
        .unwrap()
        .with_file(Path::new("/other/.remargin.yaml"), config_yaml)
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
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "via config_path",
                    "config_path": "/other/.remargin.yaml"
                }
            }
        }),
    );

    assert!(!is_tool_error(&response), "got error: {response}");
    let created_id = String::from(extract_tool_text(&response)["id"].as_str().unwrap());

    // Verify the comment was authored by the config-declared identity.
    let doc_text = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_text).unwrap();
    let comment = doc.find_comment(&created_id).unwrap();
    assert_eq!(comment.author, "alt-alice");
    assert_eq!(comment.author_type, AuthorType::Human);
}

/// Manual `{identity, type}` declaration (open mode; no key required) is the
/// branch-2 happy path — identity and type are both adopted.
#[test]
fn comment_with_manual_identity_type_declaration_writes_as_new_identity() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nBody.\n");
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
                    "content": "manual declaration",
                    "identity": "manual-bob",
                    "type": "agent"
                }
            }
        }),
    );

    assert!(!is_tool_error(&response), "got error: {response}");
    let created_id = String::from(extract_tool_text(&response)["id"].as_str().unwrap());

    let doc_text = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_text).unwrap();
    let comment = doc.find_comment(&created_id).unwrap();
    assert_eq!(comment.author, "manual-bob");
    assert_eq!(comment.author_type, AuthorType::Agent);
}

/// Unknown author-type strings surface a diagnostic rather than silently
/// falling through — matches the CLI's `parse_author_type` behavior.
#[test]
fn comment_rejects_unknown_type_value() {
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
                    "file": "doc.md",
                    "content": "x",
                    "identity": "bob",
                    "type": "martian"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("unknown author type"),
        "expected author-type diagnostic, got: {msg}"
    );
}

// ===========================================================================
// query.pending_for_me + pending_broadcast MCP tests (rem-4j91)
// ===========================================================================

#[test]
fn mcp_query_pending_includes_broadcast_rem_4j91() {
    // --pending must now surface broadcast comments (the bug fix).
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_FOUR_SHAPES.as_bytes())
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
                    "expanded": true,
                    "pending": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let comments = result["results"][0]["comments"].as_array().unwrap();
    let mut ids: Vec<&str> = comments.iter().map(|c| c["id"].as_str().unwrap()).collect();
    ids.sort_unstable();
    // Expected pending: brd_open (broadcast, no acks), dir_me, dir_other.
    // brd_mine is NOT pending (tester's ack closes the broadcast).
    assert_eq!(ids, vec!["brd_open", "dir_me", "dir_other"]);
}

#[test]
fn mcp_query_pending_for_me_uses_server_identity() {
    // pending_for_me=true must use the server's configured identity
    // ("tester" from test_config), surfacing only dir_me.
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_FOUR_SHAPES.as_bytes())
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
                    "expanded": true,
                    "pending_for_me": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let comments = result["results"][0]["comments"].as_array().unwrap();
    let ids: Vec<&str> = comments.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["dir_me"]);
}

#[test]
fn mcp_query_pending_broadcast_only_surfaces_unacked_broadcasts() {
    // pending_broadcast=true with the server identity (tester): only
    // brd_open surfaces — brd_mine is already acked by tester, and
    // directed comments never count as broadcast.
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_FOUR_SHAPES.as_bytes())
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
                    "expanded": true,
                    "pending_broadcast": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let comments = result["results"][0]["comments"].as_array().unwrap();
    let ids: Vec<&str> = comments.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["brd_open"]);
}

#[test]
fn mcp_query_pending_for_me_and_broadcast_union() {
    // Union of directed-to-me (dir_me) and unacked broadcasts for me
    // (brd_open). brd_mine is acked by tester, so excluded.
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_FOUR_SHAPES.as_bytes())
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
                    "expanded": true,
                    "pending_for_me": true,
                    "pending_broadcast": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let comments = result["results"][0]["comments"].as_array().unwrap();
    let mut ids: Vec<&str> = comments.iter().map(|c| c["id"].as_str().unwrap()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["brd_open", "dir_me"]);
}

#[test]
fn mcp_query_pending_for_me_errors_without_identity() {
    // A config with no identity must fail loudly when pending_for_me
    // is requested.
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), DOC_FOUR_SHAPES.as_bytes())
        .unwrap();
    let config = ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: None,
        identity: None,
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        unrestricted: false,
    };

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
                    "pending_for_me": true
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("pending_for_me") || msg.contains("identity"),
        "expected identity diagnostic, got: {msg}"
    );
}

// ===========================================================================
// identity_create MCP tests (rem-8cnc)
// ===========================================================================

#[test]
fn mcp_identity_create_minimal_returns_yaml() {
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
                "name": "identity_create",
                "arguments": {
                    "identity": "alice",
                    "type": "human"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["identity"].as_str().unwrap(), "alice");
    assert_eq!(result["type"].as_str().unwrap(), "human");
    assert!(result["key"].is_null());
    assert_eq!(
        result["yaml"].as_str().unwrap(),
        "identity: alice\ntype: human\n"
    );
}

#[test]
fn mcp_identity_create_with_key() {
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
                "name": "identity_create",
                "arguments": {
                    "identity": "bot",
                    "type": "agent",
                    "key": "mykey"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["identity"].as_str().unwrap(), "bot");
    assert_eq!(result["type"].as_str().unwrap(), "agent");
    assert_eq!(result["key"].as_str().unwrap(), "mykey");
    assert_eq!(
        result["yaml"].as_str().unwrap(),
        "identity: bot\ntype: agent\nkey: mykey\n"
    );
}

#[test]
fn mcp_identity_create_rejects_invalid_type() {
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
                "name": "identity_create",
                "arguments": {
                    "identity": "alice",
                    "type": "martian"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.contains("martian") || msg.contains("author type"),
        "expected author-type diagnostic, got: {msg}"
    );
}

#[test]
fn mcp_identity_create_yaml_never_contains_mode() {
    // Parity with the CLI: mode is tree-level, never identity-scoped.
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
                "name": "identity_create",
                "arguments": {
                    "identity": "alice",
                    "type": "human",
                    "key": "mykey"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let yaml = result["yaml"].as_str().unwrap();
    assert!(
        !yaml.contains("mode:"),
        "identity_create yaml must not emit mode: got {yaml:?}"
    );
}

#[test]
fn mcp_identity_create_missing_identity_errors() {
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
                "name": "identity_create",
                "arguments": {
                    "type": "human"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

// ---------- remargin_kind surface (rem-49w0) ----------

#[test]
fn mcp_comment_accepts_remargin_kind_and_persists_to_yaml() {
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
                    "file": "doc.md",
                    "content": "tagged body",
                    "remargin_kind": ["question", "todo"]
                }
            }
        }),
    );
    assert!(!is_tool_error(&response));
    let id = extract_tool_text(&response)["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let raw = system.read_to_string(&base.join("doc.md")).unwrap();
    assert!(raw.contains(&format!("id: {id}")));
    assert!(
        raw.contains("remargin_kind: [question, todo]"),
        "MCP-written kinds should round-trip through YAML: {raw}"
    );
}

#[test]
fn mcp_comments_filters_by_kind() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");
    let config = test_config();

    for (content, kinds) in [
        ("first with question", vec!["question"]),
        ("todo content", vec!["todo"]),
    ] {
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
                        "content": content,
                        "remargin_kind": kinds,
                    }
                }
            }),
        );
        assert!(!is_tool_error(&resp));
    }

    let resp = call(
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
                    "file": "doc.md",
                    "remargin_kind": ["todo"]
                }
            }
        }),
    );
    let body = extract_tool_text(&resp);
    let comments = body["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    assert!(comments[0]["content"].as_str().unwrap().contains("todo"));
}

#[test]
fn mcp_query_kind_filter_or_semantics() {
    let base = Path::new("/vault");
    let system = MockSystem::new()
        .with_file(base.join("a.md").as_path(), b"# a\n")
        .unwrap()
        .with_file(base.join("b.md").as_path(), b"# b\n")
        .unwrap();
    let config = test_config();

    // Seed comments directly via the core API so we skip MCP boilerplate.
    let pos = InsertPosition::Append;
    let kinds_q = vec![String::from("question")];
    let kinds_t = vec![String::from("todo")];
    let mut p1 = CreateCommentParams::new("a1", &pos);
    p1.remargin_kind = &kinds_q;
    create_comment(&system, &base.join("a.md"), &config, &p1).unwrap();
    let mut p2 = CreateCommentParams::new("b1", &pos);
    p2.remargin_kind = &kinds_t;
    create_comment(&system, &base.join("b.md"), &config, &p2).unwrap();

    let resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 3_i32,
            "method": "tools/call",
            "params": {
                "name": "query",
                "arguments": {
                    "path": ".",
                    "expanded": true,
                    "remargin_kind": ["question", "todo"]
                }
            }
        }),
    );
    let body = extract_tool_text(&resp);
    let results = body["results"].as_array().unwrap();
    let mut ids: Vec<&str> = results
        .iter()
        .flat_map(|r| {
            r["comments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap())
        })
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids.len(),
        2,
        "OR filter should surface both comments: {ids:?}"
    );
}

#[test]
fn mcp_edit_with_kind_replaces_stored_list() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");
    let config = test_config();

    let create = call(
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
                    "content": "body",
                    "remargin_kind": ["question"]
                }
            }
        }),
    );
    let id = extract_tool_text(&create)["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let edit = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "edit",
                "arguments": {
                    "file": "doc.md",
                    "id": id,
                    "content": "updated body",
                    "remargin_kind": ["todo"]
                }
            }
        }),
    );
    assert!(!is_tool_error(&edit));

    let raw = system.read_to_string(&base.join("doc.md")).unwrap();
    assert!(raw.contains("remargin_kind: [todo]"));
    assert!(!raw.contains("remargin_kind: [question]"));
}

#[test]
fn mcp_comment_after_heading_resolves_section_path() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_HEADINGS);
    let config = test_config();

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
                    "content": "Anchored after the P3 heading.",
                    "after_heading": "P3."
                }
            }
        }),
    );
    assert!(!is_tool_error(&resp), "{resp:?}");
    let new_id = String::from(extract_tool_text(&resp)["id"].as_str().unwrap());

    let raw = system.read_to_string(&base.join("doc.md")).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    let p3_line = lines
        .iter()
        .position(|l| l.trim_start().starts_with("## P3."))
        .unwrap();
    let new_block_line = lines
        .iter()
        .position(|l| l.contains(&format!("id: {new_id}")))
        .unwrap();
    // Comment block lands strictly after the P3 heading line.
    assert!(
        new_block_line > p3_line,
        "expected new comment block (line {new_block_line}) after P3 heading (line {p3_line})"
    );
}

#[test]
fn mcp_comment_after_heading_path_disambiguates_duplicate_subheadings() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_HEADINGS);
    let config = test_config();

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
                    "content": "Anchored after Activity > A10.",
                    "after_heading": "Activity epic tests > A10."
                }
            }
        }),
    );
    assert!(!is_tool_error(&resp), "{resp:?}");
    let new_id = String::from(extract_tool_text(&resp)["id"].as_str().unwrap());

    let raw = system.read_to_string(&base.join("doc.md")).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    let a10_line = lines
        .iter()
        .position(|l| l.trim_start().starts_with("## A10."))
        .unwrap();
    let p11_line = lines
        .iter()
        .position(|l| l.trim_start().starts_with("## P11."))
        .unwrap();
    let new_block_line = lines
        .iter()
        .position(|l| l.contains(&format!("id: {new_id}")))
        .unwrap();
    assert!(new_block_line > a10_line);
    assert!(new_block_line < p11_line);
}

#[test]
fn mcp_comment_after_heading_no_match_errors_without_writing() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_HEADINGS);
    let config = test_config();

    let before = system.read_to_string(&base.join("doc.md")).unwrap();

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
                    "content": "Should not be written.",
                    "after_heading": "Z9. nonexistent"
                }
            }
        }),
    );
    assert!(is_tool_error(&resp));
    let after = system.read_to_string(&base.join("doc.md")).unwrap();
    assert_eq!(before, after, "doc must be unchanged on resolver failure");
}

#[test]
fn mcp_batch_after_heading_inserts_each_op_at_its_anchor() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_HEADINGS);
    let config = test_config();

    let resp = call(
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
                        { "content": "after A10",
                          "after_heading": "Activity epic tests > A10." },
                        { "content": "after P3",
                          "after_heading": "P3." }
                    ]
                }
            }
        }),
    );
    assert!(!is_tool_error(&resp), "{resp:?}");
    let ids = extract_tool_text(&resp)["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| String::from(v.as_str().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);

    let raw = system.read_to_string(&base.join("doc.md")).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    let position_of_id = |id: &str| {
        lines
            .iter()
            .position(|l| l.contains(&format!("id: {id}")))
            .unwrap()
    };
    let a10_line = lines
        .iter()
        .position(|l| l.trim_start().starts_with("## A10."))
        .unwrap();
    let p3_line = lines
        .iter()
        .position(|l| l.trim_start().starts_with("## P3."))
        .unwrap();
    let id0_line = position_of_id(&ids[0]);
    let id1_line = position_of_id(&ids[1]);
    assert!(id0_line > a10_line);
    assert!(id1_line > p3_line);
}

#[test]
fn mcp_batch_rejects_multiple_anchors_per_op() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_HEADINGS);
    let config = test_config();

    let resp = call(
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
                        { "content": "x",
                          "after_heading": "P3.",
                          "after_line": 5_i32 }
                    ]
                }
            }
        }),
    );
    assert!(is_tool_error(&resp));
}

/// `mv` MCP tool moves a file and reports the documented outcome
/// shape (rem-0j2x / T44).
#[test]
fn mcp_mv_renames_file() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_dir(base)
        .unwrap()
        .with_file(base.join("a.md"), b"hello mcp")
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
                "name": "mv",
                "arguments": {
                    "src": "a.md",
                    "dst": "b.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["bytes_moved"].as_u64().unwrap(), 9_u64);
    assert!(!result["overwritten"].as_bool().unwrap());
    assert!(!result["noop_same_path"].as_bool().unwrap());
    assert!(!result["fallback_copy"].as_bool().unwrap());
}

/// `mv` MCP tool refuses an existing destination without `force`.
#[test]
fn mcp_mv_refuses_existing_destination_without_force() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_dir(base)
        .unwrap()
        .with_file(base.join("a.md"), b"src")
        .unwrap()
        .with_file(base.join("b.md"), b"dst")
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
                "name": "mv",
                "arguments": {
                    "src": "a.md",
                    "dst": "b.md"
                }
            }
        }),
    );
    assert!(is_tool_error(&response));
}

/// `mv` MCP tool with `force = true` overwrites an existing
/// destination and reports `overwritten = true`.
#[test]
fn mcp_mv_force_overwrites_destination() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_dir(base)
        .unwrap()
        .with_file(base.join("a.md"), b"new")
        .unwrap()
        .with_file(base.join("b.md"), b"old")
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
                "name": "mv",
                "arguments": {
                    "src": "a.md",
                    "dst": "b.md",
                    "force": true
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["overwritten"].as_bool().unwrap());
}

/// `plan mv` MCP tool surfaces the documented `mv_diff` shape with
/// `would_commit = true` for a clean projection.
#[test]
fn mcp_plan_mv_emits_mv_diff() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_dir(base)
        .unwrap()
        .with_file(base.join("a.md"), b"plan me")
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
                "name": "plan",
                "arguments": {
                    "op": "mv",
                    "src": "a.md",
                    "dst": "b.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["op"].as_str().unwrap(), "mv");
    assert!(result["would_commit"].as_bool().unwrap());
    let mv_diff = &result["mv_diff"];
    assert!(mv_diff["src_exists"].as_bool().unwrap());
    assert!(!mv_diff["dst_exists"].as_bool().unwrap());
    assert!(!mv_diff["noop_same_path"].as_bool().unwrap());
}
