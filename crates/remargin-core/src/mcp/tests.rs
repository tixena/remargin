//! MCP server tests.
//!
//! Tests use the `process_request` function to exercise the JSON-RPC layer
//! without actual stdin/stdout I/O.

use core::fmt::Write as _;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::codecs::gif::GifEncoder;
use image::codecs::png::PngEncoder;
use image::{Frame, Rgb, RgbImage, Rgba, RgbaImage};
use os_shim::System as _;
use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use crate::config::identity::IdentityFlags;
use crate::config::registry::Registry;
use crate::config::{Mode, ResolvedConfig};
use crate::mcp;
use crate::operations::batch::OP_FIELDS;
use crate::operations::projections::PLAN_OP_FIELDS;
use crate::operations::{CreateCommentParams, create_comment};
use crate::parser::{self, AuthorType};
use crate::permissions::pretool_install::{HOOK_MATCHER, HOOK_SUBCOMMAND};
use crate::writer::InsertPosition;

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

/// Four shapes: a fresh broadcast, a broadcast the caller acked, one directed to the caller and
/// one directed to someone else.
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

/// Two top-level sections, each holding a sibling heading with the same label.
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

fn test_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Agent),
        identity: Some(String::from("tester")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn system_with_doc(base: &Path, filename: &str, content: &str) -> MemorySystem {
    let path = base.join(filename);
    MemorySystem::new()
        .with_file(&path, content.as_bytes())
        .unwrap()
}

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

/// Drive a JSON-RPC request against a persistent [`super::SessionState`] so
/// adaptive state (spill cap, last-response size) carries across calls.
fn call_session(
    system: &dyn os_shim::System,
    base_dir: &Path,
    config: &ResolvedConfig,
    session: &mut super::SessionState,
    request: &Value,
) -> Value {
    let request_str = serde_json::to_string(request).unwrap();
    let response_str =
        super::process_request_with_session(system, base_dir, config, session, &request_str)
            .unwrap()
            .unwrap();
    serde_json::from_str(&response_str).unwrap()
}

/// A `tools/call` request for `search` over `pattern`, with an optional page
/// `limit`.
fn search_request(pattern: &str, limit: Option<i64>) -> Value {
    let mut arguments = json!({ "pattern": pattern });
    if let Some(lim) = limit {
        arguments["limit"] = json!(lim);
    }
    json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": { "name": "search", "arguments": arguments }
    })
}

/// A `tools/call` request for `report_spill`, with an optional explicit `size`.
fn report_spill_request(size: Option<i64>) -> Value {
    let arguments = size.map_or_else(|| json!({}), |s| json!({ "size": s }));
    json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": { "name": "report_spill", "arguments": arguments }
    })
}

/// A document body with `n` distinct `needle` lines (one match per line).
fn needle_doc(n: usize) -> String {
    let mut doc = String::from("# Needles\n\n");
    for i in 0..n {
        let _ = writeln!(doc, "needle {i}");
    }
    doc
}

/// Count compact match rows across every file group in a `search` result.
fn compact_row_count(result: &Value) -> usize {
    result["files"].as_array().map_or(0, |files| {
        files
            .iter()
            .map(|f| f["matches"].as_array().map_or(0, Vec::len))
            .sum()
    })
}

fn extract_tool_text(response: &Value) -> Value {
    let result = &response["result"];
    let content = result["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

/// Extract the raw, unparsed `text` string from an MCP tool result's first
/// content block — used to assert minified vs pretty serialization.
fn extract_tool_raw_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn is_tool_error(response: &Value) -> bool {
    response["result"]["isError"].as_bool().unwrap_or(false)
}

#[test]
fn initialize_returns_capabilities() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    const EXPECTED_TOOLS: &[&str] = &[
        "ack",
        "activity",
        "batch",
        "comment",
        "comments",
        "cp",
        "delete",
        "doctor",
        "edit",
        "get",
        "get_image",
        "identity_create",
        "lint",
        "ls",
        "metadata",
        "mv",
        "permissions_check",
        "permissions_show",
        "plan",
        "prompt_delete",
        "prompt_list",
        "prompt_resolve",
        "prompt_set",
        "purge",
        "query",
        "react",
        "replace",
        "reply",
        "report_spill",
        "rm",
        "sandbox_add",
        "sandbox_list",
        "sandbox_remove",
        "search",
        "sign",
        "verify",
        "whoami",
        "write",
    ];
    const CLI_ONLY_TOOLS: &[&str] = &["claude_restrict", "claude_unrestrict"];

    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    assert_eq!(tools.len(), EXPECTED_TOOLS.len());

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    for expected in EXPECTED_TOOLS {
        assert!(
            names.contains(expected),
            "missing MCP tool: {expected}; got: {names:?}"
        );
    }
    for cli_only in CLI_ONLY_TOOLS {
        assert!(
            !names.contains(cli_only),
            "{cli_only} must not appear on the MCP surface; got: {names:?}"
        );
    }
}

#[test]
fn tools_list_all_have_input_schema() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    assert_ne!(result["id"].as_str().unwrap(), "");
}

#[test]
fn comments_lists_created_comment() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome text.\n");
    let config = test_config();

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
    let comments = rows_as_objects(&result);
    assert_eq!(comments.len(), 1_usize);
    assert_eq!(comments[0]["id"].as_str().unwrap(), created_id);
    assert_eq!(comments[0]["author"], "tester");
    assert_eq!(comments[0]["content"], "First comment");
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
fn batch_kinds_are_found_by_the_kind_filter() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Repro\n\n## Target\n\nBody text.\n");
    let config = test_config();

    let batch = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "batch", "arguments": {
                "file": "doc.md",
                "operations": [
                    { "content": "Tagged batch op.", "after_heading": "Repro > Target", "kind": ["decision-item"] },
                    { "content": "Untagged batch op.", "after_heading": "Repro > Target" }
                ]
            }}
        }),
    );
    let ids = extract_tool_text(&batch)["ids"].clone();

    let filtered = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 2_i32, "method": "tools/call",
            "params": { "name": "comments", "arguments": {
                "file": "doc.md",
                "kind": ["decision-item"]
            }}
        }),
    );
    let found: Vec<Value> = rows_as_objects(&extract_tool_text(&filtered))
        .iter()
        .map(|cm| cm["id"].clone())
        .collect();
    assert_eq!(found, [ids[0].clone()]);
}

#[test]
fn batch_refuses_an_invalid_kind_before_writing() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nBody text.\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "batch", "arguments": {
                "file": "doc.md",
                "operations": [
                    { "content": "Fine." },
                    { "content": "Bad tag.", "kind": "decision-item" }
                ]
            }}
        }),
    );

    assert!(is_tool_error(&response));
    assert!(tool_error_text(&response).contains("batch op[1]"));
    assert_eq!(
        system.read_to_string(&base.join("doc.md")).unwrap(),
        "# Hello\n\nBody text.\n"
    );
}

/// A field missing from the sub-op schema is one agents are never told they can send.
#[test]
fn batch_op_schema_declares_every_comment_field() {
    let base = Path::new("/docs");
    let response = call(
        &MemorySystem::new(),
        base,
        &test_config(),
        &json!({ "jsonrpc": "2.0", "id": 1_i32, "method": "tools/list", "params": {} }),
    );
    let tools = response["result"]["tools"].as_array().unwrap();
    let schema = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .map(|t| t["inputSchema"].clone())
            .unwrap()
    };
    let comment = schema("comment");
    let batch = schema("batch");
    let op_fields = batch["properties"]["operations"]["items"]["properties"]
        .as_object()
        .unwrap();

    let missing: Vec<&String> = comment["properties"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|key| key.as_str() != "file")
        .filter(|key| !op_fields.contains_key(*key))
        .collect();
    assert!(missing.is_empty(), "batch ops lack {missing:?}");
}

/// A field the schema advertises but the parser refuses would fail every call that uses it.
#[test]
fn batch_op_schemas_declare_exactly_the_accepted_fields() {
    let base = Path::new("/docs");
    let response = call(
        &MemorySystem::new(),
        base,
        &test_config(),
        &json!({ "jsonrpc": "2.0", "id": 1_i32, "method": "tools/list", "params": {} }),
    );
    let tools = response["result"]["tools"].as_array().unwrap();
    let declared = |tool: &str, array: &str| -> Vec<String> {
        let schema = tools
            .iter()
            .find(|t| t["name"] == tool)
            .map(|t| t["inputSchema"].clone())
            .unwrap();
        let mut keys: Vec<String> = schema["properties"][array]["items"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    let accepted = |fields: &[&str]| -> Vec<String> {
        let mut keys: Vec<String> = fields.iter().map(|key| String::from(*key)).collect();
        keys.sort();
        keys
    };

    assert_eq!(declared("batch", "operations"), accepted(OP_FIELDS));
    assert_eq!(declared("plan", "ops"), accepted(PLAN_OP_FIELDS));
}

#[test]
fn batch_refuses_an_unknown_field_before_writing() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nBody text.\n");
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "batch", "arguments": {
                "file": "doc.md",
                "operations": [
                    { "content": "Fine." },
                    { "content": "Typo.", "after_headng": "Hello" }
                ]
            }}
        }),
    );

    assert!(is_tool_error(&response));
    assert!(tool_error_text(&response).contains("batch op[1]: unknown field `after_headng`"));
    assert_eq!(
        system.read_to_string(&base.join("doc.md")).unwrap(),
        "# Hello\n\nBody text.\n"
    );
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

    let raw = extract_tool_raw_text(&response);
    assert!(
        !raw.contains('\n'),
        "compact payload must be minified: {raw}"
    );

    let result = extract_tool_text(&response);
    let cols = result["match_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 4_usize);
    assert_eq!(cols[0], "line");
    assert_eq!(cols[1], "location");
    assert_eq!(cols[2], "text");
    assert_eq!(cols[3], "comment_id");

    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 1_usize);
    assert_eq!(files[0]["path"].as_str().unwrap(), "doc.md");
    let rows = files[0]["matches"].as_array().unwrap();
    assert_eq!(rows.len(), 1_usize);
    let row = rows[0].as_array().unwrap();
    assert_eq!(row[0], 3_i32);
    assert_eq!(row[1], "body");
    assert!(row[2].as_str().unwrap().contains("notification"));
    assert!(row[3].is_null(), "body comment_id must be null: {row:?}");
}

#[test]
fn search_limit_offset_envelope_carries_total() {
    let base = Path::new("/docs");
    let system = system_with_doc(
        base,
        "doc.md",
        "# Hello\n\nneedle 1\nneedle 2\nneedle 3\nneedle 4\nneedle 5\n",
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
                    "pattern": "needle",
                    "limit": 2_i32,
                    "offset": 1_i32
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 1_usize);
    let rows = files[0]["matches"].as_array().unwrap();
    assert_eq!(rows.len(), 2_usize);
    assert_eq!(result["total"], 5_i32);
    assert_eq!(rows[0].as_array().unwrap()[0], 4_i32);
}

#[test]
fn report_spill_infers_size_from_last_result() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", &needle_doc(100));
    let config = test_config();
    let mut session = super::SessionState::default();
    assert_eq!(session.spill_cap, super::DEFAULT_SPILL_CAP);

    let response = call_session(
        &system,
        base,
        &config,
        &mut session,
        &search_request("needle", None),
    );
    let result = extract_tool_text(&response);
    assert_eq!(compact_row_count(&result), 100_usize);
    assert!(result.get("effective_limit").is_none());
    let learned = session.last_response_size;
    assert!(learned > 0);
    assert!(learned < super::DEFAULT_SPILL_CAP);

    let spill = call_session(
        &system,
        base,
        &config,
        &mut session,
        &report_spill_request(None),
    );
    let spill_result = extract_tool_text(&spill);
    assert_eq!(spill_result["inferred"], true);
    assert_eq!(spill_result["spill_cap"].as_u64().unwrap(), learned as u64);
    assert_eq!(session.spill_cap, learned);
    assert!(session.spill_cap < super::DEFAULT_SPILL_CAP);
}

