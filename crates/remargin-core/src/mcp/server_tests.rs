//! The generated server: the tool list, the refusals, and the session state `report_spill`
//! lowers.

extern crate alloc;

use alloc::collections::BTreeSet;
use core::fmt::Write as _;
use std::path::{Path, PathBuf};

use image::codecs::png::PngEncoder;
use image::{Rgb, RgbImage};
use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use crate::config::{Mode, ResolvedConfig};
use crate::mcp::handlers::Server;
use crate::mcp::host;
use crate::parser::AuthorType;

const BASE: &str = "/docs";

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

const DOC_PLAIN: &str = "# Plain\n\nneedle in a haystack\n";

const TEST_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";

const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

fn agent_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Agent),
        identity: Some(String::from("tester")),
        ignore: Vec::new(),
        key_path: Some(PathBuf::from("/keys/ed25519")),
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn human_config() -> ResolvedConfig {
    ResolvedConfig {
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo-burgos")),
        ..agent_config()
    }
}

fn solid_png() -> Vec<u8> {
    let image = RgbImage::from_pixel(8, 8, Rgb([10, 120, 200]));
    let mut bytes: Vec<u8> = Vec::new();
    image
        .write_with_encoder(PngEncoder::new(&mut bytes))
        .unwrap();
    bytes
}

fn needles(count: usize) -> String {
    let mut doc = String::from("# Needles\n\n");
    for index in 0..count {
        let _ = writeln!(doc, "needle {index}");
    }
    doc
}

fn realm() -> MemorySystem {
    let base = Path::new(BASE);
    MemorySystem::new()
        .with_file(base.join("a.md"), DOC_WITH_COMMENT.as_bytes())
        .unwrap()
        .with_file(base.join("c.md"), DOC_PLAIN.as_bytes())
        .unwrap()
        .with_file(base.join("note.txt"), b"hello\nworld\n")
        .unwrap()
        .with_file(base.join("pic.png"), &solid_png())
        .unwrap()
        .with_file(base.join("many.md"), needles(40).as_bytes())
        .unwrap()
        .with_dir(base.join("p"))
        .unwrap()
        .with_file(base.join(".remargin.yaml"), b"identity: tester\n")
        .unwrap()
        .with_file("/keys/ed25519", TEST_PRIVATE_KEY.as_bytes())
        .unwrap()
}

