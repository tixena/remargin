//! The generated server held against today's: the tool list, the success of every tool, the
//! refusals, and the session state `report_spill` lowers.

extern crate alloc;

use alloc::collections::BTreeSet;
use core::fmt::Write as _;
use std::path::{Path, PathBuf};

use image::codecs::png::PngEncoder;
use image::{Rgb, RgbImage};
use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use crate::config::{Mode, ResolvedConfig};
use crate::mcp;
use crate::mcp_server::handlers::Server;
use crate::mcp_server::host;
use crate::parser::AuthorType;

macro_rules! same_success {
    ($($test:ident => [$(($tool:literal, $arguments:expr)),+ $(,)?]),+ $(,)?) => {
        const TOOLS_HELD: &[&str] = &[$($($tool),+),+];

        $(
            #[test]
            fn $test() {
                assert_same_successes(&[$(($tool, $arguments)),+]);
            }
        )+
    };
}

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

const CONTENT_TOOLS: &[&str] = &["get", "get_image", "metadata"];

const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

same_success! {
    ack => [("ack", json!({ "file": "a.md", "ids": ["aaa"] }))],
    activity => [("activity", json!({}))],
    batch => [(
        "batch",
        json!({ "file": "c.md", "operations": [{ "content": "one" }, { "content": "two" }] })
    )],
    comment => [("comment", json!({ "file": "c.md", "content": "hello" }))],
    comments => [("comments", json!({ "file": "a.md", "include_integrity": true }))],
    cp => [("cp", json!({ "src": "c.md", "dst": "copy.md" }))],
    delete => [("delete", json!({ "file": "a.md", "ids": ["aaa"] }))],
    doctor => [("doctor", json!({ "user_settings_file": "/none.json" }))],
    edit => [("edit", json!({ "file": "a.md", "id": "aaa", "content": "changed" }))],
    get => [
        ("get", json!({ "path": "note.txt" })),
        ("get", json!({ "path": "c.md", "line_numbers": true, "start_line": 1_u32, "end_line": 2_u32 })),
        ("get", json!({ "path": "pic.png", "binary": true })),
    ],
    get_image => [("get_image", json!({ "path": "pic.png" }))],
    identity_create => [(
        "identity_create",
        json!({ "identity": "bot", "type": "agent", "key": "/keys/bot" })
    )],
    lint => [("lint", json!({ "file": "a.md" }))],
    ls => [("ls", json!({}))],
    metadata => [("metadata", json!({ "path": "a.md" })), ("metadata", json!({ "path": "pic.png" }))],
    mv => [("mv", json!({ "src": "c.md", "dst": "moved.md" }))],
    permissions_check => [("permissions_check", json!({ "path": "a.md", "why": true }))],
    permissions_show => [("permissions_show", json!({}))],
    plan => [
        ("plan", json!({ "op": "ack", "file": "a.md", "ids": ["aaa"] })),
        ("plan", json!({ "op": "comment", "file": "c.md", "content": "projected" })),
        ("plan", json!({ "op": "mv", "src": "c.md", "dst": "moved.md" })),
        ("plan", json!({
            "op": "batch", "file": "c.md", "ops": [{ "content": "x" }, { "content": "y" }]
        })),
    ],
    prompts => [
        ("prompt_set", json!({ "folder": "p", "name": "reviewer", "prompt": "review" })),
        ("prompt_list", json!({})),
        ("prompt_resolve", json!({ "file": "p/x.md" })),
        ("prompt_delete", json!({ "folder": "p" })),
    ],
    purge => [("purge", json!({ "file": "a.md" }))],
    query => [("query", json!({ "expanded": true, "include_integrity": true }))],
    react => [("react", json!({ "file": "a.md", "id": "aaa", "emoji": "\u{1F44D}" }))],
    replace => [(
        "replace",
        json!({ "path": "c.md", "pattern": "needle", "replacement": "pin" })
    )],
    reply => [("reply", json!({ "file": "a.md", "parent_id": "aaa", "content": "answer" }))],
    report_spill => [("report_spill", json!({ "size": 1000_u32 }))],
    rm => [("rm", json!({ "path": "note.txt" }))],
    sandbox => [
        ("sandbox_add", json!({ "files": ["a.md"] })),
        ("sandbox_list", json!({})),
        ("sandbox_remove", json!({ "files": ["a.md"] })),
    ],
    search => [("search", json!({ "pattern": "needle", "context": 1_u32 }))],
    sign => [("sign", json!({ "file": "a.md", "all_mine": true }))],
    verify => [("verify", json!({ "path": "a.md" })), ("verify", json!({ "path": "." }))],
    whoami => [("whoami", json!({}))],
    write => [("write", json!({ "path": "note.txt", "content": "replaced\n" }))],
}

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