#[test]
fn report_spill_lowers_the_cap_and_never_raises() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", &needle_doc(100));
    let config = test_config();
    let mut session = super::SessionState::default();

    let low = call_session(
        &system,
        base,
        &config,
        &mut session,
        &report_spill_request(Some(1000)),
    );
    assert_eq!(
        extract_tool_text(&low)["spill_cap"].as_u64().unwrap(),
        1000_u64
    );
    assert_eq!(session.spill_cap, 1000_usize);

    let high = call_session(
        &system,
        base,
        &config,
        &mut session,
        &report_spill_request(Some(50_000)),
    );
    let high_result = extract_tool_text(&high);
    assert_eq!(high_result["reported_size"].as_u64().unwrap(), 50_000_u64);
    assert_eq!(high_result["spill_cap"].as_u64().unwrap(), 1000_u64);
    assert_eq!(session.spill_cap, 1000_usize);

    call_session(
        &system,
        base,
        &config,
        &mut session,
        &search_request("needle", None),
    );
    assert!(session.last_response_size > 1000);
    call_session(
        &system,
        base,
        &config,
        &mut session,
        &report_spill_request(None),
    );
    assert_eq!(session.spill_cap, 1000_usize);
}

#[test]
fn search_page_sized_under_cap() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", &needle_doc(100));
    let config = test_config();

    let mut tight = super::SessionState::default();
    call_session(
        &system,
        base,
        &config,
        &mut tight,
        &report_spill_request(Some(1500)),
    );
    let tight_page = extract_tool_text(&call_session(
        &system,
        base,
        &config,
        &mut tight,
        &search_request("needle", None),
    ));
    let tight_n = compact_row_count(&tight_page);
    assert_eq!(tight_page["total"].as_u64().unwrap(), 100_u64);
    assert!((1..100).contains(&tight_n));
    assert_eq!(
        usize::try_from(tight_page["effective_limit"].as_u64().unwrap()).unwrap(),
        tight_n
    );

    let mut loose = super::SessionState::default();
    call_session(
        &system,
        base,
        &config,
        &mut loose,
        &report_spill_request(Some(6000)),
    );
    let loose_page = extract_tool_text(&call_session(
        &system,
        base,
        &config,
        &mut loose,
        &search_request("needle", None),
    ));
    let loose_n = compact_row_count(&loose_page);
    assert_eq!(loose_page["total"].as_u64().unwrap(), 100_u64);
    assert!(loose_n > tight_n);

    let mut caller = super::SessionState::default();
    let bounded = extract_tool_text(&call_session(
        &system,
        base,
        &config,
        &mut caller,
        &search_request("needle", Some(3)),
    ));
    assert_eq!(compact_row_count(&bounded), 3_usize);
    assert!(bounded.get("effective_limit").is_none());
    assert_eq!(bounded["total"].as_u64().unwrap(), 100_u64);
}

#[test]
fn search_compact_groups_matches_by_file_in_page_order() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_file(Path::new("/docs/a.md"), b"needle one\nneedle two\n")
        .unwrap()
        .with_file(Path::new("/docs/b.md"), b"needle three\n")
        .unwrap();
    let config = test_config();

    let response = call(&system, base, &config, &search_request("needle", None));
    let result = extract_tool_text(&response);

    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 2_usize, "one group per file: {result}");
    assert_eq!(files[0]["path"].as_str().unwrap(), "a.md");
    assert_eq!(files[0]["matches"].as_array().unwrap().len(), 2_usize);
    assert_eq!(files[1]["path"].as_str().unwrap(), "b.md");
    assert_eq!(files[1]["matches"].as_array().unwrap().len(), 1_usize);
    assert_eq!(result["total"].as_u64().unwrap(), 3_u64);
    assert_eq!(result["match_cols"].as_array().unwrap().len(), 4_usize);
}

#[test]
fn search_compact_context_widens_rows_and_cols() {
    let base = Path::new("/docs");
    let system = system_with_doc(
        base,
        "doc.md",
        "line a\nline b\nTARGET here\nline d\nline e\n",
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
                "arguments": { "pattern": "TARGET", "context": 2_i32 }
            }
        }),
    );
    let result = extract_tool_text(&response);

    let cols = result["match_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 6_usize);
    assert_eq!(cols[4], "before");
    assert_eq!(cols[5], "after");

    let row = result["files"][0]["matches"][0].as_array().unwrap();
    assert_eq!(row.len(), 6_usize);
    assert_eq!(row[1], "body");
    assert!(row[3].is_null(), "body comment_id null: {row:?}");
    assert_eq!(
        row[4].as_array().unwrap(),
        &vec![json!("line a"), json!("line b")]
    );
    assert_eq!(
        row[5].as_array().unwrap(),
        &vec![json!("line d"), json!("line e")]
    );
}

#[test]
fn search_compact_comment_match_carries_comment_id() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    let response = call(&system, base, &config, &search_request("Original", None));
    let result = extract_tool_text(&response);

    let row = result["files"][0]["matches"][0].as_array().unwrap();
    assert_eq!(row[1], "comment");
    assert_eq!(row[3].as_str().unwrap(), "aaa");
}

#[test]
fn replace_rewrites_body_via_mcp() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nThe foo system.\n");
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
                "name": "replace",
                "arguments": {
                    "pattern": "foo",
                    "replacement": "bar",
                    "path": "doc.md"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert_eq!(result["total_replacements"], 1_i32);
    assert_eq!(result["files_changed"], 1_i32);
    assert_eq!(result["files_failed"], 0_i32);
    assert_eq!(result["dry_run"], false);

    let after = system.read_to_string(Path::new("/docs/doc.md")).unwrap();
    assert!(after.contains("The bar system."));
}

#[test]
fn replace_requires_explicit_path() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "foo\n");
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
                "name": "replace",
                "arguments": {
                    "pattern": "foo",
                    "replacement": "bar"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let after = system.read_to_string(Path::new("/docs/doc.md")).unwrap();
    assert_eq!(after, "foo\n");
}

#[test]
fn ls_lists_directory() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
fn get_returns_links_array() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_file(base.join("notes.md"), b"See [[Target]] for details.")
        .unwrap()
        .with_file(base.join("Target.md"), b"---\ntitle: The Target\n---\n# T")
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
                "name": "get",
                "arguments": { "path": "notes.md" }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["content"].as_str().unwrap().contains("[[Target]]"));
    assert_eq!(
        result["links_cols"],
        json!(["alias", "lines", "target", "title"])
    );
    let links = result["links"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    let row = links[0].as_array().unwrap();
    assert!(row[0].is_null(), "absent alias is null: {row:?}");
    assert_eq!(row[1], json!([1_i32]));
    assert_eq!(row[2], "Target");
    assert_eq!(row[3], "The Target");
}

#[test]
fn get_compact_line_numbers_minified() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_file(base.join("notes.md"), b"See [[Target]].\nMore text.\n")
        .unwrap()
        .with_file(base.join("Target.md"), b"---\ntitle: The Target\n---\n# T")
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
                "name": "get",
                "arguments": { "path": "notes.md", "line_numbers": true }
            }
        }),
    );

    let raw = extract_tool_raw_text(&response);
    assert!(
        !raw.contains('\n'),
        "compact payload must be minified: {raw}"
    );

    let result: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(result["start_line"], 1_i32);
    let lines = result["lines"].as_array().unwrap();
    assert!(lines[0].is_string(), "lines are bare strings: {lines:?}");
    assert!(lines[0].as_str().unwrap().contains("[[Target]]"));
    assert_eq!(
        result["links_cols"],
        json!(["alias", "lines", "target", "title"])
    );
    let link_rows = result["links"].as_array().unwrap();
    assert_eq!(link_rows.len(), 1);
    let row = link_rows[0].as_array().unwrap();
    assert_eq!(row.len(), 4, "count/path dropped: {row:?}");
    assert!(row[0].is_null(), "absent alias is null: {row:?}");
    assert_eq!(row[2], "Target");
    assert_eq!(row[3], "The Target");
    assert!(result.get("content").is_none());
    assert!(result["elapsed_ms"].is_number());
}

#[test]
fn get_compact_no_line_numbers_minified() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_file(base.join("notes.md"), b"See [[Target]].\nMore text.\n")
        .unwrap()
        .with_file(base.join("Target.md"), b"---\ntitle: The Target\n---\n# T")
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
                "name": "get",
                "arguments": { "path": "notes.md" }
            }
        }),
    );

    let raw = extract_tool_raw_text(&response);
    assert!(
        !raw.contains('\n'),
        "compact payload must be minified: {raw}"
    );

    let result: Value = serde_json::from_str(&raw).unwrap();
    assert!(result["content"].as_str().unwrap().contains("[[Target]]"));
    assert_eq!(
        result["links_cols"],
        json!(["alias", "lines", "target", "title"])
    );
    assert!(result.get("start_line").is_none());
    assert!(result.get("lines").is_none());
    let row = result["links"][0].as_array().unwrap();
    assert_eq!(row[2], "Target");
    assert!(result["elapsed_ms"].is_number());
}

#[test]
fn get_lone_start_line_returns_tail_not_whole_file() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_file(base.join("notes.md"), b"one\ntwo\nthree\nfour\nfive\n")
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
                "name": "get",
                "arguments": { "path": "notes.md", "start_line": 3_i32, "line_numbers": true }
            }
        }),
    );

    let result: Value = serde_json::from_str(&extract_tool_raw_text(&response)).unwrap();
    assert_eq!(
        result["start_line"], 3_i32,
        "tail must start at requested line"
    );
    let lines = result["lines"].as_array().unwrap();
    assert_eq!(lines[0], "three");
    assert!(
        !lines.iter().any(|l| l == "one" || l == "two"),
        "lone start_line must drop the head, got {lines:?}"
    );
}

#[test]
fn get_lone_end_line_returns_head_not_whole_file() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_file(base.join("notes.md"), b"one\ntwo\nthree\nfour\nfive\n")
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
                "name": "get",
                "arguments": { "path": "notes.md", "end_line": 2_i32, "line_numbers": true }
            }
        }),
    );

    let result: Value = serde_json::from_str(&extract_tool_raw_text(&response)).unwrap();
    assert_eq!(result["start_line"], 1_i32);
    let lines = result["lines"].as_array().unwrap();
    assert!(
        !lines
            .iter()
            .any(|l| l == "three" || l == "four" || l == "five"),
        "lone end_line must drop the tail, got {lines:?}"
    );
}

/// A minified `get` stays minified and a pretty `metadata` stays pretty; both keep `elapsed_ms`.
#[test]
fn injector_preserves_payload_style() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "notes.md", "Body one.\nBody two.\n");
    let config = test_config();

    let get_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": { "name": "get", "arguments": { "path": "notes.md" } }
        }),
    );
    let get_raw = extract_tool_raw_text(&get_resp);
    assert!(!get_raw.contains('\n'), "get stays minified: {get_raw}");
    assert!(
        get_raw.contains("\"elapsed_ms\":"),
        "elapsed_ms injected without pretty spacing: {get_raw}"
    );

    let meta_resp = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": { "name": "metadata", "arguments": { "path": "notes.md" } }
        }),
    );
    let meta_raw = extract_tool_raw_text(&meta_resp);
    assert!(meta_raw.contains('\n'), "metadata stays pretty: {meta_raw}");
    assert!(
        meta_raw.contains("\"elapsed_ms\""),
        "elapsed_ms injected into pretty payload: {meta_raw}"
    );
}

