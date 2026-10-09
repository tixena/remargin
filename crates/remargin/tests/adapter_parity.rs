//! Cross-surface parity harness for CLI + MCP `plan` ops.
//!
//! For each mutating `plan` op we invoke the CLI binary via `assert_cmd`
//! AND the in-process MCP handler via `mcp::process_request` against a
//! byte-identical fixture, then assert the resulting [`PlanReport`]
//! JSON payloads are structurally equivalent.
//!
//! `plan` is a pure projection (no disk mutation), so we can compare
//! both surfaces without worrying about mutation-order effects. The
//! only fields that legitimately differ across adapter invocations are
//! wall-clock dependent (`ts`, `elapsed_ms`) — [`normalize`] strips
//! them before the `assert_eq!`. Any *other* divergence indicates
//! adapter drift and is the regression this harness is designed to
//! catch.
//!
//! Covers the deterministic ops `delete`, `edit`, `purge`, and
//! `write` (markdown + raw). `ack`, `react`,
//! `sandbox-add`, `sandbox-remove`, `comment`, and `batch` are excluded
//! because their projections stamp `Utc::now()` into the `after`
//! document, so byte-level parity would require freezing the clock.

#[cfg(test)]
#[path = "adapter_parity/tests.rs"]
mod tests;