fn old_request(tool: &str, arguments: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
    .to_string()
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

fn old_answer(system: &MemorySystem, config: &ResolvedConfig, text: &str) -> Value {
    let answer = mcp::process_request(system, Path::new(BASE), config, text)
        .unwrap()
        .unwrap();
    serde_json::from_str(&answer).unwrap()
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

fn without_elapsed(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.remove("elapsed_ms");
    }
    value
}

/// Members a call writes from the clock or a random id, which two runs do not share.
fn mask(value: &mut Value, members: &[&str]) {
    match value {
        Value::Object(object) => {
            for (key, member) in object.iter_mut() {
                if members.contains(&key.as_str()) {
                    *member = json!("<masked>");
                } else {
                    mask(member, members);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| mask(item, members)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn unstable_members(tool: &str) -> &'static [&'static str] {
    match tool {
        "batch" | "comment" | "reply" => &["id", "ids"],
        "plan" => &[
            "added",
            "checksum_after",
            "id",
            "ids",
            "rows",
            "verify_after",
        ],
        "sandbox_add" | "sandbox_list" | "sandbox_remove" => &["since"],
        _ => &[],
    }
}

/// What today's server answered, as the generated server would carry it: a text block parsed, a
/// block that is not text as it is.
fn blocks_of(content: &Value) -> Vec<Value> {
    content
        .as_array()
        .unwrap()
        .iter()
        .map(|block| {
            if block["type"] == "text" {
                let parsed: Value = serde_json::from_str(block["text"].as_str().unwrap()).unwrap();
                json!({ "type": "text", "json": without_elapsed(parsed) })
            } else {
                block.clone()
            }
        })
        .collect()
}

fn success_of_old(tool: &str, answer: &Value) -> Value {
    let result = &answer["result"];
    assert_ne!(
        result["isError"], true,
        "{tool} failed on today's server: {answer}"
    );
    if CONTENT_TOOLS.contains(&tool) {
        return json!(blocks_of(&result["content"]));
    }
    let text = result["content"][0]["text"].as_str().unwrap();
    without_elapsed(serde_json::from_str(text).unwrap())
}

fn success_of_new(tool: &str, answer: &Value) -> Value {
    let result = &answer["result"];
    assert_eq!(
        result["isError"], false,
        "{tool} failed on the generated server: {answer}"
    );
    if CONTENT_TOOLS.contains(&tool) {
        assert!(
            result.get("structuredContent").is_none(),
            "{tool}: {answer}"
        );
        return json!(blocks_of(&result["content"]));
    }
    let structured = result["structuredContent"].clone();
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        structured, text,
        "{tool}: the text block is the structured content"
    );
    assert!(structured["elapsed_ms"].is_u64(), "{tool}: {answer}");
    without_elapsed(structured)
}

/// Sends each step to today's server and to the generated one, each over its own copy of the
/// realm, and holds the two successes equal.
fn assert_same_successes(steps: &[(&str, Value)]) {
    let config = agent_config();
    let (old_system, new_system) = (realm(), realm());
    let server = Server::new();
    for (tool, arguments) in steps {
        let old = old_answer(&old_system, &config, &old_request(tool, arguments));
        let new = new_answer(
            &server,
            &new_system,
            &config,
            &new_call_text(tool, arguments),
        );
        let (mut old_success, mut new_success) =
            (success_of_old(tool, &old), success_of_new(tool, &new));
        mask(&mut old_success, unstable_members(tool));
        mask(&mut new_success, unstable_members(tool));
        assert_eq!(new_success, old_success, "{tool} {arguments}");
    }
}

fn tools_of(answer: &Value) -> Vec<Value> {
    answer["result"]["tools"].as_array().unwrap().clone()
}

fn listed() -> (Vec<Value>, Vec<Value>) {
    let system = MemorySystem::new();
    let old = old_answer(
        &system,
        &agent_config(),
        &json!({ "jsonrpc": "2.0", "id": 1_i32, "method": "tools/list", "params": {} }).to_string(),
    );
    let new = new_answer(
        &Server::new(),
        &system,
        &agent_config(),
        &new_request("tools/list", json!({})),
    );
    (tools_of(&old), tools_of(&new))
}

#[test]
fn tools_list_names_the_38_tools_in_todays_order() {
    let (old, new) = listed();
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
fn tools_list_describes_each_tool_as_today_does() {
    let (old, new) = listed();
    let plain = |tool: &Value| tool["description"].as_str().unwrap().replace('`', "");
    for (today, generated) in old.iter().zip(&new) {
        assert_eq!(plain(generated), plain(today), "{}", today["name"]);
    }
}

#[test]
fn tools_list_declares_the_arguments_each_tool_declares_today() {
    let (old, new) = listed();
    for (today, generated) in old.iter().zip(&new) {
        let keys = |tool: &Value| -> Vec<String> {
            tool["inputSchema"]["properties"]
                .as_object()
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default()
        };
        assert_eq!(keys(generated), keys(today), "{}", today["name"]);
        let required = |tool: &Value| -> BTreeSet<String> {
            tool["inputSchema"]["required"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|name| String::from(name.as_str().unwrap()))
                .collect()
        };
        assert_eq!(required(generated), required(today), "{}", today["name"]);
    }
}

#[test]
fn tools_list_marks_the_tools_that_change_nothing_read_only() {
    let (_, new) = listed();
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

#[test]
fn every_tool_has_a_parity_test() {
    let (_, new) = listed();
    for tool in &new {
        let name = tool["name"].as_str().unwrap();
        assert!(TOOLS_HELD.contains(&name), "{name} has no parity test");
    }
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