#[test]
fn unknown_method_returns_error() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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

/// The tool error points the caller at the CLI.
#[test]
fn claude_restrict_tool_dispatch_rejected() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
                "name": "claude_restrict",
                "arguments": { "path": "src/secret" }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let content = response["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("not available via MCP"),
        "expected refusal pointing to CLI, got: {text}"
    );
    assert!(text.contains("remargin claude restrict"), "got: {text}");
}

/// The tool error points the caller at the CLI.
#[test]
fn claude_unrestrict_tool_dispatch_rejected() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
                "name": "claude_unrestrict",
                "arguments": { "path": "src/secret" }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let content = response["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("not available via MCP"),
        "expected refusal pointing to CLI, got: {text}"
    );
    assert!(text.contains("remargin claude unrestrict"), "got: {text}");
}

/// The error points the caller at the CLI.
#[test]
fn plan_claude_restrict_op_rejected_via_mcp() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
                "arguments": { "op": "claude_restrict" }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let content = response["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("not available via MCP"),
        "expected refusal pointing to CLI, got: {text}"
    );
    assert!(
        text.contains("remargin plan claude restrict"),
        "got: {text}"
    );
}

/// The error points the caller at the CLI.
#[test]
fn plan_claude_unrestrict_op_rejected_via_mcp() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
                "arguments": { "op": "claude_unrestrict" }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let content = response["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("not available via MCP"),
        "expected refusal pointing to CLI, got: {text}"
    );
    assert!(
        text.contains("remargin plan claude unrestrict"),
        "got: {text}"
    );
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
    assert_eq!(
        result["errors"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );
}

#[test]
fn verify_checks_checksum_integrity() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nText.\n");
    let config = test_config();

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
    assert_eq!(results[0]["signature"], "missing");
    assert!(result["ok"].as_bool().unwrap(), "verify should pass");
}

#[test]
fn verify_escalates_to_realm_strict_mode_when_caller_is_open() {
    // BUG: verifying a strict-realm file under the caller's open mode lets an unsigned comment by
    // a registered participant pass; verify must escalate to the realm's mode first.

    let unsigned_doc = "\
# Realm doc

```remargin
---
id: u01
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";
    let alice_active_yaml = "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAtestalicekey
  caller:
    type: agent
    status: active
    pubkeys: []
";
    let base = Path::new("/parent");
    let system = MemorySystem::new()
        .with_dir(Path::new("/parent/realm"))
        .unwrap()
        .with_file(
            Path::new("/parent/.remargin.yaml"),
            b"mode: open\nidentity: caller\ntype: agent\n",
        )
        .unwrap()
        .with_file(
            Path::new("/parent/realm/.remargin.yaml"),
            b"mode: strict\nidentity: realm-owner\ntype: agent\n",
        )
        .unwrap()
        .with_file(
            Path::new("/parent/realm/.remargin-registry.yaml"),
            alice_active_yaml.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/parent/realm/file.md"), unsigned_doc.as_bytes())
        .unwrap();

    let registry: Registry = serde_yaml::from_str(alice_active_yaml).unwrap();

    let caller_cfg = ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Agent),
        identity: Some(String::from("caller")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: Some(registry),
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    };

    let response = call(
        &system,
        base,
        &caller_cfg,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "verify",
                "arguments": { "file": "realm/file.md" }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);
    assert!(results[0]["checksum_ok"].as_bool().unwrap());
    assert_eq!(results[0]["signature"], "missing");

    assert!(
        !result["ok"].as_bool().unwrap(),
        "verify must escalate to the realm's strict mode for files inside it; \
         an unsigned comment by a registered-active participant must report ok=false"
    );
}

#[test]
fn verify_keeps_open_verdict_when_no_stricter_subrealm_exists() {
    let unsigned_doc = "\
# Doc

```remargin
---
id: u01
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";
    let base = Path::new("/parent");
    let system = MemorySystem::new()
        .with_file(
            Path::new("/parent/.remargin.yaml"),
            b"mode: open\nidentity: caller\ntype: agent\n",
        )
        .unwrap()
        .with_file(Path::new("/parent/file.md"), unsigned_doc.as_bytes())
        .unwrap();

    let alice_active_yaml = "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAtestalicekey
";
    let registry: Registry = serde_yaml::from_str(alice_active_yaml).unwrap();

    let caller_cfg = ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Agent),
        identity: Some(String::from("caller")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: Some(registry),
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    };

    let response = call(
        &system,
        base,
        &caller_cfg,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "verify",
                "arguments": { "file": "file.md" }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(
        result["ok"].as_bool().unwrap(),
        "no stricter sub-realm exists; open-mode verdict (Missing is neutral) must stand"
    );
}

#[test]
fn metadata_returns_document_info() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome text.\n");
    let config = test_config();

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
    assert_eq!(result["binary"], false);
    assert_eq!(result["mime"], "text/markdown");
    assert!(result["path"].is_string());
    assert!(result["size_bytes"].is_number());
}

#[test]
fn get_binary_returns_resource_block() {
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

    let content = response["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "content is a resource + metadata pair");

    assert_eq!(content[0]["type"], "resource");
    let resource = &content[0]["resource"];
    assert_eq!(resource["mimeType"], "image/png");
    let uri = resource["uri"].as_str().unwrap();
    assert!(
        uri.starts_with("file://"),
        "uri is a file:// pointer, got {uri}"
    );
    let blob = resource["blob"].as_str().unwrap();
    assert!(!blob.starts_with("data:"), "blob must be bare base64");
    // base64 of "fake-png-bytes"
    assert_eq!(blob, "ZmFrZS1wbmctYnl0ZXM=");
    let decoded = BASE64_STANDARD.decode(blob).unwrap();
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        "fake-png-bytes",
        "blob decodes to the original file bytes"
    );

    assert_eq!(content[1]["type"], "text");
    let metadata: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["binary"], true);
    assert_eq!(metadata["mime"], "image/png");
    assert!(metadata["path"].is_string());
    assert!(metadata["size_bytes"].is_number());
    assert!(metadata.get("elapsed_ms").is_some(), "elapsed_ms injected");
    assert!(metadata["elapsed_ms"].is_u64());
}

#[test]
fn get_binary_records_nonzero_response_size() {
    let base = Path::new("/docs");
    let payload = "x".repeat(4096);
    let system = system_with_doc(base, "blob.png", &payload);
    let config = test_config();
    let mut session = super::SessionState::default();

    let response = call_session(
        &system,
        base,
        &config,
        &mut session,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get",
                "arguments": { "path": "blob.png", "binary": true }
            }
        }),
    );

    let blob_len = response["result"]["content"][0]["resource"]["blob"]
        .as_str()
        .unwrap()
        .len();
    assert!(
        session.last_response_size >= blob_len,
        "size {} should include the base64 resource blob ({blob_len} bytes)",
        session.last_response_size
    );
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

    let is_error = response["result"]["isError"].as_bool().unwrap_or(false);
    assert!(is_error, "binary get on .md should be an error response");
}

#[test]
fn metadata_binary_file_omits_markdown_fields() {
    let base = Path::new("/docs");
    // Only the extension drives mime detection, so the content is a placeholder.
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
    assert!(result.get("comment_count").is_none());
    assert!(result.get("line_count").is_none());
    assert!(result.get("pending_count").is_none());
    assert!(result.get("frontmatter").is_none());
}

#[test]
fn response_includes_jsonrpc_version() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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
    let comments = rows_as_objects(&result);

    let parent = comments.iter().find(|c| c["id"] == "aaa").unwrap();
    let reply = comments.iter().find(|c| c["id"] == reply_id).unwrap();

    let parent_line = parent["line"].as_u64().unwrap();
    let reply_line = reply["line"].as_u64().unwrap();

    assert!(
        reply_line > parent_line,
        "reply (line {reply_line}) should be after parent (line {parent_line})"
    );
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
    let comments = rows_as_objects(&result);

    let parent = comments.iter().find(|c| c["id"] == "aaa").unwrap();
    let reply = comments.iter().find(|c| c["id"] == reply_id).unwrap();

    let parent_line = parent["line"].as_u64().unwrap();
    let reply_line = reply["line"].as_u64().unwrap();

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
    let comments = rows_as_objects(&result);

    let parent = comments.iter().find(|c| c["id"] == "aaa").unwrap();
    let new_comment = comments.iter().find(|c| c["id"] == new_id).unwrap();

    let parent_line = parent["line"].as_u64().unwrap();
    let new_line = new_comment["line"].as_u64().unwrap();

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
    let comments = rows_as_objects(&result);
    let new_comment = comments.iter().find(|c| c["id"] == new_id).unwrap();
    let new_line = new_comment["line"].as_u64().unwrap();

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

/// A small solid-colour PNG for `get_image` round-trips.
fn solid_png(width: u32, height: u32, colour: [u8; 3]) -> Vec<u8> {
    let img: RgbImage = RgbImage::from_pixel(width, height, Rgb(colour));
    let mut bytes: Vec<u8> = Vec::new();
    img.write_with_encoder(PngEncoder::new(&mut bytes)).unwrap();
    bytes
}

/// A two-frame GIF: frame 1 is `first`, frame 2 is `second`. Used to prove
/// `get_image` returns frame 1.
fn two_frame_gif(width: u32, height: u32, first: [u8; 4], second: [u8; 4]) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        for colour in [first, second] {
            let frame: RgbaImage = RgbaImage::from_pixel(width, height, Rgba(colour));
            encoder.encode_frame(Frame::new(frame)).unwrap();
        }
    }
    bytes
}

/// Send a `get_image` `tools/call` and return the raw `result` object (its
/// `content` array is the point of these tests, so we don't flatten it).
fn get_image_result(system: &dyn os_shim::System, base: &Path, config: &ResolvedConfig) -> Value {
    let response = call(
        system,
        base,
        config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get_image",
                "arguments": { "path": "pic.png" }
            }
        }),
    );
    response["result"].clone()
}

#[test]
fn get_image_returns_image_content_block() {
    let base = Path::new("/docs");
    let png = solid_png(64, 48, [10, 120, 200]);
    let system = MemorySystem::new()
        .with_file(base.join("pic.png"), &png)
        .unwrap();
    let config = test_config();

    let result = get_image_result(&system, base, &config);
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "content is an image + metadata pair");

    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/png");
    let data = content[0]["data"].as_str().unwrap();
    assert!(
        !data.starts_with("data:"),
        "data must be bare base64, not a data: URI"
    );
    let decoded = BASE64_STANDARD.decode(data).unwrap();
    assert_eq!(&decoded[..8], b"\x89PNG\r\n\x1a\n", "data decodes to a PNG");

    assert_eq!(content[1]["type"], "text");
    let metadata: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["mime"], "image/png");
    assert!(metadata.get("elapsed_ms").is_some(), "elapsed_ms injected");
    assert!(metadata["elapsed_ms"].is_u64());
    assert!(metadata.get("content").is_none());
}

#[test]
fn get_image_gif_returns_first_frame() {
    let base = Path::new("/docs");
    let gif = two_frame_gif(16, 16, [255, 0, 0, 255], [0, 0, 255, 255]);
    let system = MemorySystem::new()
        .with_file(base.join("pic.gif"), &gif)
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
                "name": "get_image",
                "arguments": { "path": "pic.gif" }
            }
        }),
    );

    let content = response["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    let decoded = BASE64_STANDARD
        .decode(content[0]["data"].as_str().unwrap())
        .unwrap();
    let pixel = image::load_from_memory(&decoded).unwrap().to_rgb8();
    let Rgb([r, g, b]) = *pixel.get_pixel(0, 0);
    assert!(
        r > 200 && g < 60 && b < 60,
        "expected frame 1 (red), got rgb({r},{g},{b})"
    );
}