fn new_request(method: &str, params: Value) -> String {
    let mut carried = params;
    carried["_meta"] = json!({
        PROTOCOL_VERSION_KEY: "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    json!({ "jsonrpc": "2.0", "id": 1_i32, "method": method, "params": carried }).to_string()
}

fn new_call_text(tool: &str, arguments: &Value) -> String {
    new_request(
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
}

fn new_answer(
    server: &Server,
    system: &MemorySystem,
    config: &ResolvedConfig,
    text: &str,
) -> Value {
    let answer = host::process_request(server, system, Path::new(BASE), config.clone(), text)
        .unwrap()
        .unwrap();
    serde_json::from_str(&answer).unwrap()
}

fn tools_of(answer: &Value) -> Vec<Value> {
    answer["result"]["tools"].as_array().unwrap().clone()
}

fn today() -> Vec<Value> {
    serde_json::from_str(include_str!("args/today_schemas.json")).unwrap()
}

fn generated() -> Vec<Value> {
    let answer = new_answer(
        &Server::new(),
        &MemorySystem::new(),
        &agent_config(),
        &new_request("tools/list", json!({})),
    );
    tools_of(&answer)
}

#[test]
fn tools_list_names_the_38_tools_in_their_order() {
    let (old, new) = (today(), generated());
    let names = |tools: &[Value]| -> Vec<String> {
        tools
            .iter()
            .map(|tool| String::from(tool["name"].as_str().unwrap()))
            .collect()
    };
    assert_eq!(names(&new).len(), 38);
    assert_eq!(names(&new), names(&old));
}

/// The rustdoc of the trait carries each description with the identifiers `clippy::doc_markdown`
/// asks to be quoted in backticks, so the two texts are held equal once backticks are set aside.
#[test]
fn tools_list_describes_each_tool_as_the_schemas_do() {
    let (old, new) = (today(), generated());
    let plain = |tool: &Value| tool["description"].as_str().unwrap().replace('`', "");
    for (recorded, listed) in old.iter().zip(&new) {
        assert_eq!(plain(listed), plain(recorded), "{}", recorded["name"]);
    }
}

#[test]
fn tools_list_declares_the_arguments_the_schemas_declare() {
    let (old, new) = (today(), generated());
    for (recorded, listed) in old.iter().zip(&new) {
        let keys = |tool: &Value| -> Vec<String> {
            tool["inputSchema"]["properties"]
                .as_object()
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default()
        };
        assert_eq!(keys(listed), keys(recorded), "{}", recorded["name"]);
        let required = |tool: &Value| -> BTreeSet<String> {
            tool["inputSchema"]["required"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|name| String::from(name.as_str().unwrap()))
                .collect()
        };
        assert_eq!(required(listed), required(recorded), "{}", recorded["name"]);
    }
}

#[test]
fn tools_list_marks_the_tools_that_change_nothing_read_only() {
    let new = generated();
    let read_only: Vec<&str> = new
        .iter()
        .filter(|tool| tool["annotations"]["readOnlyHint"] == true)
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        read_only,
        [
            "activity",
            "comments",
            "doctor",
            "get",
            "get_image",
            "lint",
            "ls",
            "metadata",
            "permissions_check",
            "permissions_show",
            "prompt_list",
            "prompt_resolve",
            "query",
            "sandbox_list",
            "search",
            "whoami",
        ]
    );
}

fn refusal_text(answer: &Value) -> Value {
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap_or_else(|_| json!({ "message": text }))
}

fn refused(config: &ResolvedConfig, tool: &str, arguments: &Value) -> Value {
    refused_in(&realm(), config, tool, arguments)
}

fn refused_in(
    system: &MemorySystem,
    config: &ResolvedConfig,
    tool: &str,
    arguments: &Value,
) -> Value {
    refusal_text(&new_answer(
        &Server::new(),
        system,
        config,
        &new_call_text(tool, arguments),
    ))
}

#[test]
fn an_unknown_argument_is_refused() {
    let refusal = refused(
        &agent_config(),
        "comment",
        &json!({ "file": "c.md", "content": "x", "not_an_argument": true }),
    );
    assert!(
        refusal.to_string().contains("not_an_argument"),
        "the refusal names the argument: {refusal}"
    );
}

#[test]
fn an_identity_flag_is_refused() {
    for flag in ["config_path", "identity", "key", "type"] {
        let refusal = refused(
            &agent_config(),
            "comment",
            &json!({ "file": "c.md", "content": "x", flag: "someone" }),
        );
        assert!(
            refusal.to_string().contains(flag),
            "{flag} is refused by name: {refusal}"
        );
    }
}

#[test]
fn a_human_identity_is_refused_but_for_whoami_and_identity_create() {
    let refusal = refused(
        &human_config(),
        "comment",
        &json!({ "file": "c.md", "content": "x" }),
    );
    assert_eq!(refusal["error_kind"], "mcp_human_identity_rejected");
    assert_eq!(refusal["tool"], "comment");
    assert_eq!(refusal["identity"], "eduardo-burgos");

    let server = Server::new();
    let system = realm();
    for (tool, arguments) in [
        ("whoami", json!({})),
        (
            "identity_create",
            json!({ "identity": "bot", "type": "agent" }),
        ),
    ] {
        let answer = new_answer(
            &server,
            &system,
            &human_config(),
            &new_call_text(tool, &arguments),
        );
        assert_eq!(answer["result"]["isError"], false, "{tool}: {answer}");
    }
}

#[test]
fn a_path_outside_the_realm_is_refused() {
    let refusal = refused(&agent_config(), "get", &json!({ "path": "/etc/hostname" }));
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .contains("path escapes MCP sandbox"),
        "{refusal}"
    );
}

#[test]
fn a_path_outside_trusted_roots_is_refused() {
    let system = realm()
        .with_file(
            Path::new(BASE).join(".remargin.yaml"),
            b"identity: tester\npermissions:\n  trusted_roots:\n    - path: /docs/p\n",
        )
        .unwrap()
        .with_file(Path::new(BASE).join("p/inside.md"), DOC_PLAIN.as_bytes())
        .unwrap();
    let refusal = refused_in(&system, &agent_config(), "get", &json!({ "path": "c.md" }));
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .contains("trusted_roots"),
        "{refusal}"
    );
    let allowed = new_answer(
        &Server::new(),
        &system,
        &agent_config(),
        &new_call_text("get", &json!({ "path": "p/inside.md" })),
    );
    assert_eq!(allowed["result"]["isError"], false, "{allowed}");
}

