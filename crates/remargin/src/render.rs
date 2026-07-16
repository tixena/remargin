//! Thin sink-writers: each function calls a `remargin_core` renderer and
//! writes the returned `String` (or bytes) to the appropriate [`IoSinks`]
//! stream. No formatting logic lives here — this module is the glue between
//! the core renderers and the CLI streams.

use std::path::Path;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System;
use serde_json::{Value, json};

use remargin_core::activity;
use remargin_core::config::identity::IdentityReport;
use remargin_core::config::registry;
use remargin_core::config::system_prompt;
use remargin_core::config::system_prompt::ResolvedSystemPrompt;
use remargin_core::display;
use remargin_core::document::get_image as image_ops;
use remargin_core::operations;
use remargin_core::operations::cp as cp_op;
use remargin_core::operations::mv as mv_op;
use remargin_core::operations::plan as plan_ops;
use remargin_core::operations::query;
use remargin_core::operations::sandbox as sandbox_ops;
use remargin_core::permissions::doctor as permissions_doctor;
use remargin_core::permissions::inspect as permissions_inspect;
use remargin_core::permissions::restrict as permissions_restrict;
use remargin_core::permissions::unprotect as permissions_unprotect;

use crate::io::{IoSinks, out_json, out_raw, print_output};
use crate::params::{QueryOutputMode, QueryParams};

pub fn render_identity_report(
    sinks: &mut IoSinks<'_>,
    report: &IdentityReport,
    json_mode: bool,
) -> Result<()> {
    if !report.found {
        if json_mode {
            return print_output(sinks, true, &json!({ "found": false }));
        }
        writeln!(sinks.stderr, "No identity config found.").context("writing to stderr")?;
        return Ok(());
    }

    if json_mode {
        return print_output(
            sinks,
            true,
            &serde_json::to_value(report).context("serializing identity report")?,
        );
    }

    write!(sinks.stderr, "{}", display::render_identity_report(report))
        .context("writing identity report to stderr")
}

pub fn emit_activity_pretty(
    sinks: &mut IoSinks<'_>,
    result: &activity::ActivityResult,
) -> Result<()> {
    write!(sinks.stderr, "{}", display::render_activity_pretty(result))
        .context("writing activity to stderr")
}

pub fn emit_doctor_text(
    sinks: &mut IoSinks<'_>,
    report: &permissions_doctor::DoctorReport,
    verbose: bool,
) -> Result<()> {
    write!(
        sinks.stdout,
        "{}",
        permissions_doctor::render_doctor_text(report, verbose)
    )
    .context("writing doctor output")
}

pub fn emit_doctor_prompt(
    sinks: &mut IoSinks<'_>,
    report: &permissions_doctor::DoctorReport,
) -> Result<()> {
    write!(
        sinks.stdout,
        "{}",
        permissions_doctor::render_doctor_prompt(report)
    )
    .context("writing doctor repair prompt")
}

pub fn emit_permissions_show_text(
    sinks: &mut IoSinks<'_>,
    cwd: &Path,
    report: &permissions_inspect::ShowOutput,
) -> Result<()> {
    write!(
        sinks.stderr,
        "{}",
        permissions_inspect::render_show_text(cwd, report)
    )
    .context("writing permissions show to stderr")
}

pub fn emit_permissions_check_text(
    sinks: &mut IoSinks<'_>,
    report: &permissions_inspect::CheckOutput,
    why: bool,
) -> Result<()> {
    write!(
        sinks.stderr,
        "{}",
        permissions_inspect::render_check_text(report, why)
    )
    .context("writing permissions check to stderr")
}

pub fn emit_restrict_summary(
    sinks: &mut IoSinks<'_>,
    outcome: &permissions_restrict::RestrictOutcome,
) -> Result<()> {
    write!(
        sinks.stderr,
        "{}",
        permissions_restrict::render_restrict_summary(outcome)
    )
    .context("writing restrict summary to stderr")
}