#[test]
fn get_image_records_nonzero_response_size() {
    let base = Path::new("/docs");
    let png = solid_png(64, 48, [10, 120, 200]);
    let system = MemorySystem::new()
        .with_file(base.join("pic.png"), &png)
        .unwrap();
    let config = test_config();
    let mut session = super::SessionState::default();

    let response = call_session(
        &system,
        base,
        &config,
        &mut session,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get_image",
                "arguments": { "path": "pic.png" }
            }
        }),
    );

    let data_len = response["result"]["content"][0]["data"]
        .as_str()
        .unwrap()
        .len();
    assert!(
        session.last_response_size >= data_len,
        "size {} should include the base64 image payload ({data_len} bytes)",
        session.last_response_size
    );
}

#[test]
fn mcp_query_comment_id_finds_doc() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    assert_eq!(results.as_slice(), [] as [Value; 0]);
}

fn query_base_path(system: &MemorySystem, base: &Path, path: &str) -> String {
    let response = call(
        system,
        base,
        &test_config(),
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "query",
                "arguments": { "path": path }
            }
        }),
    );
    String::from(extract_tool_text(&response)["base_path"].as_str().unwrap())
}

/// `base_path` is the join root for result paths: a file argument renders its parent directory.
#[test]
fn mcp_query_file_base_path_is_parent_directory() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_dir(Path::new("/docs/notes"))
        .unwrap()
        .with_file(
            Path::new("/docs/notes/nested.md"),
            DOC_WITH_COMMENT.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/docs/root.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap();

    assert_eq!(query_base_path(&system, base, "notes/nested.md"), "notes/");
    assert_eq!(query_base_path(&system, base, "root.md"), "./");
    assert_eq!(query_base_path(&system, base, "notes"), "notes/");
    assert_eq!(query_base_path(&system, base, "."), "./");
}

#[test]
fn mcp_query_file_base_path_joins_to_result_path() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_dir(Path::new("/docs/notes"))
        .unwrap()
        .with_file(
            Path::new("/docs/notes/nested.md"),
            DOC_WITH_COMMENT.as_bytes(),
        )
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
                "arguments": { "path": "notes/nested.md" }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let base_path = result["base_path"].as_str().unwrap();
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);
    let joined = Path::new(base_path).join(results[0]["path"].as_str().unwrap());
    assert_eq!(joined, Path::new("notes/nested.md"));
    assert!(system.is_file(&base.join(&joined)).unwrap());
}

#[test]
fn mcp_query_expanded_returns_comments() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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

    let raw = extract_tool_raw_text(&response);
    assert!(
        !raw.contains('\n'),
        "compact payload must be minified: {raw}"
    );

    let result = extract_tool_text(&response);
    let cols = result["comment_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 14_usize);
    assert_eq!(cols[0], "id");
    assert_eq!(cols[13], "content");
    assert!(
        !cols
            .iter()
            .any(|c| c == "checksum" || c == "signature" || c == "file")
    );

    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);

    let comments = results[0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2_usize);

    let row0 = comments[0].as_array().unwrap();
    assert_eq!(row0.len(), 14_usize);
    assert_eq!(row0[0].as_str().unwrap(), "ex1");
    assert_eq!(row0[2].as_str().unwrap(), "alice");
    assert_eq!(row0[3].as_str().unwrap(), "human");
    assert_eq!(row0[13].as_str().unwrap(), "Pending comment from alice.");
    assert!(row0[7].as_array().unwrap().contains(&json!("bob")));
    assert_eq!(row0[8].as_array().unwrap().as_slice(), [] as [Value; 0]);
    assert!(row0[5].is_null(), "reply_to null: {row0:?}");
    assert!(row0[10].is_null(), "remargin_kind null: {row0:?}");

    let row1 = comments[1].as_array().unwrap();
    assert_eq!(row1[0].as_str().unwrap(), "ex2");
    assert_eq!(row1[3].as_str().unwrap(), "agent");
    let acks = row1[8].as_array().unwrap();
    assert_eq!(acks.len(), 1_usize);
    assert!(acks[0].as_str().unwrap().contains('@'));
}

/// A comment-level filter narrows `comments` and `matched_count`; summary counts stay file-wide.
#[test]
fn mcp_query_reports_matched_count_beside_file_wide_counts() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
                "arguments": { "expanded": true, "pending": true }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);
    assert_eq!(results[0]["comment_count"].as_u64().unwrap(), 2_u64);
    assert_eq!(results[0]["matched_count"].as_u64().unwrap(), 1_u64);
    let comments = results[0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1_usize);
    assert_eq!(comments[0][0].as_str().unwrap(), "ex1");
}

#[test]
fn mcp_query_compact_include_integrity_widens_rows() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
                "arguments": { "expanded": true, "include_integrity": true }
            }
        }),
    );

    let result = extract_tool_text(&response);
    let cols = result["comment_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 16_usize);
    assert_eq!(cols[13], "checksum");
    assert_eq!(cols[14], "signature");
    assert_eq!(cols[15], "content");

    let comments = result["results"][0]["comments"].as_array().unwrap();
    let row0 = comments[0].as_array().unwrap();
    assert_eq!(row0.len(), 16_usize);
    assert_eq!(row0[13].as_str().unwrap(), "sha256:ex1");
    assert!(row0[14].is_null(), "unsigned signature is null: {row0:?}");
    assert_eq!(row0[15].as_str().unwrap(), "Pending comment from alice.");
}

/// A realm whose user-scope settings carry only the `PreToolUse` hook (so
/// enforcement is active but the `SessionStart` guard is missing) and whose
/// project-local settings carry a stale `Bash(remargin *)` deny. The full
/// doctor run trips both `SessionGuardMissing` and `LeftoverProjectedRule`.
fn doctor_two_finding_system() -> MemorySystem {
    let exe = "/opt/bin/remargin";
    let command = format!("{exe} {HOOK_SUBCOMMAND}");
    let user_settings = json!({
        "hooks": {
            "PreToolUse": [
                { "matcher": HOOK_MATCHER, "hooks": [ { "type": "command", "command": command } ] }
            ]
        }
    })
    .to_string();
    let local = json!({ "permissions": { "deny": ["Bash(remargin *)"] } }).to_string();
    MemorySystem::new()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_dir(Path::new("/r/.claude"))
        .unwrap()
        .with_file(Path::new(exe), b"binary")
        .unwrap()
        .with_file(
            Path::new("/home/u/.claude/settings.json"),
            user_settings.as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/r/.claude/settings.local.json"),
            local.as_bytes(),
        )
        .unwrap()
}

fn finding_kinds(report: &Value) -> Vec<String> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn doctor_call(system: &MemorySystem, arguments: &Value) -> Value {
    let config = test_config();
    call(
        system,
        Path::new("/r"),
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": { "name": "doctor", "arguments": arguments }
        }),
    )
}

/// Omitting `check` runs every check; naming one scopes the run to it alone.
#[test]
fn mcp_doctor_check_scopes_findings() {
    let system = doctor_two_finding_system();

    let full = extract_tool_text(&doctor_call(
        &system,
        &json!({ "user_settings_file": "/home/u/.claude/settings.json" }),
    ));
    let full_kinds = finding_kinds(&full);
    assert!(
        full_kinds.iter().any(|k| k == "session_guard_missing")
            && full_kinds.iter().any(|k| k == "leftover_projected_rule"),
        "default MCP run surfaces both findings: {full_kinds:?}",
    );

    let scoped = extract_tool_text(&doctor_call(
        &system,
        &json!({ "user_settings_file": "/home/u/.claude/settings.json", "check": "leftover-rules" }),
    ));
    assert_eq!(
        finding_kinds(&scoped),
        vec![String::from("leftover_projected_rule")],
        "scoped MCP run reports only the selected check",
    );
}

/// An unknown `check` is an `isError` response naming the slug, not a silent empty run.
#[test]
fn mcp_doctor_unknown_check_errors() {
    let system = doctor_two_finding_system();
    let response = doctor_call(
        &system,
        &json!({ "user_settings_file": "/home/u/.claude/settings.json", "check": "bogus" }),
    );
    assert!(
        response["result"]["isError"].as_bool().unwrap_or(false),
        "unknown check must be an error response: {response}",
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("unknown check `bogus`"),
        "error names the bad slug: {text}",
    );
}

/// Assert the three compact change rows (comment, ack, sandbox) carry the
/// right populated / null columns.
fn assert_activity_change_rows(rows: &[Value]) {
    assert_eq!(rows.len(), 3_usize);
    let comment = rows[0].as_array().unwrap();
    assert_eq!(comment.len(), 9_usize);
    assert_eq!(comment[1], "comment");
    assert_eq!(comment[2], "bob");
    assert_eq!(comment[3], "human");
    assert_eq!(comment[4], "c1");
    assert_eq!(comment[8], json!(["carol"]));

    let ack = rows[1].as_array().unwrap();
    assert_eq!(ack[1], "ack");
    assert_eq!(ack[4], "c1");
    assert!(ack[5].is_null(), "ack line_start null: {ack:?}");
    assert!(ack[6].is_null(), "ack line_end null: {ack:?}");
    assert!(ack[7].is_null(), "ack reply_to null: {ack:?}");
    assert!(ack[8].is_null(), "ack to null: {ack:?}");

    let sandbox = rows[2].as_array().unwrap();
    assert_eq!(sandbox[1], "sandbox");
    assert!(sandbox[4].is_null(), "sandbox comment_id null: {sandbox:?}");
    assert!(sandbox[8].is_null(), "sandbox to null: {sandbox:?}");
}

#[test]
fn mcp_activity_compact_columnar_minified() {
    let base = Path::new("/docs");
    let body = "---\ntitle: t\nsandbox:\n  - alice@2026-04-06T17:00:00-04:00\n---\n\n# Body\n\n```remargin\n---\nid: c1\nauthor: bob\ntype: human\nts: 2026-04-06T12:00:00-04:00\nto: [carol]\nchecksum: sha256:test\nack:\n  - carol@2026-04-06T14:00:00-04:00\n---\nHello.\n```\n";
    let system = MemorySystem::new()
        .with_file(
            Path::new("/docs/.remargin.yaml"),
            b"identity: tester\ntype: human\n",
        )
        .unwrap()
        .with_file(Path::new("/docs/note.md"), body.as_bytes())
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
                "name": "activity",
                "arguments": { "since": "2026-01-01T00:00:00-04:00" }
            }
        }),
    );

    let raw = extract_tool_raw_text(&response);
    assert!(
        !raw.contains('\n'),
        "compact payload must be minified: {raw}"
    );

    let result = extract_tool_text(&response);
    let cols = result["change_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 9_usize);
    assert_eq!(cols[0], "ts");
    assert_eq!(cols[1], "kind");
    assert_eq!(cols[8], "to");
    assert_eq!(result["cutoff_explicit"], json!(true));
    assert!(result["newest_ts_overall"].is_string());
    assert!(result["elapsed_ms"].is_number());

    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 1_usize);
    let file = &files[0];
    assert!(file["path"].as_str().unwrap().ends_with("note.md"));
    assert!(file["newest_ts"].is_string());
    assert_eq!(
        file["cutoff_applied"].as_str().unwrap(),
        "2026-01-01T00:00:00-04:00"
    );

    let rows = file["changes"].as_array().unwrap();
    assert_activity_change_rows(rows);
}

#[test]
fn mcp_query_summary_omits_comments() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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

    let raw = extract_tool_raw_text(&response);
    assert!(
        !raw.contains('\n'),
        "summary payload must be minified: {raw}"
    );

    let result = extract_tool_text(&response);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1_usize);

    assert!(results[0].get("comments").is_none());
}

