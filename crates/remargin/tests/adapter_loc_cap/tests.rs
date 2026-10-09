//! Counts the lines of every CLI and MCP adapter handler and holds each to the cap.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use syn::spanned::Spanned as _;
use syn::{Item, ItemFn};

/// Relative to the crate manifest directory.
const ADAPTER_FILES: &[(&str, &str)] = &[
    ("CLI", "../remargin/src/main.rs"),
    ("MCP", "../remargin-core/src/mcp.rs"),
];

const ADAPTER_PREFIXES: &[&str] = &["cmd_", "handle_"];

/// Physical lines per handler. Loose on purpose: the guard is against a new handler creeping in.
const LOC_CAP: usize = 50;

/// Named exceptions to the [`LOC_CAP`]: each function that exceeds it, with a one-line rationale.
///
/// NOTE: values here are *recorded* line counts, not ceilings. If a
/// function grows beyond the recorded value, the test fails and the
/// entry must be re-examined (or the function refactored).
fn allowlist() -> HashMap<&'static str, (usize, &'static str)> {
    let mut m = HashMap::new();
    m.insert(
            "cmd_plan",
            (
                215_usize,
                "consolidated PlanAction -> PlanRequest match (plan restrict resolves anchor + user/project settings inline; plan unprotect builds UnprotectArgs inline; plan mv adds a 5-line src/dst/force unwrap)",
            ),
        );
    m.insert(
        "cmd_sandbox",
        (100, "dispatches four SandboxAction variants"),
    );
    m.insert("cmd_mcp", (100, "MCP server bootstrap + tracing setup"));
    m.insert(
        "cmd_plugin",
        (
            120,
            "shells out to claude plugins marketplace add / install / uninstall / list",
        ),
    );
    m.insert(
        "cmd_obsidian",
        (75, "feature-gated Obsidian vault plugin install"),
    );
    m.insert(
        "cmd_query",
        (70, "parses rich QueryOptions from clap flags"),
    );
    m.insert(
        "cmd_activity",
        (
            55,
            "path/since/identity resolution before delegating to gather_activity",
        ),
    );
    m.insert(
        "cmd_search",
        (
            55,
            "extracts SearchOptions from clap flags + result formatting",
        ),
    );
    m.insert(
        "handle_plan",
        (
            120,
            "mirrors cmd_plan PlanAction dispatch (plan mv/cp each add a 4-line src/dst/force unwrap)",
        ),
    );
    m.insert(
        "cmd_get",
        (
            60,
            "two-branch get adapter (json+line-numbers vs default) over document::get",
        ),
    );
    m.insert(
        "cmd_get_binary",
        (
            60,
            "binary get dispatch (--out vs --json vs raw bytes) over read_binary",
        ),
    );
    m.insert(
        "handle_get",
        (65, "binary vs text response split over read_binary/get"),
    );
    m
}

fn collect_fn_line_counts(src: &str) -> Result<Vec<(String, usize)>, syn::Error> {
    let file = syn::parse_file(src)?;

    let mut out = Vec::new();
    for item in &file.items {
        if let Item::Fn(ItemFn {
            sig,
            block,
            attrs: _,
            vis: _,
            modifiers: _,
        }) = item
        {
            let name = sig.ident.to_string();
            if !ADAPTER_PREFIXES.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            // From the `fn` keyword to the closing `}`, so doc comments and attributes do not count.
            let start_line = sig.ident.span().start().line;
            let end_line = block.span().end().line;
            let loc = end_line.saturating_sub(start_line).saturating_add(1);
            out.push((name, loc));
        }
    }
    Ok(out)
}

#[test]
fn adapter_handlers_stay_under_loc_cap() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let allow = allowlist();
    let mut violations: Vec<String> = Vec::new();

    for (surface, rel_path) in ADAPTER_FILES {
        let full = Path::new(manifest_dir).join(rel_path);
        let read = fs::read_to_string(&full);
        assert!(
            read.is_ok(),
            "reading {}: {:?}",
            full.display(),
            read.as_ref().err()
        );
        let src = read.unwrap();
        let counts = match collect_fn_line_counts(&src) {
            Ok(c) => c,
            Err(err) => {
                violations.push(format!("{surface}: parsing {}: {err}", full.display()));
                continue;
            }
        };

        for (name, loc) in counts {
            if let Some((recorded_cap, reason)) = allow.get(name.as_str()) {
                if loc > *recorded_cap {
                    violations.push(format!(
                            "{surface} fn {name} is {loc} lines; allowlisted at {recorded_cap} ({reason}). \
                             Either refactor or update the recorded cap in allowlist()."
                        ));
                }
            } else if loc > LOC_CAP {
                violations.push(format!(
                    "{surface} fn {name} is {loc} lines; cap is {LOC_CAP}. \
                         Refactor to push logic into remargin_core, or add a justified \
                         entry to allowlist() in tests/adapter_loc_cap.rs."
                ));
            } else {
                // Within cap and not allowlisted — nothing to do.
            }
        }
    }

    assert!(
        violations.is_empty(),
        "{} adapter handler(s) exceeded LOC cap:\n  - {}",
        violations.len(),
        violations.join("\n  - ")
    );
}