#[test]
fn a_request_without_meta_is_refused_with_invalid_params() {
    let asked = json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": { "name": "whoami", "arguments": {} },
    })
    .to_string();
    let answer = new_answer(&Server::new(), &realm(), &agent_config(), &asked);
    assert_eq!(answer["error"]["code"], -32_602_i64, "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Missing required"),
        "{answer}"
    );
}

fn search_page(server: &Server, system: &MemorySystem) -> Value {
    let answer = new_answer(
        server,
        system,
        &agent_config(),
        &new_call_text("search", &json!({ "pattern": "needle", "path": "many.md" })),
    );
    answer["result"]["structuredContent"].clone()
}

fn rows_in(page: &Value) -> usize {
    page["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["matches"].as_array().unwrap().len())
        .sum()
}

#[test]
fn report_spill_lowers_the_cap_and_the_listings_page_under_it() {
    let (server, system) = (Server::new(), realm());
    let whole = search_page(&server, &system);
    assert_eq!(rows_in(&whole), 40);
    assert!(whole.get("effective_limit").is_none());

    let reported = new_answer(
        &server,
        &system,
        &agent_config(),
        &new_call_text("report_spill", &json!({ "size": 600_u32 })),
    );
    let lowered = &reported["result"]["structuredContent"];
    assert_eq!(lowered["previous_cap"], 40_000_u64);
    assert_eq!(lowered["spill_cap"], 600_u64);
    assert_eq!(lowered["inferred"], false);

    let paged = search_page(&server, &system);
    let kept = rows_in(&paged);
    assert!(kept > 0 && kept < 40, "{paged}");
    assert_eq!(paged["effective_limit"], kept);
    assert_eq!(paged["total"], 40_u64);

    let raised = new_answer(
        &server,
        &system,
        &agent_config(),
        &new_call_text("report_spill", &json!({ "size": 900_000_u32 })),
    );
    assert_eq!(raised["result"]["structuredContent"]["spill_cap"], 600_u64);
}

#[test]
fn report_spill_infers_the_size_of_the_last_answer() {
    let (server, system) = (Server::new(), realm());
    let first = new_answer(
        &server,
        &system,
        &agent_config(),
        &new_call_text("search", &json!({ "pattern": "needle", "path": "many.md" })),
    );
    let blocks = first["result"]["content"].as_array().unwrap();
    let written: usize = blocks
        .iter()
        .map(|block| block["text"].as_str().unwrap().len())
        .sum();

    let reported = new_answer(
        &server,
        &system,
        &agent_config(),
        &new_call_text("report_spill", &json!({})),
    );
    let answered = &reported["result"]["structuredContent"];
    assert_eq!(answered["inferred"], true);
    assert_eq!(answered["reported_size"], written);
    assert_eq!(answered["spill_cap"], written);
}