#[test]
fn mcp_ack_without_file_resolves_from_tree() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
        .with_dir(Path::new("/docs/sub"))
        .unwrap()
        .with_file(Path::new("/docs/sub/a.md"), DOC_WITH_COMMENT.as_bytes())
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "tester");
}

#[test]
fn mcp_comment_auto_ack_omitted_acks_other_author() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

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
                    "content": "Reply with default auto_ack.",
                    "reply_to": "aaa"
                }
            }
        }),
    );

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert!(
        parent.ack.iter().any(|a| a.author == "tester"),
        "MCP smart default must ack when parent.author != caller; acks = {:?}",
        parent.ack,
    );
}

#[test]
fn mcp_comment_auto_ack_omitted_skips_self_authored_parent() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let mut config = test_config();
    config.identity = Some(String::from("eduardo"));

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
                    "content": "Reply to my own comment.",
                    "reply_to": "aaa"
                }
            }
        }),
    );

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert!(
        parent.ack.is_empty(),
        "MCP smart default must NOT ack the caller's own comment; acks = {:?}",
        parent.ack,
    );
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

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "tester");
}

#[test]
fn mcp_rm_deletes_file() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let system = MemorySystem::new().with_dir(Path::new("/docs")).unwrap();
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
    let system = MemorySystem::new();
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
    let system = MemorySystem::new().with_dir(Path::new("/docs")).unwrap();
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
    let system = MemorySystem::new()
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
    let system = MemorySystem::new().with_dir(Path::new("/docs")).unwrap();
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
    let system = MemorySystem::new()
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
    let base = Path::new("/docs");
    let original = "\
---
title: Test
description: ''
author: eduardo
created: 2026-04-18T00:00:00+00:00
remargin_last_activity: null
---

body A
body B
body C
";
    let system = MemorySystem::new()
        .with_file(Path::new("/docs/doc.md"), original.as_bytes())
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
                    "content": "BODY B NEW",
                    "start_line": 10_i32,
                    "end_line": 10_i32
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
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
fn seed_real_comment(base: &Path, filename: &str) -> (MemorySystem, ResolvedConfig, String) {
    let path = base.join(filename);
    let system = MemorySystem::new()
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
                    "ids": [id]
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
    assert_eq!(report["comments"]["preserved"].as_array().unwrap().len(), 1);

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
    let base = Path::new("/docs");
    // A sibling file makes `/docs` exist in the `MemorySystem`; the sandbox resolver needs the
    // parent directory even when the target is missing.
    let system = MemorySystem::new()
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
    let base = Path::new("/docs");
    let path = base.join("data.txt");
    let system = MemorySystem::new()
        .with_file(&path, b"old bytes\n")
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
fn mcp_purge_recursive_clears_every_md_file() {
    let base = Path::new("/realm");
    let path_a = base.join("a.md");
    let path_b = base.join("notes/b.md");
    let system = MemorySystem::new()
        .with_dir(base)
        .unwrap()
        .with_dir(base.join("notes"))
        .unwrap()
        .with_file(&path_a, b"# A\n")
        .unwrap()
        .with_file(&path_b, b"# B\n")
        .unwrap();
    let config = test_config();
    let _id_a: String = create_comment(
        &system,
        &path_a,
        &config,
        &CreateCommentParams::new("seed a", &InsertPosition::Append),
    )
    .unwrap();
    let _id_b: String = create_comment(
        &system,
        &path_b,
        &config,
        &CreateCommentParams::new("seed b", &InsertPosition::Append),
    )
    .unwrap();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "purge",
                "arguments": { "file": ".", "recursive": true }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "recursive purge should succeed: {response}"
    );
    let payload = extract_tool_text(&response);
    assert_eq!(payload["comments_removed"], 2_u64);
    let purged = payload["purged"].as_array().unwrap();
    assert_eq!(purged.len(), 2);
}

#[test]
fn mcp_purge_dir_without_recursive_errors() {
    let base = Path::new("/realm");
    let system = MemorySystem::new()
        .with_dir(base)
        .unwrap()
        .with_file(base.join("a.md"), b"# A\n")
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
                "name": "purge",
                "arguments": { "file": "." }
            }
        }),
    );

    assert!(
        is_tool_error(&response),
        "purge on a directory without `recursive` must error: {response}"
    );
}

#[test]
fn mcp_plan_purge_recursive_emits_purge_dir_diff() {
    let base = Path::new("/realm");
    let path_a = base.join("a.md");
    let system = MemorySystem::new()
        .with_dir(base)
        .unwrap()
        .with_file(&path_a, b"# A\n")
        .unwrap();
    let config = test_config();
    let _id_a: String = create_comment(
        &system,
        &path_a,
        &config,
        &CreateCommentParams::new("seed a", &InsertPosition::Append),
    )
    .unwrap();
    let before_bytes = system.read_to_string(&path_a).unwrap();

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
                "arguments": { "op": "purge", "file": ".", "recursive": true }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "plan recursive purge should succeed: {response}"
    );
    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "purge");
    assert_eq!(report["would_commit"], json!(true));
    let diff = &report["purge_dir_diff"];
    assert!(diff.is_object(), "purge_dir_diff missing: {report}");
    let files = diff["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["outcome"], "would_purge");
    assert_eq!(files[0]["comments_removed"], 1_u64);

    let after_bytes = system.read_to_string(&path_a).unwrap();
    assert_eq!(before_bytes, after_bytes);
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
    let system = MemorySystem::new();
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

/// `identity_create` is exempt: its `identity`/`type`/`key` name the new identity, not the caller.
#[test]
fn no_identity_flags_on_any_mcp_tool_schema() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
        if name == "identity_create" {
            continue;
        }
        let props = &tool["inputSchema"]["properties"];
        for field in ["config_path", "identity", "key", "type"] {
            assert!(
                props.get(field).is_none_or(Value::is_null),
                "tool {name} must not advertise identity-declaration field {field}"
            );
        }
        let not = &tool["inputSchema"].get("not");
        assert!(
            not.is_none() || not.unwrap().is_null(),
            "tool {name} must not carry a top-level `not` exclusivity clause"
        );
    }
}

#[test]
fn no_mode_or_dry_run_in_any_schema() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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

#[test]
fn every_mcp_tool_rejects_identity_flags() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");
    let config = test_config();

    let cases: &[(&str, Value)] = &[
        ("ack", json!({"file": "doc.md", "ids": ["abc"]})),
        ("activity", json!({})),
        (
            "batch",
            json!({"file": "doc.md", "operations": [{"content": "x"}]}),
        ),
        ("comment", json!({"file": "doc.md", "content": "x"})),
        ("comments", json!({"file": "doc.md"})),
        ("delete", json!({"file": "doc.md", "ids": ["abc"]})),
        (
            "edit",
            json!({"file": "doc.md", "id": "abc", "content": "y"}),
        ),
        ("get", json!({"path": "doc.md"})),
        ("lint", json!({"path": "doc.md"})),
        ("ls", json!({})),
        ("metadata", json!({"path": "doc.md"})),
        ("mv", json!({"file": "doc.md", "id": "abc", "to": "end"})),
        ("permissions_check", json!({"op": "comment"})),
        ("permissions_show", json!({})),
        (
            "plan",
            json!({"op": "comment", "file": "doc.md", "content": "x"}),
        ),
        ("prompt_delete", json!({})),
        ("prompt_list", json!({})),
        ("prompt_resolve", json!({})),
        ("prompt_set", json!({"name": "p", "prompt": "do thing"})),
        ("purge", json!({"file": "doc.md"})),
        ("query", json!({})),
        (
            "react",
            json!({"file": "doc.md", "id": "abc", "emoji": "+1"}),
        ),
        (
            "reply",
            json!({"file": "doc.md", "parent_id": "abc", "content": "x"}),
        ),
        ("rm", json!({"path": "doc.md"})),
        ("sandbox_add", json!({"files": ["doc.md"]})),
        ("sandbox_list", json!({})),
        ("sandbox_remove", json!({"files": ["doc.md"]})),
        ("search", json!({"query": "x"})),
        ("sign", json!({"file": "doc.md"})),
        ("verify", json!({"file": "doc.md"})),
        ("whoami", json!({})),
        ("write", json!({"path": "doc.md", "content": "hi"})),
    ];

    for (tool, base_args) in cases {
        for flag in ["config_path", "identity", "key", "type"] {
            let mut args = base_args.clone();
            args[flag] = json!("anything");
            let response = call(
                &system,
                base,
                &config,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1_i32,
                    "method": "tools/call",
                    "params": {"name": tool, "arguments": args}
                }),
            );
            assert!(
                is_tool_error(&response),
                "tool {tool} did not reject flag {flag}: {response}"
            );
            let msg = response["result"]["content"][0]["text"].as_str().unwrap();
            assert!(
                msg.contains("identity flag") && msg.contains(flag),
                "tool {tool} returned wrong diagnostic for {flag}: {msg}"
            );
        }
    }
}

/// Hosts branch on `error_kind: "mcp_identity_flag_rejected"`, not on the message text.
#[test]
fn identity_flag_rejection_is_structured() {
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
                    "identity": "alice"
                }
            }
        }),
    );
    assert!(is_tool_error(&response));
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let payload: Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["error_kind"], "mcp_identity_flag_rejected");
    assert_eq!(payload["tool"], "comment");
    assert_eq!(payload["flag"], "identity");
    assert!(payload["headline"].as_str().unwrap().contains("identity"));
}

/// A config whose resolved identity is `type: human`, as when the walk
/// falls through to a user-level `~/.remargin.yaml`.
fn human_config() -> ResolvedConfig {
    ResolvedConfig {
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo-burgos")),
        ..test_config()
    }
}

/// A human identity from the config walk is refused at dispatch, with the recovery path named.
#[test]
fn mcp_human_identity_rejects_comment() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");

    let response = call(
        &system,
        base,
        &human_config(),
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": { "file": "doc.md", "content": "x" }
            }
        }),
    );
    assert!(is_tool_error(&response));
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let payload: Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["error_kind"], "mcp_human_identity_rejected");
    assert_eq!(payload["tool"], "comment");
    assert_eq!(payload["identity"], "eduardo-burgos");
    let headline = payload["headline"].as_str().unwrap();
    assert!(headline.contains("identity_create"), "{headline}");
    assert!(headline.contains("permission"), "{headline}");
}

/// The ban is blanket: a read under a human identity is refused too.
#[test]
fn mcp_human_identity_rejects_read_only_get() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");

    let response = call(
        &system,
        base,
        &human_config(),
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get",
                "arguments": { "path": "doc.md" }
            }
        }),
    );
    assert!(is_tool_error(&response));
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let payload: Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["error_kind"], "mcp_human_identity_rejected");
    assert_eq!(payload["tool"], "get");
}

/// They are the diagnosis and recovery path the rejection points at.
#[test]
fn mcp_human_identity_allows_whoami_and_identity_create() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
        .with_file(
            Path::new("/docs/.remargin.yaml"),
            b"identity: eduardo-burgos\ntype: human\n",
        )
        .unwrap();
    let config = human_config();

    let whoami = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": { "name": "whoami", "arguments": {} }
        }),
    );
    assert!(!is_tool_error(&whoami), "{whoami}");

    let create = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "identity_create",
                "arguments": { "identity": "docs_agent", "type": "agent" }
            }
        }),
    );
    assert!(!is_tool_error(&create), "{create}");
}

#[test]
fn mcp_agent_identity_passes() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");

    let response = call(
        &system,
        base,
        &test_config(),
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get",
                "arguments": { "path": "doc.md" }
            }
        }),
    );
    assert!(!is_tool_error(&response), "{response}");
}

/// No resolved identity is not the human case: the guard must not fire.
#[test]
fn mcp_no_identity_not_rejected_by_human_guard() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n");
    let config = ResolvedConfig {
        author_type: None,
        identity: None,
        ..test_config()
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
                "name": "get",
                "arguments": { "path": "doc.md" }
            }
        }),
    );
    if is_tool_error(&response) {
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("mcp_human_identity_rejected"), "{text}");
    }
}

