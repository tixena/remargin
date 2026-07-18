use std::collections::HashMap;
use std::fs;
use std::path::Path;

use syn::spanned::Spanned as _;
use syn::{Item, ItemFn};

/// The two adapter files we hold to this cap. Relative paths from
/// the crate manifest (resolved below via `CARGO_MANIFEST_DIR`).
const ADAPTER_FILES: &[(&str, &str)] = &[
    ("CLI", "../remargin/src/main.rs"),
    ("MCP", "../remargin-core/src/mcp.rs"),
];

/// Prefixes that identify adapter-layer handlers: `cmd_*` in the
/// CLI, `handle_*` in the MCP tool router.
const ADAPTER_PREFIXES: &[&str] = &["cmd_", "handle_"];

/// Physical-line cap for adapter-layer helpers. Set loose enough to
/// keep the noise level low — its purpose is to prevent a new
/// handler from crossing 2x the current upper-bound on adapter size
/// without deliberate allowlisting. Tighten once the outliers in
/// [`allowlist`] drop below.
const LOC_CAP: usize = 50;

/// Named exceptions to the [`LOC_CAP`]. Each entry records a
/// function that legitimately exceeds the cap today, paired with a
/// one-line rationale the next reader can verify.
///
/// Entries should shrink over time as core helpers absorb more
/// adapter glue. Any new entry is a signal that either (a) the
/// function should be refactored, or (b) the cap should move.
///
/// NOTE: values here are *recorded* line counts, not ceilings. If a
/// function grows beyond the recorded value, the test fails and the
/// entry must be re-examined (or the function refactored).
fn allowlist() -> HashMap<&'static str, (usize, &'static str)> {
    let mut m = HashMap::new();
    // CLI: plan dispatcher — consolidated match over all PlanAction
    // variants, one arm per op. Shrinking further would duplicate
    // argument unwrapping between cmd_plan and core dispatch; the
    // match itself is low-density mapping code.
    m.insert(
            "cmd_plan",
            (
                215_usize,
                "consolidated PlanAction -> PlanRequest match (plan restrict resolves anchor + user/project settings inline; plan unprotect builds UnprotectArgs inline; plan mv adds a 5-line src/dst/force unwrap)",
            ),
        );
    // CLI: sandbox top-level command dispatches across four
    // SandboxAction variants; each sub-branch is adapter glue
    // (identity gating, strip_prefix_display, dry-run toggles).
    m.insert(
        "cmd_sandbox",
        (100, "dispatches four SandboxAction variants"),
    );
    // CLI: MCP-server boot adapter — sets up stdin/stdout streams,
    // the tracing sub-subscriber, and the MCP loop. Not per-op glue;
    // configuration-heavy setup code.
    m.insert("cmd_mcp", (100, "MCP server bootstrap + tracing setup"));
    // CLI: plugin install / uninstall / test — shells out to the
    // Claude CLI for marketplace + plugin management; each arm is
    // build-args + output-handling glue, not per-op core logic.
    m.insert(
        "cmd_plugin",
        (
            120,
            "shells out to claude plugins marketplace add / install / uninstall / list",
        ),
    );
    // CLI: Obsidian install/uninstall — opt-in via the `obsidian`
    // feature; interactive-ish download + patch flow that is itself
    // not part of the document API surface.
    m.insert(
        "cmd_obsidian",
        (75, "feature-gated Obsidian vault plugin install"),
    );
    // CLI: query parses a rich filter DSL out of clap args; most
    // lines are flag -> QueryOptions field assignments, no logic.
    m.insert(
        "cmd_query",
        (70, "parses rich QueryOptions from clap flags"),
    );
    // CLI: activity adapter resolves explicit/implicit path, parses
    // optional --since cutoff, and resolves caller identity through
    // ResolvedConfig before delegating to activity::gather_activity.
    // Each step is a 4-6 line block of clap-arg unwrapping.
    m.insert(
        "cmd_activity",
        (
            55,
            "path/since/identity resolution before delegating to gather_activity",
        ),
    );
    // CLI: search adapter unwraps eight clap fields (scope enum,
    // pattern, options) into search::SearchOptions, expands the
    // path through the System, then formats results.
    m.insert(
        "cmd_search",
        (
            55,
            "extracts SearchOptions from clap flags + result formatting",
        ),
    );
    // MCP: plan dispatcher mirrors the CLI shape above; same
    // rationale. Shrinks when the adapter-layer PlanRequest builder
    // grows helpers in core (follow-on work).
    m.insert(
        "handle_plan",
        (
            120,
            "mirrors cmd_plan PlanAction dispatch (plan mv/cp each add a 4-line src/dst/force unwrap)",
        ),
    );
    // CLI: get adapter splits json+line-numbers from the default
    // path, both branches threading the trusted_roots slice through
    // the resolved-config-aware document::get. The split
    // is shape-shifting on flag combos, not logic; pushing it into
    // core would force the adapter to teach core about JSON output.
    m.insert(
            "cmd_get",
            (
                60,
                "two-branch get adapter (json+line-numbers vs default), threading trusted_roots through document::get",
            ),
        );
    // CLI: get binary adapter dispatches across --out / --json /
    // raw-bytes shapes, threading trusted_roots through
    // document::read_binary.
    m.insert(
            "cmd_get_binary",
            (
                60,
                "binary get dispatch (--out vs --json vs raw bytes), threading trusted_roots through read_binary",
            ),
        );
    // MCP: get handler likewise splits binary vs text response
    // shaping, both branches threading trusted_roots through
    // document::read_binary / document::get. Shape-only
    // adapter glue.
    m.insert(
        "handle_get",
        (
            65,
            "binary vs text response split, threading trusted_roots through read_binary/get",
        ),
    );
    m
}

fn collect_fn_line_counts(src: &str) -> Result<Vec<(String, usize)>, syn::Error> {
    let file = syn::parse_file(src)?;

    let mut out = Vec::new();
    for item in &file.items {
        if let Item::Fn(ItemFn { sig, block, .. }) = item {
            let name = sig.ident.to_string();
            if !ADAPTER_PREFIXES.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            // Measure from the `fn` keyword through the closing `}`.
            // Use the block's span for the end so doc-comment
            // attributes do not inflate the count.
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
