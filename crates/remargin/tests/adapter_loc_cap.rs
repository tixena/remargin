//! Regression guard for CLI/MCP adapter bloat (rem-wpq).
//!
//! Every mutating surface in this workspace is implemented twice: once
//! as a clap `cmd_*` helper in the CLI binary, and once as a `handle_*`
//! tool handler in the in-process MCP server. After the rem-3a2 audit
//! (and its follow-ups rem-oqv / rem-2ji / rem-e9c / rem-9ey), both
//! adapter layers are meant to stay genuinely thin: argument extraction
//! plus response formatting, with any non-trivial logic living once in
//! core.
//!
//! This test walks the two adapter files at compile time, counts the
//! physical lines of every `cmd_*` / `handle_*` free-standing function,
//! and asserts each is under a target cap. The cap is deliberately
//! loose (keeps the noise level low) — the value of the guard is that a
//! *new* handler cannot creep in at 90 lines without either (a) shrinking
//! below the cap by pushing logic to core, or (b) being explicitly
//! allowlisted below with a rationale. Allowlist entries force a
//! conscious decision; the test surfaces every offender so ad-hoc
//! bloat cannot slip in silently.
//!
//! To refresh after a legitimate migration: if a function drops below
//! the cap, remove its allowlist entry. If a new function intentionally
//! exceeds the cap, add it here with a one-line reason.
#![expect(
    clippy::print_stdout,
    reason = "diagnostic output on failure helps debug cap regressions"
)]

#[cfg(test)]
mod tests {

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
        // match itself is low-density mapping code (rem-oqv, rem-9ey).
        m.insert(
            "cmd_plan",
            (140_usize, "consolidated PlanAction -> PlanRequest match"),
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
        m.insert("cmd_mcp", (95, "MCP server bootstrap + tracing setup"));
        // CLI: Obsidian install/uninstall — opt-in via the `obsidian`
        // feature; interactive-ish download + patch flow that is itself
        // not part of the document API surface.
        m.insert(
            "cmd_obsidian",
            (60, "feature-gated Obsidian vault plugin install"),
        );
        // CLI: query parses a rich filter DSL out of clap args; most
        // lines are flag -> QueryOptions field assignments, no logic.
        m.insert(
            "cmd_query",
            (70, "parses rich QueryOptions from clap flags"),
        );
        // MCP: plan dispatcher mirrors the CLI shape above; same
        // rationale. Shrinks when the adapter-layer PlanRequest builder
        // grows helpers in core (follow-on work).
        m.insert("handle_plan", (101, "mirrors cmd_plan PlanAction dispatch"));
        // MCP: search handler extracts eight optional filter fields.
        m.insert(
            "handle_search",
            (60, "extracts SearchOptions from tool params"),
        );
        m
    }

    #[expect(
        clippy::expect_used,
        reason = "integration-test parser: failing to parse the adapter files means the test is broken, which we want surfaced loudly"
    )]
    fn collect_fn_line_counts(src: &str) -> Vec<(String, usize)> {
        let file = syn::parse_file(src).expect("adapter source parses as a Rust file");

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
        out
    }

    #[expect(
        clippy::panic,
        reason = "integration-test assertion helper: panic is how test failures propagate to cargo test"
    )]
    #[test]
    fn adapter_handlers_stay_under_loc_cap() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let allow = allowlist();
        let mut violations: Vec<String> = Vec::new();

        for (surface, rel_path) in ADAPTER_FILES {
            let full = Path::new(manifest_dir).join(rel_path);
            let src = fs::read_to_string(&full)
                .unwrap_or_else(|err| panic!("reading {}: {err}", full.display()));
            let counts = collect_fn_line_counts(&src);

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

        if !violations.is_empty() {
            for v in &violations {
                println!("LOC CAP VIOLATION: {v}");
            }
            panic!("{} adapter handler(s) exceeded LOC cap", violations.len());
        }
    }
} // mod tests