#[test]
fn identity_create_keeps_identity_fields() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    let tool = tools
        .iter()
        .find(|t| t["name"] == "identity_create")
        .unwrap();
    let props = &tool["inputSchema"]["properties"];
    for field in ["identity", "key", "type"] {
        assert!(
            props[field].is_object(),
            "identity_create must expose {field} (names the new identity)"
        );
    }
}

#[test]
fn mcp_query_pending_includes_broadcast_rem_4j91() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let mut ids: Vec<&str> = comments.iter().map(|c| c[0].as_str().unwrap()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["brd_open", "dir_me", "dir_other"]);
}

#[test]
fn mcp_query_pending_for_me_uses_server_identity() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let ids: Vec<&str> = comments.iter().map(|c| c[0].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["dir_me"]);
}

#[test]
fn mcp_query_pending_broadcast_only_surfaces_unacked_broadcasts() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let ids: Vec<&str> = comments.iter().map(|c| c[0].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["brd_open"]);
}

#[test]
fn mcp_query_pending_for_me_and_broadcast_union() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
    let mut ids: Vec<&str> = comments.iter().map(|c| c[0].as_str().unwrap()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["brd_open", "dir_me"]);
}

#[test]
fn mcp_query_pending_for_me_errors_without_identity() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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
        trusted_roots: Vec::new(),
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

#[test]
fn mcp_identity_create_minimal_returns_yaml() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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

#[test]
fn mcp_whoami_returns_resolved_identity_from_walked_config() {
    let base = Path::new("/docs");
    let yaml = b"identity: alice\ntype: human\nassets_dir: assets\nmode: open\n" as &[u8];
    let system = MemorySystem::new()
        .with_file(base.join(".remargin.yaml"), yaml)
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
                "name": "whoami",
                "arguments": {}
            }
        }),
    );

    assert!(!is_tool_error(&response), "got error: {response}");
    let result = extract_tool_text(&response);
    assert_eq!(result["found"].as_bool(), Some(true));
    assert_eq!(result["identity"].as_str(), Some("alice"));
    assert_eq!(result["author_type"].as_str(), Some("human"));
    assert_eq!(result["mode"].as_str(), Some("open"));
    assert_eq!(
        result["path"].as_str(),
        Some("/docs/.remargin.yaml"),
        "expected path to point at the walked config"
    );
}

#[test]
fn mcp_whoami_with_no_config_returns_found_false() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
                "name": "whoami",
                "arguments": {}
            }
        }),
    );

    assert!(!is_tool_error(&response), "got error: {response}");
    let result = extract_tool_text(&response);
    assert_eq!(result["found"].as_bool(), Some(false));
    assert!(result.get("identity").is_none() || result["identity"].is_null());
}

#[test]
fn mcp_whoami_rejects_config_path() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
                "name": "whoami",
                "arguments": {
                    "config_path": "/other/.remargin.yaml"
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
    let msg = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(msg.contains("config_path"), "got: {msg}");
}

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
                    "kind": ["question", "todo"]
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
                        "kind": kinds,
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
                    "kind": ["todo"]
                }
            }
        }),
    );
    let body = extract_tool_text(&resp);
    let comments = rows_as_objects(&body);
    assert_eq!(comments.len(), 1);
    assert!(comments[0]["content"].as_str().unwrap().contains("todo"));
}

#[test]
fn mcp_query_kind_filter_or_semantics() {
    let base = Path::new("/vault");
    let system = MemorySystem::new()
        .with_file(base.join("a.md").as_path(), b"# a\n")
        .unwrap()
        .with_file(base.join("b.md").as_path(), b"# b\n")
        .unwrap();
    let config = test_config();

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
                    "kind": ["question", "todo"]
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
                .map(|c| c[0].as_str().unwrap())
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
                    "kind": ["question"]
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
                    "kind": ["todo"]
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

#[test]
fn mcp_batch_warns_against_the_op_whose_body_earned_the_note() {
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
                        { "content": "A clean single-line body." },
                        { "content": "See a5q for the field list." }
                    ]
                }
            }
        }),
    );
    assert!(!is_tool_error(&resp), "{resp:?}");

    let payload = extract_tool_text(&resp);
    assert_eq!(payload["ids"].as_array().unwrap().len(), 2);
    let warnings = payload["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["op"], 1_u64);
    assert_eq!(warnings[0]["line"], 1_u64);
    assert!(
        warnings[0]["message"].as_str().unwrap().contains("a5q"),
        "{warnings:?}"
    );
}

#[test]
fn mcp_batch_omits_warnings_when_every_body_reads_cleanly() {
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
                        { "content": "A clean single-line body." },
                        { "content": "Another one, equally clean." }
                    ]
                }
            }
        }),
    );
    assert!(!is_tool_error(&resp), "{resp:?}");

    let payload = extract_tool_text(&resp);
    assert_eq!(payload["ids"].as_array().unwrap().len(), 2);
    assert!(
        payload.get("warnings").is_none(),
        "a clean batch keeps the payload it has always had: {payload}"
    );
}

#[test]
fn mcp_mv_renames_file() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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

#[test]
fn mcp_mv_refuses_existing_destination_without_force() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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

#[test]
fn mcp_mv_force_overwrites_destination() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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

#[test]
fn mcp_plan_mv_emits_mv_diff() {
    let base = Path::new("/docs");
    let system = MemorySystem::new()
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

#[test]
fn mcp_mv_renames_directory() {
    let base = Path::new("/realm");
    let system = MemorySystem::new()
        .with_dir(base)
        .unwrap()
        .with_dir(base.join("notes"))
        .unwrap()
        .with_file(base.join("notes/a.md"), b"x")
        .unwrap()
        .with_file(base.join("notes/b.md"), b"yy")
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
                    "src": "notes",
                    "dst": "archive"
                }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "directory mv should succeed: {response}"
    );
    let result = extract_tool_text(&response);
    assert!(result["is_directory"].as_bool().unwrap());
    assert_eq!(result["nested_files_moved"].as_u64().unwrap(), 2_u64);
    assert!(!system.exists(&base.join("notes")).unwrap());
    assert!(system.is_dir(&base.join("archive")).unwrap());
}

#[test]
fn mcp_plan_mv_directory_emits_is_directory() {
    let base = Path::new("/realm");
    let system = MemorySystem::new()
        .with_dir(base)
        .unwrap()
        .with_dir(base.join("src"))
        .unwrap()
        .with_file(base.join("src/a.md"), b"x")
        .unwrap()
        .with_file(base.join("src/b.md"), b"yy")
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
                    "src": "src",
                    "dst": "dst"
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["would_commit"].as_bool().unwrap());
    let mv_diff = &result["mv_diff"];
    assert!(mv_diff["is_directory"].as_bool().unwrap());
    assert_eq!(mv_diff["nested_files_moved"].as_u64().unwrap(), 2_u64);
    assert!(mv_diff["src_exists"].as_bool().unwrap());
    assert!(!mv_diff["dst_exists"].as_bool().unwrap());

    assert!(system.is_dir(&base.join("src")).unwrap());
    assert!(!system.exists(&base.join("dst")).unwrap());
}

/// The subset gate admits an anomaly that was already on disk before the op.
#[test]
fn mcp_ack_succeeds_when_pre_existing_bad_checksum() {
    let base = Path::new("/docs");
    let bad_doc = "\
---
title: Doc
---

Body.

```remargin
---
id: abc
author: tester
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:0000000000000000000000000000000000000000000000000000000000000000
---
hello
```
";
    let system = MemorySystem::new()
        .with_file(base.join("a.md"), bad_doc.as_bytes())
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
                    "file": "a.md",
                    "ids": ["abc"]
                }
            }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "subset gate must allow ack when no new anomaly is introduced: {response}"
    );
}

#[test]
fn prompt_resolve_returns_nearest_block() {
    let base = Path::new("/vault");
    let system = MemorySystem::new()
        .with_dir(base.join("a/b"))
        .unwrap()
        .with_file(
            base.join(".remargin.yaml"),
            b"system_prompt:\n  name: outer\n  prompt: outer body\n",
        )
        .unwrap()
        .with_file(
            base.join("a/.remargin.yaml"),
            b"system_prompt:\n  name: inner\n  prompt: inner body\n",
        )
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
                "name": "prompt_resolve",
                "arguments": {
                    "file": "a/b/file.md"
                }
            }
        }),
    );

    let payload = extract_tool_text(&response);
    assert_eq!(payload["name"], "inner");
    assert_eq!(payload["prompt"], "inner body");
    assert_eq!(payload["is_default"], false);
    assert!(payload["source"].is_string());
}

#[test]
fn prompt_resolve_falls_through_to_default() {
    let base = Path::new("/vault");
    let system = MemorySystem::new().with_dir(base.join("a/b")).unwrap();
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
                "name": "prompt_resolve",
                "arguments": {
                    "file": "a/b/file.md"
                }
            }
        }),
    );

    let payload = extract_tool_text(&response);
    assert_eq!(payload["name"], "default");
    assert_eq!(payload["is_default"], true);
    assert!(payload["source"].is_null());
    assert!(
        payload["prompt"]
            .as_str()
            .unwrap()
            .contains("remargin skill")
    );
}

#[test]
fn prompt_resolve_absolute_and_relative_paths_match() {
    let base = Path::new("/vault");
    let system = MemorySystem::new()
        .with_dir(base.join("a"))
        .unwrap()
        .with_file(
            base.join("a/.remargin.yaml"),
            b"system_prompt:\n  name: a\n  prompt: body\n",
        )
        .unwrap();
    let config = test_config();

    let response_rel = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "prompt_resolve",
                "arguments": { "file": "a/file.md" }
            }
        }),
    );
    let response_abs = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "prompt_resolve",
                "arguments": { "file": "/vault/a/file.md" }
            }
        }),
    );

    let rel = extract_tool_text(&response_rel);
    let abs = extract_tool_text(&response_abs);
    assert_eq!(rel["name"], abs["name"]);
    assert_eq!(rel["prompt"], abs["prompt"]);
}

#[test]
fn prompt_set_runner_round_trips_and_clears() {
    let base = Path::new("/vault");
    let system = MemorySystem::new().with_dir(base.join("a")).unwrap();
    let config = test_config();

    let set_response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "prompt_set",
                "arguments": {
                    "folder": "a",
                    "name": "reviewer",
                    "prompt": "review",
                    "runner": "goose run -i -"
                }
            }
        }),
    );
    assert!(
        !is_tool_error(&set_response),
        "prompt_set failed: {set_response}"
    );

    let resolve_response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "prompt_resolve",
                "arguments": { "file": "a/file.md" }
            }
        }),
    );
    let resolved = extract_tool_text(&resolve_response);
    assert_eq!(resolved["runner"], "goose run -i -");

    let clear_response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 3_i32,
            "method": "tools/call",
            "params": {
                "name": "prompt_set",
                "arguments": {
                    "folder": "a",
                    "name": "reviewer",
                    "prompt": "review"
                }
            }
        }),
    );
    assert!(
        !is_tool_error(&clear_response),
        "prompt_set failed: {clear_response}"
    );

    let recheck_response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 4_i32,
            "method": "tools/call",
            "params": {
                "name": "prompt_resolve",
                "arguments": { "file": "a/file.md" }
            }
        }),
    );
    let rechecked = extract_tool_text(&recheck_response);
    assert!(rechecked["runner"].is_null(), "payload: {rechecked}");
}