pub fn emit_unprotect_summary(
    sinks: &mut IoSinks<'_>,
    outcome: &permissions_unprotect::UnprotectOutcome,
) -> Result<()> {
    write!(
        sinks.stderr,
        "{}",
        permissions_unprotect::render_unprotect_summary(outcome)
    )
    .context("writing unprotect summary to stderr")
}

pub fn write_prompt_resolve_text(
    sinks: &mut IoSinks<'_>,
    target: &Path,
    resolved: &ResolvedSystemPrompt,
) -> Result<()> {
    write!(
        sinks.stderr,
        "{}",
        system_prompt::render_resolved_prompt(target, resolved)
    )
    .context("writing prompt resolve to stderr")
}

pub fn emit_plan_restrict_text(
    sinks: &mut IoSinks<'_>,
    report: &plan_ops::PlanReport,
) -> Result<()> {
    out_raw(sinks, &plan_ops::render_plan_restrict_text(report))
}

pub fn emit_plan_unprotect_text(
    sinks: &mut IoSinks<'_>,
    report: &plan_ops::PlanReport,
) -> Result<()> {
    out_raw(sinks, &plan_ops::render_plan_unprotect_text(report))
}

pub fn render_query_output(
    sinks: &mut IoSinks<'_>,
    results: &[query::QueryResult],
    params: &QueryParams<'_>,
    pending_label: Option<&str>,
) -> Result<()> {
    match params.output {
        QueryOutputMode::Json => {
            return print_output(
                sinks,
                true,
                &json!({
                    "base_path": format!("{}/", params.path.trim_end_matches('/')),
                    "results": results,
                }),
            );
        }
        QueryOutputMode::Pretty => {
            return out_raw(sinks, &display::format_query_pretty(results, pending_label));
        }
        QueryOutputMode::Plain | QueryOutputMode::Summary => {}
    }
    out_raw(sinks, &query::render_query_plain(results))
}

/// Render a single registry participant as a one-line string.
pub fn registry_participant_pretty(
    name: &str,
    participant: &registry::RegistryParticipant,
) -> String {
    registry::render_registry_participant(name, participant)
}

pub fn emit_sandbox_bulk_result(
    sinks: &mut IoSinks<'_>,
    result: &sandbox_ops::SandboxBulkResult,
    cwd: &Path,
    changed_key: &str,
    json_mode: bool,
) -> Result<()> {
    if json_mode {
        out_json(sinks, &result.to_json(cwd, changed_key))?;
    } else {
        write!(
            sinks.stderr,
            "{}",
            sandbox_ops::render_sandbox_bulk_result(result, cwd, changed_key)
        )
        .context("writing sandbox result to stderr")?;
    }
    Ok(())
}

pub fn cp_outcome_pretty(src: &str, dst: &str, outcome: &cp_op::CpOutcome) -> String {
    cp_op::render_cp_outcome(src, dst, outcome)
}

pub fn mv_outcome_pretty(src: &str, dst: &str, outcome: &mv_op::MvOutcome) -> String {
    mv_op::render_mv_outcome(src, dst, outcome)
}

pub fn render_get_image_result(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    result: &image_ops::GetImageResult,
    out_path: Option<&Path>,
    json_mode: bool,
) -> Result<()> {
    if let Some(dest) = out_path {
        system
            .write(dest, &result.bytes)
            .with_context(|| format!("writing {}", dest.display()))?;
        let mut summary = result.to_json_without_content();
        summary["out"] = json!(dest);
        return print_output(sinks, json_mode, &summary);
    }
    if json_mode {
        let mut payload = result.to_json_without_content();
        payload["binary"] = Value::Bool(true);
        payload["content"] = Value::String(BASE64_STANDARD.encode(&result.bytes));
        payload["path"] = json!(result.source_path);
        return print_output(sinks, true, &payload);
    }
    sinks
        .stdout
        .write_all(&result.bytes)
        .context("writing image bytes to stdout")
}

pub fn render_sign_result_text(
    sinks: &mut IoSinks<'_>,
    result: &operations::sign::SignResult,
) -> Result<()> {
    out_raw(sinks, &operations::sign::render_sign_result_text(result))
}