#[test]
fn mcp_reply_acks_parent_when_authors_differ() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "parent_id": "aaa",
                    "content": "Reply via reply tool."
                }
            }
        }),
    );

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert!(
        parent.ack.iter().any(|a| a.author == "tester"),
        "reply smart default must ack when parent.author != caller; acks = {:?}",
        parent.ack,
    );
}

#[test]
fn mcp_reply_skips_ack_for_self_authored_parent() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let mut config = test_config();
    config.identity = Some(String::from("eduardo"));

    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "parent_id": "aaa",
                    "content": "Self-reply."
                }
            }
        }),
    );

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert!(
        parent.ack.is_empty(),
        "reply must NOT ack caller's own comment; acks = {:?}",
        parent.ack,
    );
}

#[test]
fn mcp_reply_auto_ack_true_forces_ack_on_self_authored() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let mut config = test_config();
    config.identity = Some(String::from("eduardo"));

    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "parent_id": "aaa",
                    "content": "Force ack via explicit auto_ack.",
                    "auto_ack": true
                }
            }
        }),
    );

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert!(
        parent.ack.iter().any(|a| a.author == "eduardo"),
        "auto_ack=true must force the ack; acks = {:?}",
        parent.ack,
    );
}

#[test]
fn mcp_reply_auto_ack_false_skips_other_author() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "parent_id": "aaa",
                    "content": "Skip the ack.",
                    "auto_ack": false
                }
            }
        }),
    );

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    let doc = parser::parse(&doc_content).unwrap();
    let parent = doc.find_comment("aaa").unwrap();
    assert!(
        parent.ack.is_empty(),
        "auto_ack=false must skip the ack; acks = {:?}",
        parent.ack,
    );
}

#[test]
fn mcp_reply_missing_parent_id_errors() {
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
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "content": "Missing parent."
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn mcp_reply_unknown_parent_errors() {
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
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "parent_id": "nope",
                    "content": "Unknown parent."
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn mcp_reply_sandbox_flag_stages_file() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "parent_id": "aaa",
                    "content": "Stage via sandbox.",
                    "sandbox": true
                }
            }
        }),
    );

    let doc_content = system.read_to_string(&base.join("doc.md")).unwrap();
    assert!(
        doc_content.contains("sandbox:") && doc_content.contains("tester"),
        "expected sandbox marker for caller in frontmatter; doc = {doc_content}",
    );
}

#[test]
fn mcp_plan_reply_op_projects_like_comment_with_reply_to() {
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
                    "op": "reply",
                    "file": "doc.md",
                    "parent_id": parent_id,
                    "content": "Plan reply.",
                    "auto_ack": true
                }
            }
        }),
    );

    let report = extract_tool_text(&response);
    assert_eq!(report["op"], "comment");
    assert_eq!(report["comments"]["added"].as_array().unwrap().len(), 1);
    let preserved_has_parent = report["comments"]["preserved"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some(parent_id.as_str()));
    assert!(preserved_has_parent, "expected parent in preserved set");
}

#[test]
fn mcp_plan_reply_op_missing_parent_id_errors() {
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
                    "op": "reply",
                    "file": "doc.md",
                    "content": "Plan reply with no parent."
                }
            }
        }),
    );

    assert!(is_tool_error(&response));
}

#[test]
fn mcp_tools_list_includes_reply_alphabetically_between_react_and_rm() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let react_idx = names.iter().position(|n| *n == "react").unwrap();
    let reply_idx = names.iter().position(|n| *n == "reply").unwrap();
    let rm_idx = names.iter().position(|n| *n == "rm").unwrap();
    assert!(
        react_idx < reply_idx && reply_idx < rm_idx,
        "expected react < reply < rm in tools/list; got order = {names:?}",
    );
}

#[test]
fn mcp_tools_list_descriptor_text_matches_spec() {
    let base = Path::new("/docs");
    let system = MemorySystem::new();
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
    let descriptors: Vec<(&str, &str)> = tools
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap(),
                t["description"].as_str().unwrap(),
            )
        })
        .collect();
    let lookup = |needle: &str| -> &str {
        descriptors
            .iter()
            .find(|(name, _)| *name == needle)
            .map(|(_, desc)| *desc)
            .unwrap()
    };

    let comment_desc = lookup("comment");
    assert!(
        comment_desc.contains("For two or more comments on the same file")
            && comment_desc.contains("use `batch`"),
        "comment descriptor must steer multi-comment loops to batch; got: {comment_desc}",
    );
    assert!(
        comment_desc.contains("Use `reply` (not this tool)"),
        "comment descriptor must point at reply for thread replies; got: {comment_desc}",
    );

    let batch_desc = lookup("batch");
    assert!(
        batch_desc.contains("PREFERRED for any time you'll post more than one comment"),
        "batch descriptor must be marked PREFERRED; got: {batch_desc}",
    );

    let write_desc = lookup("write");
    assert!(
        write_desc.contains("start_line/end_line"),
        "write descriptor must surface partial writes; got: {write_desc}",
    );

    let activity_desc = lookup("activity");
    assert!(
        activity_desc.starts_with("Call this BEFORE processing pending comments"),
        "activity descriptor must lead with the BEFORE guidance; got: {activity_desc}",
    );

    let reply_desc = lookup("reply");
    assert!(
        reply_desc.contains("PREFERRED way to respond to a comment"),
        "reply descriptor must be marked PREFERRED; got: {reply_desc}",
    );
    assert!(
        reply_desc.contains("Smart auto-ack default"),
        "reply descriptor must surface the smart auto-ack default; got: {reply_desc}",
    );
}

/// Text body of an MCP tool-error response.
fn tool_error_text(response: &Value) -> String {
    String::from(response["result"]["content"][0]["text"].as_str().unwrap())
}

fn fetch_comment(
    system: &dyn os_shim::System,
    base: &Path,
    config: &ResolvedConfig,
    id: &str,
) -> Value {
    let resp = call(
        system,
        base,
        config,
        &json!({
            "jsonrpc": "2.0", "id": 99_i32, "method": "tools/call",
            "params": { "name": "comments", "arguments": { "file": "doc.md" } }
        }),
    );
    rows_as_objects(&extract_tool_text(&resp))
        .into_iter()
        .find(|c| c["id"] == id)
        .unwrap_or(Value::Null)
}

/// Rebuild a `comments` payload's positional rows as objects keyed by
/// `comment_cols`, so tests can address cells by column name.
fn rows_as_objects(payload: &Value) -> Vec<Value> {
    let cols: Vec<&str> = payload["comment_cols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|col| col.as_str().unwrap())
        .collect();
    payload["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            Value::Object(
                cols.iter()
                    .zip(row.as_array().unwrap())
                    .map(|(col, cell)| (String::from(*col), cell.clone()))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn reply_auto_ack_false_to_other_without_reason_is_rejected() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "reply", "arguments": {
                "file": "doc.md", "parent_id": "aaa",
                "content": "A reply.", "auto_ack": false
            } }
        }),
    );

    assert!(is_tool_error(&response));
    let text = tool_error_text(&response);
    assert!(text.contains("ack_skip_reason"), "got: {text}");
    let parent = fetch_comment(&system, base, &config, "aaa");
    assert_eq!(
        parent["ack"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );
}

#[test]
fn reply_auto_ack_false_to_other_with_reason_succeeds_unacked() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "reply", "arguments": {
                "file": "doc.md", "parent_id": "aaa",
                "content": "A reply.", "auto_ack": false,
                "ack_skip_reason": "deferring until the build is green"
            } }
        }),
    );

    assert!(!is_tool_error(&response));
    let parent = fetch_comment(&system, base, &config, "aaa");
    assert_eq!(
        parent["ack"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );
}

#[test]
fn reply_auto_ack_false_to_own_comment_needs_no_reason() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    let posted = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "comment", "arguments": {
                "file": "doc.md", "content": "My own note."
            } }
        }),
    );
    let own_id = String::from(extract_tool_text(&posted)["id"].as_str().unwrap());

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 2_i32, "method": "tools/call",
            "params": { "name": "reply", "arguments": {
                "file": "doc.md", "parent_id": own_id,
                "content": "Self reply.", "auto_ack": false
            } }
        }),
    );

    assert!(
        !is_tool_error(&response),
        "self-reply must not require a reason"
    );
}

#[test]
fn reply_with_smart_default_still_acks_parent() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "reply", "arguments": {
                "file": "doc.md", "parent_id": "aaa", "content": "A reply."
            } }
        }),
    );

    assert!(!is_tool_error(&response));
    let parent = fetch_comment(&system, base, &config, "aaa");
    let ack = parent["ack"].as_array().unwrap();
    assert_eq!(ack.len(), 1);
    assert!(ack[0].as_str().unwrap().starts_with("tester@"), "{ack:?}");
}

#[test]
fn batch_reply_auto_ack_false_without_reason_rejects_whole_batch() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", DOC_WITH_COMMENT);
    let config = test_config();

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "tools/call",
            "params": { "name": "batch", "arguments": {
                "file": "doc.md",
                "operations": [
                    { "content": "Standalone note." },
                    { "content": "A reply.", "reply_to": "aaa", "auto_ack": false }
                ]
            } }
        }),
    );

    assert!(is_tool_error(&response));
    let text = tool_error_text(&response);
    assert!(text.contains("ack_skip_reason"), "got: {text}");
    let listing = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0", "id": 2_i32, "method": "tools/call",
            "params": { "name": "comments", "arguments": { "file": "doc.md" } }
        }),
    );
    let comments = extract_tool_text(&listing);
    assert_eq!(comments["comments"].as_array().unwrap().len(), 1);
}

/// The advice is an extra field on the successful result, not a refusal.
#[test]
fn comment_attaches_advice_for_bare_id_reference_without_failing() {
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
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "See a5q for the earlier decision."
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(
        result["id"].is_string() && !result["id"].as_str().unwrap().is_empty(),
        "the comment was still created: {result}"
    );
    let warnings = result["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1_usize, "{result}");
    assert_eq!(warnings[0]["line"].as_u64(), Some(1));
    assert!(
        warnings[0]["message"]
            .as_str()
            .unwrap()
            .contains("reads as a comment id"),
        "advisory wording: {result}"
    );
}

#[test]
fn comment_rejects_hard_wrapped_body_from_agent() {
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
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "This comment body is broken\nacross two lines by hand."
                }
            }
        }),
    );

    assert!(is_tool_error(&response), "{response}");
    let text = extract_tool_raw_text(&response);
    assert!(text.contains("comment body rejected"), "{text}");
}

#[test]
fn comment_omits_warnings_for_continuous_body() {
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
                "name": "comment",
                "arguments": {
                    "file": "doc.md",
                    "content": "One continuous line, however long it happens to run."
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["id"].is_string(), "{result}");
    assert!(
        result.get("warnings").is_none(),
        "clean body keeps the original payload: {result}"
    );
}

#[test]
fn reply_inherits_the_comment_advisory_pass() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome text.\n");
    let config = test_config();

    let created = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": { "file": "doc.md", "content": "Parent comment." }
            }
        }),
    );
    let parent_id = String::from(extract_tool_text(&created)["id"].as_str().unwrap());

    let response = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "reply",
                "arguments": {
                    "file": "doc.md",
                    "parent_id": parent_id,
                    "content": "See a5q for the earlier decision."
                }
            }
        }),
    );

    let result = extract_tool_text(&response);
    assert!(result["id"].is_string(), "the reply still posted: {result}");
    assert_eq!(
        result["warnings"].as_array().unwrap().len(),
        1_usize,
        "{result}"
    );
}

/// Only the style pass reports a bare-id reference, so a warning proves the edit ran it.
#[test]
fn edit_attaches_the_style_warn_tier_to_a_successful_result() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Hello\n\nSome text.\n");
    let config = test_config();

    let created = call(
        &system,
        base,
        &config,
        &json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": { "file": "doc.md", "content": "The first body." }
            }
        }),
    );
    let id = String::from(extract_tool_text(&created)["id"].as_str().unwrap());

    let response = call(
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
                    "content": "The recipient list is derived from the parent, as in ow6."
                }
            }
        }),
    );

    assert!(!is_tool_error(&response));
    let result = extract_tool_text(&response);
    let warnings = result["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1_usize, "{result}");
    assert!(
        warnings[0]["message"]
            .as_str()
            .unwrap()
            .contains("reads as a comment id"),
        "advisory wording: {result}"
    );
}

/// Strict realm whose registry admits only `alice`, so the server's own
/// resolved (identity-less) config is outside it.
fn strict_realm_system(base: &Path) -> MemorySystem {
    MemorySystem::new()
        .with_file(base.join(".remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            base.join(".remargin-registry.yaml"),
            b"participants:\n  alice:\n    type: human\n    status: active\n    pubkeys: []\n",
        )
        .unwrap()
        .with_file(base.join("doc.md"), b"# Read doc\n\nNeedle body text.\n")
        .unwrap()
}

#[test]
fn anonymous_reads_in_a_strict_realm_are_tool_errors() {
    let base = Path::new("/strict");
    let system = strict_realm_system(base);
    let config = ResolvedConfig::resolve(&system, base, &IdentityFlags::default(), None).unwrap();

    for (tool, arguments) in [
        ("get", json!({ "path": "doc.md" })),
        ("comments", json!({ "file": "doc.md" })),
        ("search", json!({ "pattern": "Needle" })),
        ("metadata", json!({ "path": "doc.md" })),
        ("lint", json!({ "file": "doc.md" })),
        ("query", json!({ "path": "." })),
        ("ls", json!({ "path": "." })),
        ("verify", json!({ "file": "doc.md" })),
    ] {
        let response = call(
            &system,
            base,
            &config,
            &json!({
                "jsonrpc": "2.0",
                "id": 1_i32,
                "method": "tools/call",
                "params": { "name": tool, "arguments": arguments }
            }),
        );

        assert!(
            is_tool_error(&response),
            "{tool} must refuse an anonymous caller in a strict realm: {response}"
        );
        let rendered = serde_json::to_string(&response).unwrap();
        assert!(
            rendered.contains("<anonymous>"),
            "{tool} must name the absent caller: {rendered}"
        );
        assert!(
            !rendered.contains("Needle body text"),
            "{tool} must not leak document text: {rendered}"
        );
    }
}

/// A `tools/call` request for `name` with `arguments`.
fn tool_request(name: &str, arguments: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

/// A document with `count` top-level comments under `# Notes`, the second
/// one addressed to `tester` (the test identity) and unacknowledged.
fn system_with_comments(base: &Path, count: usize) -> MemorySystem {
    let system = system_with_doc(base, "doc.md", "# Notes\n\nBody text.\n");
    let config = test_config();
    for index in 0..count {
        let mut arguments = json!({
            "file": "doc.md",
            "content": format!("Comment number {index}."),
            "after_heading": "Notes",
        });
        if index == 1 {
            arguments["to"] = json!(["tester"]);
        }
        let response = call(&system, base, &config, &tool_request("comment", &arguments));
        assert!(!is_tool_error(&response), "{response}");
    }
    system
}

#[test]
fn comments_pages_cover_every_comment_exactly_once() {
    let base = Path::new("/docs");
    let system = system_with_comments(base, 5);
    let config = test_config();

    let mut seen: Vec<String> = Vec::new();
    let mut offset = 0_usize;
    loop {
        let response = call(
            &system,
            base,
            &config,
            &tool_request(
                "comments",
                &json!({ "file": "doc.md", "offset": offset, "limit": 2_i32 }),
            ),
        );
        let payload = extract_tool_text(&response);
        assert_eq!(payload["total"], 5_i32);
        let rows = rows_as_objects(&payload);
        if rows.is_empty() {
            break;
        }
        offset += rows.len();
        seen.extend(
            rows.iter()
                .map(|row| String::from(row["id"].as_str().unwrap())),
        );
    }

    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(seen.len(), 5);
    assert_eq!(unique.len(), 5);
}

#[test]
fn comments_rows_are_minified_columns_with_kind() {
    let base = Path::new("/docs");
    let system = system_with_comments(base, 1);
    let response = call(
        &system,
        base,
        &test_config(),
        &tool_request("comments", &json!({ "file": "doc.md" })),
    );
    let raw = extract_tool_raw_text(&response);
    assert!(!raw.contains('\n'), "minified: {raw}");
    let payload = extract_tool_text(&response);
    let cols: Vec<&str> = payload["comment_cols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|col| col.as_str().unwrap())
        .collect();
    assert_eq!(
        cols,
        [
            "id",
            "line",
            "author",
            "author_type",
            "ts",
            "reply_to",
            "thread",
            "to",
            "ack",
            "reactions",
            "kind",
            "edited_at",
            "attachments",
            "content"
        ]
    );
    assert!(payload.get("effective_limit").is_none());
}

#[test]
fn comments_include_integrity_adds_checksum_and_signature_before_content() {
    let base = Path::new("/docs");
    let system = system_with_comments(base, 1);
    let response = call(
        &system,
        base,
        &test_config(),
        &tool_request(
            "comments",
            &json!({ "file": "doc.md", "include_integrity": true }),
        ),
    );
    let payload = extract_tool_text(&response);
    let cols = payload["comment_cols"].as_array().unwrap();
    let tail: Vec<&str> = cols[cols.len() - 3..]
        .iter()
        .map(|col| col.as_str().unwrap())
        .collect();
    assert_eq!(tail, ["checksum", "signature", "content"]);
}

#[test]
fn comments_pending_for_me_keeps_only_comments_addressed_to_the_caller() {
    let base = Path::new("/docs");
    let system = system_with_comments(base, 3);
    let response = call(
        &system,
        base,
        &test_config(),
        &tool_request(
            "comments",
            &json!({ "file": "doc.md", "pending_for_me": true }),
        ),
    );
    let payload = extract_tool_text(&response);
    assert_eq!(payload["total"], 1_i32);
    let rows = rows_as_objects(&payload);
    assert_eq!(rows[0]["content"], "Comment number 1.");
}

#[test]
fn comments_on_a_file_without_comments_is_an_empty_page() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Notes\n\nBody text.\n");
    let response = call(
        &system,
        base,
        &test_config(),
        &tool_request("comments", &json!({ "file": "doc.md" })),
    );
    let payload = extract_tool_text(&response);
    assert_eq!(payload["total"], 0_i32);
    assert_eq!(payload["comments"], json!([]));
}

#[test]
fn comments_page_is_cut_to_the_size_cap_and_says_so() {
    let base = Path::new("/docs");
    let system = system_with_comments(base, 6);
    let config = test_config();
    let mut session = super::SessionState::default();
    assert!(!is_tool_error(&call_session(
        &system,
        base,
        &config,
        &mut session,
        &report_spill_request(Some(400)),
    )));

    let response = call_session(
        &system,
        base,
        &config,
        &mut session,
        &tool_request("comments", &json!({ "file": "doc.md" })),
    );
    let payload = extract_tool_text(&response);
    let returned = payload["comments"].as_array().unwrap().len();
    assert_eq!(payload["total"], 6_i32);
    assert!(returned < 6, "{payload}");
    assert_eq!(payload["effective_limit"], json!(returned));
}

#[test]
fn query_pages_rows_across_files_and_regroups_them() {
    let base = Path::new("/docs");
    let system = system_with_comments(base, 2);
    let config = test_config();
    system
        .write(&base.join("other.md"), b"# Other\n\nBody.\n")
        .unwrap();
    let response = call(
        &system,
        base,
        &config,
        &tool_request(
            "comment",
            &json!({ "file": "other.md", "content": "Other note." }),
        ),
    );
    assert!(!is_tool_error(&response), "{response}");

    let first = extract_tool_text(&call(
        &system,
        base,
        &config,
        &tool_request("query", &json!({ "limit": 2_i32 })),
    ));
    assert_eq!(first["total"], 3_i32);
    let rows_on = |page: &Value| -> usize {
        page["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|result| result["comments"].as_array().unwrap().len())
            .sum()
    };
    assert_eq!(rows_on(&first), 2);

    let second = extract_tool_text(&call(
        &system,
        base,
        &config,
        &tool_request("query", &json!({ "offset": 2_i32, "limit": 2_i32 })),
    ));
    assert_eq!(rows_on(&second), 1);
    assert_eq!(second["results"].as_array().unwrap().len(), 1);
}

#[test]
fn activity_reports_total_and_pages_changes() {
    let base = Path::new("/docs");
    let system = system_with_comments(base, 3);
    system
        .write(
            &base.join(".remargin.yaml"),
            b"identity: tester\ntype: human\n",
        )
        .unwrap();
    let response = call(
        &system,
        base,
        &test_config(),
        &tool_request(
            "activity",
            &json!({ "path": "doc.md", "since": "2000-01-01T00:00:00Z", "limit": 1_i32 }),
        ),
    );
    let payload = extract_tool_text(&response);
    assert_eq!(payload["total"], 3_i32);
    let changes: usize = payload["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["changes"].as_array().unwrap().len())
        .sum();
    assert_eq!(changes, 1);
}

#[test]
fn get_reports_total_lines_and_cuts_the_window_to_the_size_cap() {
    let base = Path::new("/docs");
    let body = (1_i32..=40_i32)
        .map(|n| format!("Line {n} of a long note.\n"))
        .collect::<Vec<String>>()
        .concat();
    let system = system_with_doc(base, "long.md", &body);
    let config = test_config();
    let mut session = super::SessionState::default();

    let whole = extract_tool_text(&call_session(
        &system,
        base,
        &config,
        &mut session,
        &tool_request("get", &json!({ "path": "long.md" })),
    ));
    assert_eq!(whole["total_lines"], 41_i32);
    assert!(whole.get("effective_end_line").is_none());

    assert!(!is_tool_error(&call_session(
        &system,
        base,
        &config,
        &mut session,
        &report_spill_request(Some(300)),
    )));
    let cut = extract_tool_text(&call_session(
        &system,
        base,
        &config,
        &mut session,
        &tool_request("get", &json!({ "path": "long.md", "line_numbers": true })),
    ));
    let end = cut["effective_end_line"].as_u64().unwrap();
    assert_eq!(
        cut["lines"].as_array().unwrap().len(),
        usize::try_from(end).unwrap()
    );
    assert!(end < 41);
}

#[test]
fn every_tool_refuses_an_argument_it_does_not_declare() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Notes\n\nBody text.\n");
    let config = test_config();
    let tools = call(
        &system,
        base,
        &config,
        &json!({ "jsonrpc": "2.0", "id": 1_i32, "method": "tools/list", "params": {} }),
    );
    for tool in tools["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let response = call(
            &system,
            base,
            &config,
            &tool_request(name, &json!({ "not_an_argument": true })),
        );
        assert!(is_tool_error(&response), "{name}: {response}");
        assert!(
            tool_error_text(&response).starts_with(&format!(
                "{name}: unknown argument `not_an_argument`; accepted: "
            )),
            "{name}: {response}"
        );
    }
}

#[test]
fn comment_refuses_the_storage_name_remargin_kind() {
    let base = Path::new("/docs");
    let system = system_with_doc(base, "doc.md", "# Notes\n\nBody text.\n");
    let response = call(
        &system,
        base,
        &test_config(),
        &tool_request(
            "comment",
            &json!({ "file": "doc.md", "content": "Tagged.", "remargin_kind": ["todo"] }),
        ),
    );
    assert!(is_tool_error(&response));
    assert!(
        tool_error_text(&response).starts_with("comment: unknown argument `remargin_kind`"),
        "{response}"
    );
}
