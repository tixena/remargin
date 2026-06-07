//! Command handler functions (`cmd_*`) and request builders (`build_*`).
//!
//! These are the orchestration functions that the `dispatch` layer calls:
//! resolve config → call `remargin-core` op → call the `render` sink-writers.
//! No grammar parsing happens here; argument structs arrive pre-destructured
//! from the `handle_*` adapters in `main.rs`.

use std::env;
use std::io::{Read as _, stdin as stdin_handle};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System;
use serde_json::{Value, json};

#[cfg(feature = "obsidian")]
use crate::ObsidianAction;
use crate::dispatch::{PERMISSIONS_NOT_RESTRICTED_MARKER, build_identity_flags, tri_state_flag};
use crate::io::{
    IoSinks, expand_cli_path, expand_cli_pathbuf, out, out_json, out_raw, parse_line_range,
    print_output, read_stdin, resolve_doc_path, resolve_purge_path, truncate_content,
};
#[cfg(feature = "obsidian")]
use crate::obsidian;
use crate::params::{
    AckParams, ActivityParams, CommentParams, CpParams, EditParams, GetImageParams, GetParams,
    MvParams, PromptSetParams, QueryOutputMode, QueryParams, QueryPendingFilters, ReactParams,
    ReplaceParams, RestrictParams, SearchParams, SignParams, WriteParams,
};
use crate::render;
use crate::{
    Commands, DEFAULT_USER_SETTINGS, IdentityAction, IdentityArgs, McpAction,
    PLUGIN_MARKETPLACE_NAME, PLUGIN_MARKETPLACE_SOURCE, PLUGIN_REF, PermissionsAction, PlanAction,
    PlanClaudeAction, PluginAction, RegistryAction, SandboxAction,
};
use remargin_core::activity;
use remargin_core::config::identity::{IdentityFlags, resolve_identity_report};
use remargin_core::config::{self, ResolvedConfig};
use remargin_core::display;
use remargin_core::document;
use remargin_core::document::get_image as image_ops;
use remargin_core::kind::matches_kind_filter;
use remargin_core::linter;
use remargin_core::mcp;
use remargin_core::operations;
use remargin_core::operations::batch::BatchCommentOp;
use remargin_core::operations::cp as cp_op;
use remargin_core::operations::mv as mv_op;
use remargin_core::operations::plan as plan_ops;
use remargin_core::operations::projections;
use remargin_core::operations::purge;
use remargin_core::operations::query;
use remargin_core::operations::replace;
use remargin_core::operations::sandbox as sandbox_ops;
use remargin_core::operations::search;
use remargin_core::operations::verify::RecipientStatus;
use remargin_core::parser;
use remargin_core::permissions::doctor as permissions_doctor;
use remargin_core::permissions::inspect as permissions_inspect;
use remargin_core::permissions::pretool_install;
use remargin_core::permissions::restrict as permissions_restrict;
use remargin_core::permissions::unprotect as permissions_unprotect;
use remargin_core::responses;
use remargin_core::writer::InsertPosition;

const fn author_type_str(at: &parser::AuthorType) -> &'static str {
    at.as_str()
}

fn pretool_settings_path(system: &dyn System, cwd: &Path, local: bool) -> Result<PathBuf> {
    if local {
        Ok(cwd.join(".claude/settings.json"))
    } else {
        expand_cli_path(system, DEFAULT_USER_SETTINGS)
    }
}

pub fn cmd_pretool_install(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    local: bool,
    json_mode: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("resolving current directory")?;
    let path = pretool_settings_path(system, &cwd, local)?;
    let outcome = pretool_install::install(system, &path)?;
    let scope = scope_label(local);
    let status = match outcome {
        pretool_install::InstallOutcome::AlreadyInstalled => "already_installed",
        pretool_install::InstallOutcome::Installed => "installed",
        _ => "unknown",
    };
    if json_mode {
        print_output(
            sinks,
            true,
            &json!({
                "status": status,
                "scope": scope,
                "settings_file": path.display().to_string(),
            }),
        )
    } else {
        writeln!(
            sinks.stderr,
            "PreToolUse hook ({scope}): {status} at {}",
            path.display(),
        )
        .context("writing to stderr")?;
        Ok(())
    }
}

pub fn cmd_pretool_uninstall(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    local: bool,
    json_mode: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("resolving current directory")?;
    let path = pretool_settings_path(system, &cwd, local)?;
    let outcome = pretool_install::uninstall(system, &path)?;
    let scope = scope_label(local);
    let status = match outcome {
        pretool_install::UninstallOutcome::NotInstalled => "not_installed",
        pretool_install::UninstallOutcome::Uninstalled => "uninstalled",
        _ => "unknown",
    };
    if json_mode {
        print_output(
            sinks,
            true,
            &json!({
                "status": status,
                "scope": scope,
                "settings_file": path.display().to_string(),
            }),
        )
    } else {
        writeln!(
            sinks.stderr,
            "PreToolUse hook ({scope}): {status} at {}",
            path.display(),
        )
        .context("writing to stderr")?;
        Ok(())
    }
}

pub fn cmd_pretool_test(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    local: bool,
    json_mode: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("resolving current directory")?;
    let path = pretool_settings_path(system, &cwd, local)?;
    let outcome = pretool_install::test(system, &path)?;
    let scope = scope_label(local);
    let status = match outcome {
        pretool_install::TestOutcome::Installed => "installed",
        pretool_install::TestOutcome::NotInstalled => "not_installed",
        _ => "unknown",
    };
    if json_mode {
        print_output(
            sinks,
            true,
            &json!({
                "status": status,
                "scope": scope,
                "settings_file": path.display().to_string(),
            }),
        )
    } else {
        writeln!(
            sinks.stderr,
            "PreToolUse hook ({scope}): {status} at {}",
            path.display(),
        )
        .context("writing to stderr")?;
        Ok(())
    }
}

const fn scope_label(local: bool) -> &'static str {
    if local { "project" } else { "user" }
}

pub fn build_query_params(command: &Commands) -> Result<QueryParams<'_>> {
    let Commands::Query {
        path,
        author,
        comment_id,
        content_regex,
        expanded,
        ignore_case,
        pending,
        pending_broadcast,
        pending_for,
        pending_for_me,
        pretty,
        remargin_kind,
        since,
        summary,
        output_args,
        ..
    } = command
    else {
        bail!("internal: build_query_params called with wrong subcommand");
    };
    let output = if output_args.json {
        QueryOutputMode::Json
    } else if *pretty {
        QueryOutputMode::Pretty
    } else if *summary {
        QueryOutputMode::Summary
    } else {
        QueryOutputMode::Plain
    };
    let pending_filter = QueryPendingFilters {
        any: *pending,
        broadcast: *pending_broadcast,
        for_user: pending_for.as_deref(),
        for_me: *pending_for_me,
    };
    Ok(QueryParams {
        author: author.as_deref(),
        comment_id: comment_id.as_deref(),
        content_regex: content_regex.as_deref(),
        expanded: *expanded,
        ignore_case: *ignore_case,
        output,
        path: path.as_str(),
        pending: pending_filter,
        remargin_kind,
        since: since.as_deref(),
    })
}

pub fn cmd_ack(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &AckParams<'_>,
) -> Result<()> {
    let AckParams {
        file,
        ids,
        json_mode,
        remove,
        search_path,
    } = *params;
    if let Some(doc_file) = file {
        let path = resolve_doc_path(system, cwd, doc_file)?;
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        operations::ack_comments(system, &path, config, &id_refs, remove)?;
    } else {
        let base_dir = cwd.join(search_path);
        for comment_id in ids {
            let matches = query::resolve_comment_id(system, &base_dir, comment_id)?;
            match matches.len() {
                0 => {
                    bail!("comment {comment_id:?} not found");
                }
                1 => {
                    let id_refs: Vec<&str> = vec![comment_id.as_str()];
                    operations::ack_comments(system, &matches[0], config, &id_refs, remove)?;
                }
                n => {
                    let file_list: Vec<String> =
                        matches.iter().map(|p| p.display().to_string()).collect();
                    bail!(
                        "ambiguous: comment {comment_id:?} found in {n} files: {}",
                        file_list.join(", ")
                    );
                }
            }
        }
    }
    print_output(sinks, json_mode, &responses::ack(ids, remove))
}

pub fn cmd_batch(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    ops_json: &str,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let ops_value: Vec<Value> =
        serde_json::from_str(ops_json).context("parsing batch operations JSON")?;

    let mut batch_ops = Vec::with_capacity(ops_value.len());
    for (idx, op_value) in ops_value.iter().enumerate() {
        let op_obj = op_value
            .as_object()
            .with_context(|| format!("batch op[{idx}]: expected object"))?;
        batch_ops.push(BatchCommentOp::from_json_object(op_obj, idx)?);
    }

    let created_ids = operations::batch::batch_comment(system, &path, config, &batch_ops)?;
    print_output(sinks, json_mode, &responses::batch(&created_ids))
}

fn resolve_comment_position(
    reply_to: Option<&str>,
    after_comment: Option<&str>,
    after_heading: Option<&str>,
    after_line: Option<usize>,
) -> InsertPosition {
    InsertPosition::from_hints(reply_to, after_comment, after_heading, after_line)
}

pub fn cmd_comment(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    cp: &CommentParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, cp.file)?;

    // Replies always go after their parent — explicit placement is ignored.
    let position = resolve_comment_position(
        cp.reply_to,
        cp.after_comment,
        cp.after_heading,
        cp.after_line,
    );

    let mut params = operations::CreateCommentParams::new(cp.content, &position);
    params.attachments = cp.attachments;
    params.auto_ack = cp.auto_ack;
    params.remargin_kind = cp.remargin_kind;
    params.reply_to = cp.reply_to;
    params.sandbox = cp.sandbox;
    params.to = cp.to;

    let new_id = operations::create_comment(system, &path, config, &params)?;

    // Write to stdout if stdin mode.
    if cp.file == "-" {
        let updated = system.read_to_string(&path)?;
        out_raw(sinks, &updated)?;
    }

    print_output(sinks, cp.json_mode, &responses::comment_created(&new_id))
}

pub fn cmd_comments(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    kind_filter: &[String],
    json_mode: bool,
    pretty: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let doc = parser::parse_file(system, &path)?;
    // Apply the shared kind filter from `remargin-core::kind` so this
    // surface stays in lockstep with `remargin query` — the
    // design doc explicitly calls out the previous divergence as a bug.
    let comments: Vec<_> = doc
        .comments()
        .into_iter()
        .filter(|cm| matches_kind_filter(cm.kinds(), kind_filter))
        .collect();

    if pretty {
        let formatted = display::format_comments_pretty(file, &comments);
        out(sinks, &formatted)
    } else if json_mode {
        out_json(sinks, &json!({ "comments": comments }))
    } else {
        for cm in &comments {
            let ack_status = if cm.ack.is_empty() {
                "pending"
            } else {
                "acked"
            };
            out(
                sinks,
                &format!(
                    "{} {} ({}) [{}] {}",
                    cm.id,
                    cm.author,
                    author_type_str(&cm.author_type),
                    ack_status,
                    truncate_content(&cm.content, 60_usize),
                ),
            )?;
        }
        Ok(())
    }
}

pub fn cmd_delete(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    ids: &[String],
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    operations::delete_comments(system, &path, config, &id_refs)?;
    print_output(sinks, json_mode, &responses::comments_deleted(ids))
}

pub fn cmd_edit(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    p: &EditParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, p.file)?;
    operations::edit_comment(system, &path, config, p.id, p.content, p.remargin_kind)?;
    print_output(sinks, p.json_mode, &responses::comment_edited(p.id))
}

pub fn cmd_get(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    gp: &GetParams<'_>,
) -> Result<()> {
    let target_buf = expand_cli_path(system, gp.path)?;
    let target = target_buf.as_path();

    if gp.binary {
        return cmd_get_binary(sinks, system, cwd, config, gp, target);
    }

    if gp.out.is_some() {
        bail!("--out requires --binary");
    }

    let lines = match (gp.start, gp.end) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    };

    if gp.json_mode && gp.line_numbers {
        let content = document::get(
            system,
            cwd,
            target,
            lines,
            false,
            config.unrestricted,
            &config.trusted_roots,
        )?;
        let start_num = lines.map_or(1, |(s, _)| s);
        let json_lines: Vec<Value> = content
            .split('\n')
            .enumerate()
            .map(|(i, text)| json!({ "line": start_num + i, "text": text }))
            .collect();
        print_output(sinks, true, &json!({ "lines": json_lines }))
    } else {
        let content = document::get(
            system,
            cwd,
            target,
            lines,
            gp.line_numbers,
            config.unrestricted,
            &config.trusted_roots,
        )?;
        if gp.json_mode {
            print_output(sinks, true, &json!({ "content": content }))
        } else {
            out_raw(sinks, &content)
        }
    }
}

/// Binary-mode `get` dispatch. Reads bytes once through the shared
/// core helper, then surfaces them in the caller's chosen shape:
/// - `--out <path>` — write bytes to disk, stdout shows `{path, size_bytes, mime}`.
/// - `--json` — base64-encoded `content` in the payload alongside mime / size.
/// - default — raw bytes to stdout (so `remargin get --binary x.png > out.png` works).
///
/// Incompatible flags (`--start`, `--end`, `-n`) are rejected up front so
/// binary requests never silently drop text-mode options.
pub fn cmd_get_binary(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    gp: &GetParams<'_>,
    target: &Path,
) -> Result<()> {
    if gp.start.is_some() || gp.end.is_some() {
        bail!("--start / --end are not supported with --binary");
    }
    if gp.line_numbers {
        bail!("--line-numbers is not supported with --binary");
    }

    let payload = document::read_binary(
        system,
        cwd,
        target,
        config.unrestricted,
        &config.trusted_roots,
    )?;

    if let Some(out_path) = gp.out {
        system
            .write(out_path, &payload.bytes)
            .with_context(|| format!("writing {}", out_path.display()))?;
        let summary = json!({
            "mime": payload.mime,
            "out": out_path,
            "path": payload.path,
            "size_bytes": payload.size_bytes,
        });
        return print_output(sinks, gp.json_mode, &summary);
    }

    if gp.json_mode {
        let encoded = BASE64_STANDARD.encode(&payload.bytes);
        return print_output(
            sinks,
            true,
            &json!({
                "binary": true,
                "content": encoded,
                "mime": payload.mime,
                "path": payload.path,
                "size_bytes": payload.size_bytes,
            }),
        );
    }

    // Non-JSON, no --out: raw bytes to stdout so shell redirection works.
    sinks
        .stdout
        .write_all(&payload.bytes)
        .context("writing bytes to stdout")
}

/// Resolve and print the identity the CLI's active flag set produces.
///
/// Routes through the same [`ResolvedConfig::resolve`][config::ResolvedConfig::resolve]
/// every mutating op uses, so `remargin identity --config <path>` (or
/// `--identity` + `--type` manual, or a `--type`-filtered walk) returns
/// the same identity the next write would attribute to.
///
/// A branch-3 walk that cannot match the supplied filters is treated as
/// "nothing found" rather than an error: the JSON output collapses to
/// `{ "found": false }`, preserving the historical read-only-diagnostic
/// contract and letting the Obsidian plugin call this during startup
/// without having to special-case transient "no config yet" states.
/// Other resolver errors (unknown type strings, strict-mode registry
/// misses, etc.) still propagate.
pub fn cmd_identity(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: Option<&IdentityAction>,
    identity_args: &IdentityArgs,
    json_mode: bool,
) -> Result<()> {
    match action {
        Some(IdentityAction::Create {
            identity,
            r#type,
            key,
            output_args,
        }) => cmd_identity_create(sinks, identity, r#type, key.as_deref(), output_args.json),
        Some(IdentityAction::Show {
            identity_args: nested,
            output_args,
        }) => cmd_identity_show(sinks, system, cwd, nested, output_args.json),
        None => cmd_identity_show(sinks, system, cwd, identity_args, json_mode),
    }
}

pub fn cmd_identity_show(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    identity_args: &IdentityArgs,
    json_mode: bool,
) -> Result<()> {
    let (flags, _assets_dir) = build_identity_flags(system, identity_args, None)?;
    let report = resolve_identity_report(system, cwd, &flags)?;
    render::render_identity_report(sinks, &report, json_mode)
}

/// Print a ready-to-use identity YAML block to stdout.
///
/// `mode:` is deliberately omitted — mode is a tree property resolved
/// by walk-up, not an identity-level declaration. `key:` is emitted
/// verbatim when supplied; an absent key is valid in non-strict modes.
/// `--json` returns the same fields as a structured payload so tooling
/// (the Obsidian plugin, scripts) can pick them up without re-parsing
/// YAML.
pub fn cmd_identity_create(
    sinks: &mut IoSinks<'_>,
    identity: &str,
    author_type: &str,
    key: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    // Validate the author type early so an invalid value fails before
    // any output is emitted (stdout stays clean for redirection).
    config::parse_author_type(author_type)
        .with_context(|| format!("invalid --type value: {author_type}"))?;

    if json_mode {
        return print_output(
            sinks,
            true,
            &json!({
                "identity": identity,
                "type": author_type,
                "key": key,
            }),
        );
    }
    let mut out_str = format!("identity: {identity}\ntype: {author_type}\n");
    if let Some(k) = key {
        use core::fmt::Write as _;
        let _ = writeln!(out_str, "key: {k}");
    }
    out_raw(sinks, &out_str)
}

/// Dispatch `remargin permissions <show|check>`.
///
/// `show` prints the resolved permissions tree at `cwd`. `check`
/// canonicalises its target path, asks the inspector whether any
/// `restrict` or `deny_ops` rule covers it, and exits gitignore-style:
/// 0 when restricted, 1 when not. Both paths support `--json`.
/// Wire the CLI `activity` subcommand to the
/// [`activity::gather_activity`] core.
///
/// Output mode follows the workspace `--json` convention:
/// `--json` (default) emits the structured `ActivityResult`;
/// `--pretty` switches to a human-readable timeline. Both flags
/// at once is rejected (clap-level via the surrounding
/// [`OutputArgs::json`] flag plus the local `pretty` boolean).
///
/// Identity is read-only here — the quartet resolves only the
/// caller name driving the per-file cutoff. No signing, no key
/// requirement.
pub fn cmd_activity(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    p: &ActivityParams<'_>,
) -> Result<()> {
    if p.pretty && p.json_mode {
        bail!("--pretty and --json are mutually exclusive");
    }

    let resolved_path = match p.explicit_path {
        Some(path) => {
            let expanded = expand_cli_pathbuf(system, path)?;
            if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            }
        }
        None => cwd.to_path_buf(),
    };

    let cutoff = match p.since {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .with_context(|| format!("--since: invalid ISO 8601 timestamp {raw:?}"))?,
        ),
        None => None,
    };

    let (flags, _assets_dir) = build_identity_flags(system, p.identity_args, None)?;
    let resolved = ResolvedConfig::resolve(system, cwd, &flags, None)?;
    let caller = resolved
        .identity
        .as_deref()
        .context("activity: caller identity required (declare via --identity / --config)")?;

    let result = activity::gather_activity(system, &resolved_path, cutoff, caller)?;

    if p.pretty {
        render::emit_activity_pretty(sinks, &result)?;
    } else {
        let value = serde_json::to_value(&result).context("serializing activity result")?;
        print_output(sinks, true, &value)?;
    }
    Ok(())
}

pub fn cmd_doctor(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    command: &Commands,
) -> Result<()> {
    let Commands::Doctor {
        user_settings,
        output_args,
    } = command
    else {
        bail!("internal: cmd_doctor called with wrong subcommand");
    };
    let user_settings_path = if let Some(p) = user_settings.as_deref() {
        p.to_path_buf()
    } else {
        expand_cli_pathbuf(system, Path::new(DEFAULT_USER_SETTINGS))?
    };
    let report = permissions_doctor::run_doctor(system, cwd, &user_settings_path)?;
    if output_args.json {
        let value = serde_json::to_value(&report).context("serializing doctor report")?;
        print_output(sinks, true, &value)?;
    } else {
        render::emit_doctor_text(sinks, &report, output_args.verbose)?;
    }
    // Exit non-zero when findings are present.
    if !report.is_clean() {
        bail!("doctor found {} finding(s)", report.findings.len());
    }
    Ok(())
}

pub fn cmd_permissions(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: &PermissionsAction,
) -> Result<()> {
    match action {
        PermissionsAction::Show { output_args } => {
            let report = permissions_inspect::show(system, cwd)?;
            if output_args.json {
                let value =
                    serde_json::to_value(&report).context("serializing permissions show output")?;
                print_output(sinks, true, &value)?;
            } else {
                render::emit_permissions_show_text(sinks, cwd, &report)?;
            }
            Ok(())
        }
        PermissionsAction::Check {
            path,
            why,
            output_args,
        } => {
            let expanded = expand_cli_pathbuf(system, path)?;
            let target = if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            };
            let report = permissions_inspect::check(system, cwd, &target, *why)?;
            if output_args.json {
                let value = serde_json::to_value(&report)
                    .context("serializing permissions check output")?;
                print_output(sinks, true, &value)?;
            } else {
                render::emit_permissions_check_text(sinks, &report, *why)?;
            }
            // Gitignore-style exit code: 0 when restricted, 1 otherwise.
            // We have already printed our payload, so signal "miss" with
            // a sentinel error that `main` recognises as
            // [`EXIT_NOT_RESTRICTED`] and renders silently (no
            // "error: ..." prefix).
            if report.restricted {
                Ok(())
            } else {
                bail!("{PERMISSIONS_NOT_RESTRICTED_MARKER}");
            }
        }
    }
}

/// Wire the CLI `restrict` subcommand to the
/// [`permissions_restrict::restrict`] core.
///
/// `user_settings_explicit` lets tests pin a hermetic location for
/// the user-scope file. When `None`, the function expands
/// [`DEFAULT_USER_SETTINGS`] through the active `System`.
pub fn cmd_restrict(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    p: &RestrictParams<'_>,
) -> Result<()> {
    let user_scope = match p.user_settings_explicit {
        Some(explicit) => expand_cli_pathbuf(system, explicit)?,
        None => expand_cli_path(system, DEFAULT_USER_SETTINGS)?,
    };
    let anchor = permissions_restrict::find_claude_anchor(system, cwd)?;
    let project_scope = anchor.join(".claude/settings.local.json");
    let settings_files = vec![project_scope, user_scope];

    let args = permissions_restrict::RestrictArgs::new(
        String::from(p.path),
        p.also_deny_bash.to_vec(),
        p.cli_allowed,
    );
    let outcome = permissions_restrict::restrict(system, cwd, &args, &settings_files)?;

    if p.json_mode {
        let value = serde_json::json!({
            "absolute_path": outcome.absolute_path.display().to_string(),
            "anchor": outcome.anchor.display().to_string(),
            "claude_files_touched": outcome
                .claude_files_touched
                .iter()
                .map(|file| file.display().to_string())
                .collect::<Vec<_>>(),
            "rules_applied": outcome.rules_applied,
            "yaml_was_created": outcome.yaml_was_created,
        });
        print_output(sinks, true, &value)?;
    } else {
        render::emit_restrict_summary(sinks, &outcome)?;
    }
    Ok(())
}

/// Wire the CLI `unprotect` subcommand to the
/// [`permissions_unprotect::unprotect`] core.
///
/// `_user_settings_explicit` is accepted on the CLI for symmetry
/// with `restrict` but ignored here: the unprotect path consults
/// the sidecar's `added_to_files` list (the resolved settings paths
/// captured at apply time), so the reversal scrubs exactly the files
/// the corresponding `restrict` touched.
pub fn cmd_unprotect(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    path: &str,
    strict: bool,
    _user_settings_explicit: Option<&Path>,
    json_mode: bool,
) -> Result<()> {
    let args = permissions_unprotect::UnprotectArgs::new(String::from(path)).with_strict(strict);
    let outcome = permissions_unprotect::unprotect(system, cwd, &args)?;

    if json_mode {
        let value = serde_json::json!({
            "absolute_path": outcome.absolute_path.display().to_string(),
            "anchor": outcome.anchor.display().to_string(),
            "claude_files_touched": outcome
                .claude_files_touched
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "rules_removed": outcome.rules_removed,
            "warnings": outcome.warnings,
            "yaml_entry_removed": outcome.yaml_entry_removed,
        });
        print_output(sinks, true, &value)?;
    } else {
        render::emit_unprotect_summary(sinks, &outcome)?;
    }
    Ok(())
}

pub fn cmd_prompt_resolve(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let target_buf = expand_cli_path(system, file)?;
    let absolute = if target_buf.is_absolute() {
        target_buf
    } else {
        cwd.join(&target_buf)
    };
    let resolved = config::system_prompt::resolve_system_prompt(system, &absolute)?;
    if json_mode {
        let value = serde_json::to_value(&resolved).context("serializing prompt_resolve output")?;
        return print_output(sinks, true, &value);
    }
    render::write_prompt_resolve_text(sinks, &absolute, &resolved)
}

pub fn cmd_prompt_set(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    params: &PromptSetParams<'_>,
) -> Result<()> {
    let absolute = absolute_folder(system, params.cwd, params.folder)?;
    let body = match params.prompt_flag {
        Some(p) => String::from(p),
        None => read_prompt_from_stdin()?,
    };
    if body.is_empty() {
        bail!("prompt body is required (pass --prompt or pipe via stdin)");
    }
    if params.name.is_empty() {
        bail!("--name is required");
    }
    let outcome =
        operations::prompt::set(system, &absolute, Some(params.name), &body, params.config)
            .with_context(|| format!("setting prompt at {}", absolute.display()))?;
    if params.json_mode {
        let value = serde_json::to_value(&outcome).context("serializing prompt_set output")?;
        return print_output(sinks, true, &value);
    }
    writeln!(
        sinks.stderr,
        "Prompt set at {} (created={}, noop={})",
        outcome.source.display(),
        outcome.created,
        outcome.noop,
    )
    .context("writing to stderr")?;
    Ok(())
}

pub fn cmd_prompt_delete(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    folder: &str,
    json_mode: bool,
) -> Result<()> {
    let absolute = absolute_folder(system, cwd, folder)?;
    let outcome = operations::prompt::delete(system, &absolute, config)
        .with_context(|| format!("deleting prompt at {}", absolute.display()))?;
    if json_mode {
        let value = serde_json::to_value(&outcome).context("serializing prompt_delete output")?;
        return print_output(sinks, true, &value);
    }
    if outcome.absent {
        writeln!(
            sinks.stderr,
            "No prompt to delete at {}",
            outcome.source.display(),
        )
        .context("writing to stderr")?;
    } else {
        writeln!(
            sinks.stderr,
            "Prompt deleted at {} (left_empty={})",
            outcome.source.display(),
            outcome.left_empty,
        )
        .context("writing to stderr")?;
    }
    Ok(())
}

pub fn cmd_prompt_list(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    folder: &str,
    json_mode: bool,
) -> Result<()> {
    let absolute = absolute_folder(system, cwd, folder)?;
    let entries = operations::prompt::list(system, &absolute)
        .with_context(|| format!("listing prompts under {}", absolute.display()))?;
    if json_mode {
        let value = serde_json::to_value(&entries).context("serializing prompt_list output")?;
        return print_output(sinks, true, &json!({ "entries": value }));
    }
    if entries.is_empty() {
        writeln!(
            sinks.stderr,
            "No declared prompts under {}",
            absolute.display(),
        )
        .context("writing to stderr")?;
        return Ok(());
    }
    for entry in &entries {
        let label = entry.name.as_deref().unwrap_or("(unnamed)");
        let chars = entry.prompt.chars().count();
        writeln!(
            sinks.stdout,
            "{}\t{}\t{} chars",
            entry.source.display(),
            label,
            chars,
        )
        .context("writing to stdout")?;
    }
    Ok(())
}

fn absolute_folder(system: &dyn System, cwd: &Path, folder: &str) -> Result<PathBuf> {
    let expanded = expand_cli_path(system, folder)?;
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    Ok(absolute)
}

fn read_prompt_from_stdin() -> Result<String> {
    use std::io::IsTerminal as _;
    let stdin = stdin_handle();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    let mut buf = String::new();
    stdin
        .lock()
        .read_to_string(&mut buf)
        .context("reading prompt body from stdin")?;
    Ok(String::from(buf.trim_end_matches('\n')))
}

pub fn cmd_resolve_mode(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    json_mode: bool,
) -> Result<()> {
    let resolved = config::resolve_mode(system, cwd)?;
    let source = resolved.source.as_ref().map(|p| p.display().to_string());
    let value = json!({
        "mode": resolved.mode.as_str(),
        "source": source,
    });
    if json_mode {
        print_output(sinks, true, &value)?;
    } else {
        writeln!(sinks.stderr, "Mode:   {}", resolved.mode.as_str())
            .context("writing to stderr")?;
        match &source {
            Some(path) => {
                writeln!(sinks.stderr, "Source: {path}").context("writing to stderr")?;
            }
            None => {
                writeln!(sinks.stderr, "Source: <default>").context("writing to stderr")?;
            }
        }
    }
    Ok(())
}

pub fn cmd_keygen(sinks: &mut IoSinks<'_>, system: &dyn System, output: &Path) -> Result<()> {
    use ssh_key::{Algorithm, LineEnding, PrivateKey, rand_core::OsRng};

    let private_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|err| anyhow::anyhow!("key generation failed: {err}"))?;

    let private_pem = private_key
        .to_openssh(LineEnding::LF)
        .map_err(|err| anyhow::anyhow!("encoding private key: {err}"))?;

    let public_key = private_key.public_key();
    let public_openssh = public_key
        .to_openssh()
        .map_err(|err| anyhow::anyhow!("encoding public key: {err}"))?;

    system
        .write(output, private_pem.as_bytes())
        .with_context(|| format!("writing private key to {}", output.display()))?;

    let pub_path = output.with_extension("pub");
    system
        .write(&pub_path, public_openssh.as_bytes())
        .with_context(|| format!("writing public key to {}", pub_path.display()))?;

    writeln!(sinks.stderr, "Private key: {}", output.display()).context("writing to stderr")?;
    writeln!(sinks.stderr, "Public key:  {}", pub_path.display()).context("writing to stderr")?;

    Ok(())
}

pub fn cmd_lint(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let report = linter::lint_doc(system, &path)?;

    if json_mode {
        print_output(sinks, true, &report.to_json())?;
    } else {
        write!(sinks.stderr, "{}", report.format_text()).context("writing to stderr")?;
    }

    if !report.is_clean() {
        anyhow::bail!("Lint errors found");
    }
    Ok(())
}

pub fn cmd_ls(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    path_str: &str,
    json_mode: bool,
) -> Result<()> {
    let target_buf = expand_cli_path(system, path_str)?;
    let target = target_buf.as_path();
    let entries = document::ls(system, cwd, target, config)?;

    if json_mode {
        print_output(sinks, true, &json!({ "entries": entries }))
    } else {
        for entry in &entries {
            let kind = if entry.is_dir { "d" } else { "-" };
            let size_str = entry
                .size
                .map_or_else(|| String::from("-"), |s| format!("{s}"));
            let pending_str = entry
                .remargin_pending
                .map(|p| format!(" [{p} pending]"))
                .unwrap_or_default();
            out(
                sinks,
                &format!(
                    "{kind} {size_str:>8} {}{}",
                    entry.path.display(),
                    pending_str,
                ),
            )?;
        }
        Ok(())
    }
}

pub fn cmd_metadata(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    path_str: &str,
    json_mode: bool,
) -> Result<()> {
    let target_buf = expand_cli_path(system, path_str)?;
    let target = target_buf.as_path();
    let meta = document::metadata(
        system,
        cwd,
        target,
        config.unrestricted,
        &config.trusted_roots,
    )?;

    print_output(sinks, json_mode, &meta.to_json(false))
}

/// Route a `plan` subcommand to the correct per-op projection.
///
/// Lightweight ops that have not yet been wired surface a deliberate
/// "not yet landed" error so callers discover the subcommand tree and
/// failures are loud. `plan write` is fully wired.
pub fn cmd_plan(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    action: &PlanAction,
    json_mode: bool,
) -> Result<()> {
    // `Comment` / `Write` arms need owned buffers that outlive the
    // `PlanRequest` (it borrows `&str` / `ProjectCommentParams<'_>`).
    // Stage them here so the borrows survive through `plan_ops::dispatch`.
    // Initialized to empty defaults; the `Comment` / `Write` helpers
    // overwrite them in place before the borrow flows out.
    let mut comment_body = String::new();
    let mut write_body = String::new();
    let mut attach_refs: Vec<&str> = Vec::new();
    let mut position = InsertPosition::Append;

    let request = match action {
        PlanAction::Ack { .. } => build_plan_ack(action, system, cwd)?,
        PlanAction::Batch { .. } => build_plan_batch(action, system, cwd)?,
        PlanAction::Claude { action: claude, .. } => match claude {
            PlanClaudeAction::Restrict { .. } => build_plan_claude_restrict(claude, system, cwd)?,
            PlanClaudeAction::Unrestrict { .. } => build_plan_claude_unrestrict(claude, cwd)?,
        },
        PlanAction::Comment { .. } => build_plan_comment(
            action,
            system,
            cwd,
            &mut comment_body,
            &mut position,
            &mut attach_refs,
        )?,
        PlanAction::Cp { .. } => build_plan_cp(action, system)?,
        PlanAction::Delete { .. } => build_plan_delete(action, system, cwd)?,
        PlanAction::Edit { .. } => build_plan_edit(action, system, cwd)?,
        PlanAction::Mv { .. } => build_plan_mv(action, system)?,
        PlanAction::Purge { .. } => build_plan_purge(action, system, cwd)?,
        PlanAction::React { .. } => build_plan_react(action, system, cwd)?,
        PlanAction::SandboxAdd { .. } => build_plan_sandbox_add(action, system, cwd)?,
        PlanAction::SandboxRemove { .. } => build_plan_sandbox_remove(action, system, cwd)?,
        PlanAction::Sign { .. } => build_plan_sign(action, system, cwd)?,
        PlanAction::Write { .. } => build_plan_write(action, system, &mut write_body)?,
    };

    let report = plan_ops::dispatch(system, cwd, config, &request)?;
    let value = serde_json::to_value(&report).context("serializing plan report")?;

    // Config-mutation plans get a structured text block in text mode so
    // the multi-file projection is readable. JSON mode still emits the
    // full PlanReport payload.
    if !json_mode {
        if report.config_diff.is_some() {
            return render::emit_plan_restrict_text(sinks, &report);
        }
        if report.unprotect_diff.is_some() {
            return render::emit_plan_unprotect_text(sinks, &report);
        }
    }

    print_output(sinks, json_mode, &value)
}

fn build_plan_ack(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Ack {
        path, ids, remove, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Ack {
        path: resolve_doc_path(system, cwd, path)?,
        ids: ids.clone(),
        remove: *remove,
    })
}

fn build_plan_batch(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Batch { path, ops_file, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Batch {
        path: resolve_doc_path(system, cwd, path)?,
        ops: read_plan_batch_ops(system, ops_file)?,
    })
}

fn build_plan_comment<'cmd>(
    action: &'cmd PlanAction,
    system: &dyn System,
    cwd: &Path,
    comment_body: &'cmd mut String,
    position: &'cmd mut InsertPosition,
    attach_refs: &'cmd mut Vec<&'cmd str>,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::Comment {
        path,
        content,
        after_comment,
        after_heading,
        after_line,
        attach_names,
        auto_ack,
        no_auto_ack,
        reply_to,
        sandbox,
        to,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    let doc_path = resolve_doc_path(system, cwd, path)?;
    *comment_body = match content {
        Some(s) => s.clone(),
        None => read_stdin()?,
    };
    *position = resolve_comment_position(
        reply_to.as_deref(),
        after_comment.as_deref(),
        after_heading.as_deref(),
        *after_line,
    );
    *attach_refs = attach_names.iter().map(String::as_str).collect();
    let params = projections::ProjectCommentParams::new(comment_body, position)
        .with_attachment_filenames(attach_refs)
        .with_auto_ack(tri_state_flag(*auto_ack, *no_auto_ack))
        .with_reply_to(reply_to.as_deref())
        .with_sandbox(*sandbox)
        .with_to(to);
    Ok(plan_ops::PlanRequest::Comment {
        path: doc_path,
        params,
    })
}

fn build_plan_delete(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Delete { path, ids, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Delete {
        path: resolve_doc_path(system, cwd, path)?,
        ids: ids.clone(),
    })
}

fn build_plan_edit<'cmd>(
    action: &'cmd PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::Edit {
        path, id, content, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Edit {
        path: resolve_doc_path(system, cwd, path)?,
        id,
        content,
    })
}

fn build_plan_cp(
    action: &PlanAction,
    system: &dyn System,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Cp {
        src, dst, force, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Cp {
        src: expand_cli_path(system, src)?,
        dst: expand_cli_path(system, dst)?,
        force: *force,
    })
}

fn build_plan_mv(
    action: &PlanAction,
    system: &dyn System,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Mv {
        src, dst, force, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Mv {
        src: expand_cli_path(system, src)?,
        dst: expand_cli_path(system, dst)?,
        force: *force,
    })
}

fn build_plan_purge(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Purge {
        path, recursive, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Purge {
        path: resolve_purge_path(system, cwd, path, *recursive)?,
        recursive: *recursive,
    })
}

fn build_plan_react<'cmd>(
    action: &'cmd PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::React {
        path,
        id,
        emoji,
        remove,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::React {
        path: resolve_doc_path(system, cwd, path)?,
        id,
        emoji,
        remove: *remove,
    })
}

/// Anchor-walk failure surfaces via the projection's reject path; on
/// that path we still produce a report rather than bail here. The
/// fallback project-scope path is unused on the reject branch.
fn build_plan_claude_restrict(
    action: &PlanClaudeAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanClaudeAction::Restrict {
        path,
        also_deny_bash,
        cli_allowed,
        user_settings,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanClaudeAction variant");
    };
    let user_scope = match user_settings {
        Some(explicit) => expand_cli_pathbuf(system, explicit)?,
        None => expand_cli_path(system, DEFAULT_USER_SETTINGS)?,
    };
    let project_scope = permissions_restrict::find_claude_anchor(system, cwd).map_or_else(
        |_err| cwd.join(".claude/settings.local.json"),
        |anchor| anchor.join(".claude/settings.local.json"),
    );
    Ok(plan_ops::PlanRequest::Restrict {
        args: permissions_restrict::RestrictArgs::new(
            path.clone(),
            also_deny_bash.clone(),
            *cli_allowed,
        ),
        cwd: cwd.to_path_buf(),
        settings_files: vec![project_scope, user_scope],
    })
}

fn build_plan_sandbox_add(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::SandboxAdd { path, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::SandboxAdd {
        path: resolve_doc_path(system, cwd, path)?,
    })
}

fn build_plan_sandbox_remove(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::SandboxRemove { path, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::SandboxRemove {
        path: resolve_doc_path(system, cwd, path)?,
    })
}

fn build_plan_sign(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Sign {
        path,
        ids,
        all_mine,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Sign {
        path: resolve_doc_path(system, cwd, path)?,
        selection: build_sign_selection(*all_mine, ids)?,
    })
}

fn build_plan_claude_unrestrict(
    action: &PlanClaudeAction,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanClaudeAction::Unrestrict { path, .. } = action else {
        bail!("internal: helper called with wrong PlanClaudeAction variant");
    };
    Ok(plan_ops::PlanRequest::Unprotect {
        args: permissions_unprotect::UnprotectArgs::new(path.clone()),
        cwd: cwd.to_path_buf(),
    })
}

fn build_plan_write<'cmd>(
    action: &PlanAction,
    system: &dyn System,
    write_body: &'cmd mut String,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::Write {
        path,
        content,
        binary,
        create,
        lines,
        raw,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    *write_body = match content {
        Some(s) => s.clone(),
        None => read_stdin()?,
    };
    let line_range = lines.as_deref().map(parse_line_range).transpose()?;
    let opts = document::WriteOptions::new()
        .binary(*binary)
        .create(*create)
        .lines(line_range)
        .raw(*raw);
    Ok(plan_ops::PlanRequest::Write {
        path: expand_cli_path(system, path)?,
        content: write_body,
        opts,
    })
}

/// Render a `plan restrict` [`PlanReport`] as a structured text block.
/// Mirrors the JSON shape: anchor + `would_commit`/`noop` header, one
/// section per touched file, then conflicts. Emitted on stdout via
/// the standard `out` helper so existing pipe-friendly behaviour is
/// preserved.
/// Read a JSON file (or stdin when `path == "-"`) into a vector of
/// [`projections::ProjectBatchOp`] values for `plan batch`.
fn read_plan_batch_ops(
    system: &dyn System,
    path: &str,
) -> Result<Vec<projections::ProjectBatchOp>> {
    let json_text = if path == "-" {
        read_stdin()?
    } else {
        system
            .read_to_string(Path::new(path))
            .with_context(|| format!("reading plan batch ops file {path}"))?
    };
    let raw: Value =
        serde_json::from_str(&json_text).context("parsing plan batch ops JSON body")?;
    let arr = raw
        .as_array()
        .context("plan batch ops JSON must be an array of objects")?;

    let mut ops: Vec<projections::ProjectBatchOp> = Vec::with_capacity(arr.len());
    for (idx, entry) in arr.iter().enumerate() {
        let obj = entry
            .as_object()
            .with_context(|| format!("plan batch op[{idx}]: expected object"))?;
        ops.push(projections::ProjectBatchOp::from_json_object(obj, idx)?);
    }
    Ok(ops)
}

pub fn cmd_purge(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    recursive: bool,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_purge_path(system, cwd, file, recursive)?;

    if recursive {
        let result = purge::purge_dir(system, &path, config)?;
        return print_output(sinks, json_mode, &result.to_json(cwd));
    }

    if system.is_dir(&path).unwrap_or(false) {
        anyhow::bail!(
            "target is a directory: {file} (pass --recursive to purge every .md file under it)"
        );
    }
    let result = purge::purge(system, &path, config)?;
    print_output(sinks, json_mode, &result.to_json())
}

pub fn cmd_query(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &QueryParams<'_>,
) -> Result<()> {
    let target = cwd.join(expand_cli_path(system, params.path)?);
    let filter = build_query_filter(config, params)?;
    let results = query::query(system, &target, &filter)?;
    render::render_query_output(sinks, &results, params, filter.pending_label())
}

fn build_query_filter(
    config: &ResolvedConfig,
    params: &QueryParams<'_>,
) -> Result<query::QueryFilter> {
    let since_dt = params
        .since
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .with_context(|| format!("invalid timestamp: {s}"))
        })
        .transpose()?;

    let mut filter = query::QueryFilter::default();
    filter.author = params.author.map(String::from);
    filter.comment_id = params.comment_id.map(String::from);
    filter.expanded = params.expanded;
    filter.pending = params.pending.any;
    filter.pending_for = params.pending.for_user.map(String::from);
    filter.remargin_kind = params.remargin_kind.to_vec();
    filter.since = since_dt;
    filter.summary = matches!(params.output, QueryOutputMode::Summary);
    filter = filter.with_caller_identity(
        params.pending.for_me,
        params.pending.broadcast,
        config.identity.clone(),
    )?;
    if let Some(pattern) = params.content_regex {
        filter = filter.with_content_regex(pattern, params.ignore_case)?;
    }
    Ok(filter)
}

pub fn cmd_search(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    params: &SearchParams<'_>,
) -> Result<()> {
    let target = cwd.join(expand_cli_path(system, params.path)?);

    let scope = match params.scope {
        "body" => search::SearchScope::Body,
        "comments" => search::SearchScope::Comments,
        _ => search::SearchScope::All,
    };

    let options = search::SearchOptions::new(String::from(params.pattern))
        .context_lines(params.context)
        .ignore_case(params.ignore_case)
        .regex(params.regex)
        .scope(scope);

    let results = search::search(system, cwd, &target, &options)?;

    if params.json_mode {
        print_output(sinks, true, &json!({ "matches": results }))
    } else {
        for m in &results {
            let loc = match m.location {
                search::MatchLocation::Body => "body",
                search::MatchLocation::Comment => "comment",
                _ => "unknown",
            };
            for line in &m.before {
                out(sinks, &format!("  {line}"))?;
            }
            out(
                sinks,
                &format!("{}:{}  [{}]  {}", m.path.display(), m.line, loc, m.text),
            )?;
            for line in &m.after {
                out(sinks, &format!("  {line}"))?;
            }
        }
        Ok(())
    }
}

pub fn cmd_replace(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &ReplaceParams<'_>,
) -> Result<()> {
    let target = cwd.join(expand_cli_path(system, params.path)?);

    let report = replace::replace(system, cwd, &target, &params.options, config)?;

    if params.json_mode {
        return print_output(sinks, true, &serde_json::to_value(&report)?);
    }

    for file in &report.files {
        let line = match &file.error {
            Some(err) => format!("{}: skipped ({err})", file.path.display()),
            None if file.changed => {
                let verb = if report.dry_run {
                    "would change"
                } else {
                    "changed"
                };
                format!(
                    "{}: {verb} ({} replacement{})",
                    file.path.display(),
                    file.replacements,
                    plural_suffix(file.replacements),
                )
            }
            None => continue,
        };
        out(sinks, &line)?;
    }
    let header = if report.dry_run { "dry-run: " } else { "" };
    out(
        sinks,
        &format!(
            "{header}{} replacement{} across {} file{} ({} failed)",
            report.total_replacements,
            plural_suffix(report.total_replacements),
            report.files_changed,
            plural_suffix(report.files_changed),
            report.files_failed,
        ),
    )
}

/// `""` for exactly one, `"s"` otherwise — for human-readable count
/// messages.
const fn plural_suffix(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub fn cmd_react(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &ReactParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, params.file)?;
    operations::react(
        system,
        &path,
        config,
        params.id,
        params.emoji,
        params.remove,
    )?;
    print_output(
        sinks,
        params.json_mode,
        &responses::react(params.emoji, params.id, params.remove),
    )
}

/// Render a single registry participant as a JSON object for
/// `remargin registry show --json`. `display_name` always appears;
/// when absent in the registry it falls back to the participant id
/// so clients never have to handle a null value.
pub fn registry_participant_json(
    name: &str,
    participant: &config::registry::RegistryParticipant,
) -> Value {
    let status = match participant.status {
        config::registry::RegistryParticipantStatus::Active => "active",
        config::registry::RegistryParticipantStatus::Revoked => "revoked",
        _ => "unknown",
    };
    let display_name = participant
        .display_name
        .clone()
        .unwrap_or_else(|| String::from(name));
    json!({
        "name": name,
        "display_name": display_name,
        "type": participant.author_type,
        "status": status,
        "pubkeys": participant.pubkeys.len(),
    })
}

pub fn cmd_registry(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: &RegistryAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        RegistryAction::Show => {
            let registry = config::load_registry(system, cwd)?.context("no registry found")?;

            if json_mode {
                let participants: Vec<Value> = registry
                    .participants
                    .iter()
                    .map(|(name, participant)| registry_participant_json(name, participant))
                    .collect();
                print_output(sinks, true, &json!({ "participants": participants }))
            } else {
                for (name, participant) in &registry.participants {
                    out(
                        sinks,
                        &render::registry_participant_pretty(name, participant),
                    )?;
                }
                Ok(())
            }
        }
    }
}

pub fn cmd_sandbox(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    action: &SandboxAction,
    json_mode: bool,
) -> Result<()> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required for sandbox operations")?;

    match action {
        SandboxAction::Add { files } => {
            let absolute: Vec<PathBuf> = files
                .iter()
                .map(|f| {
                    let expanded = expand_cli_pathbuf(system, f)?;
                    Ok::<PathBuf, anyhow::Error>(if expanded.is_absolute() {
                        expanded
                    } else {
                        cwd.join(expanded)
                    })
                })
                .collect::<Result<_>>()?;
            let result = sandbox_ops::add_to_files(system, &absolute, identity, config)?;
            render::emit_sandbox_bulk_result(sinks, &result, cwd, "added", json_mode)?;
            if result.failed.is_empty() {
                Ok(())
            } else {
                bail!("sandbox add: {} file(s) failed", result.failed.len())
            }
        }
        SandboxAction::Remove { files } => {
            let absolute: Vec<PathBuf> = files
                .iter()
                .map(|f| {
                    let expanded = expand_cli_pathbuf(system, f)?;
                    Ok::<PathBuf, anyhow::Error>(if expanded.is_absolute() {
                        expanded
                    } else {
                        cwd.join(expanded)
                    })
                })
                .collect::<Result<_>>()?;
            let result = sandbox_ops::remove_from_files(system, &absolute, identity, config)?;
            render::emit_sandbox_bulk_result(sinks, &result, cwd, "removed", json_mode)?;
            if result.failed.is_empty() {
                Ok(())
            } else {
                bail!("sandbox remove: {} file(s) failed", result.failed.len())
            }
        }
        SandboxAction::List { absolute, path } => {
            let root = match path.as_ref() {
                Some(p) => cwd.join(expand_cli_pathbuf(system, p)?),
                None => cwd.to_path_buf(),
            };
            let listings = sandbox_ops::list_for_identity(system, &root, identity)?;

            if json_mode {
                let items: Vec<Value> = listings
                    .iter()
                    .map(|l| {
                        let display_path = if *absolute {
                            l.path.display().to_string()
                        } else {
                            l.path
                                .strip_prefix(&root)
                                .unwrap_or(&l.path)
                                .display()
                                .to_string()
                        };
                        json!({
                            "path": display_path,
                            "since": l.since.to_rfc3339(),
                        })
                    })
                    .collect();
                out_json(sinks, &json!({ "files": items }))
            } else {
                for l in &listings {
                    let display_path = if *absolute {
                        l.path.display().to_string()
                    } else {
                        l.path
                            .strip_prefix(&root)
                            .unwrap_or(&l.path)
                            .display()
                            .to_string()
                    };
                    out(sinks, &display_path)?;
                }
                Ok(())
            }
        }
    }
}

pub fn cmd_cp(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &CpParams<'_>,
) -> Result<()> {
    let src = expand_cli_path(system, params.src)?;
    let dst = expand_cli_path(system, params.dst)?;

    let args = cp_op::CpArgs::new(src, dst).with_force(params.force);
    let outcome = cp_op::cp(system, cwd, config, &args)?;

    if params.json_mode {
        out_json(sinks, &serde_json::to_value(&outcome)?)
    } else {
        out(
            sinks,
            &render::cp_outcome_pretty(params.src, params.dst, &outcome),
        )
    }
}

pub fn cmd_mv(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &MvParams<'_>,
) -> Result<()> {
    let src = expand_cli_path(system, params.src)?;
    let dst = expand_cli_path(system, params.dst)?;

    let args = mv_op::MvArgs::new(src, dst).with_force(params.force);
    let outcome = mv_op::mv(system, cwd, config, &args)?;

    if params.json_mode {
        out_json(sinks, &outcome.to_json())
    } else {
        out(
            sinks,
            &render::mv_outcome_pretty(params.src, params.dst, &outcome),
        )
    }
}

pub fn cmd_rm(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let target = expand_cli_path(system, file)?;
    let result = document::rm(system, cwd, &target, config)?;

    if json_mode {
        out_json(sinks, &result.to_json(file))
    } else if result.existed {
        out(sinks, &format!("deleted: {file}"))
    } else {
        out(sinks, &format!("already absent: {file}"))
    }
}

pub fn cmd_get_image(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    sp: &GetImageParams<'_>,
) -> Result<()> {
    let target_buf = expand_cli_path(system, sp.path)?;
    let options = image_ops::GetImageOptions::from_optionals(
        sp.crop,
        sp.format,
        sp.max_bytes,
        sp.max_dimension,
    )?;
    let result = image_ops::get_image(
        system,
        cwd,
        target_buf.as_path(),
        config.unrestricted,
        &config.trusted_roots,
        &options,
    )?;
    render::render_get_image_result(sinks, system, &result, sp.out, sp.json_mode)
}

/// Expand an optional `--vault-path` for the obsidian subcommand.
#[cfg(feature = "obsidian")]
fn expand_vault_path(system: &dyn System, vault_path: Option<&Path>) -> Result<Option<PathBuf>> {
    vault_path
        .map(|v| expand_cli_pathbuf(system, v))
        .transpose()
}

#[cfg(feature = "obsidian")]
pub fn cmd_obsidian(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: &ObsidianAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        ObsidianAction::Install { vault_path } => {
            if !json_mode {
                writeln!(
                    sinks.stderr,
                    "Downloading remargin plugin v{} from GitHub Releases...",
                    obsidian::plugin_version()
                )
                .context("writing to stderr")?;
            }
            let expanded = expand_vault_path(system, vault_path.as_deref())?;
            let report = obsidian::install(system, cwd, expanded.as_deref())?;
            if json_mode {
                print_output(sinks, true, &report.to_json())
            } else {
                writeln!(sinks.stderr, "{}", report.to_text()).context("writing to stderr")?;
                Ok(())
            }
        }
        ObsidianAction::Uninstall { vault_path } => {
            let expanded = expand_vault_path(system, vault_path.as_deref())?;
            let status = obsidian::uninstall(system, cwd, expanded.as_deref())?;
            match status {
                obsidian::UninstallStatus::Removed { plugin_dir } => {
                    if json_mode {
                        print_output(
                            sinks,
                            true,
                            &json!({
                                "uninstalled": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        writeln!(
                            sinks.stderr,
                            "Uninstalled remargin plugin from {}",
                            plugin_dir.display()
                        )
                        .context("writing to stderr")?;
                        Ok(())
                    }
                }
                obsidian::UninstallStatus::NotInstalled { plugin_dir } => {
                    if json_mode {
                        print_output(
                            sinks,
                            true,
                            &json!({
                                "not_installed": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        writeln!(
                            sinks.stderr,
                            "remargin plugin not installed at {}",
                            plugin_dir.display()
                        )
                        .context("writing to stderr")?;
                        Ok(())
                    }
                }
            }
        }
    }
}

pub fn cmd_sign(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &SignParams<'_>,
) -> Result<()> {
    let SignParams {
        all_mine,
        file,
        ids,
        json_mode,
        repair_checksum,
    } = *params;
    let selection = build_sign_selection(all_mine, ids)?;
    let path = resolve_doc_path(system, cwd, file)?;
    let mut options = operations::sign::SignOptions::default();
    options.repair_checksum = repair_checksum;
    let result = operations::sign::sign_comments(system, &path, config, &selection, options)?;
    if json_mode {
        print_output(sinks, true, &result.to_json())
    } else {
        render::render_sign_result_text(sinks, &result)
    }
}

fn build_sign_selection(all_mine: bool, ids: &[String]) -> Result<operations::sign::SignSelection> {
    if !all_mine && ids.is_empty() {
        bail!("sign: pass --ids <ID[,ID...]> or --all-mine");
    }
    Ok(if all_mine {
        operations::sign::SignSelection::AllMine
    } else {
        operations::sign::SignSelection::Ids(ids.to_vec())
    })
}

fn plugin_is_installed() -> Result<bool> {
    use std::process::Command;
    let output = Command::new("claude")
        .args(["plugins", "list"])
        .output()
        .context("running 'claude plugins list' -- is Claude Code CLI installed?")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|l| l.contains(PLUGIN_REF)))
}

fn plugin_install_fresh(sinks: &mut IoSinks<'_>, scope: &str, json_mode: bool) -> Result<()> {
    use std::process::Command;
    // marketplace add is idempotent on the claude side; tolerate the
    // already-registered case by inspecting stderr only on failure.
    let market = Command::new("claude")
        .args(["plugins", "marketplace", "add", PLUGIN_MARKETPLACE_SOURCE])
        .output()
        .context("running 'claude plugins marketplace add' -- is Claude Code CLI installed?")?;
    if !market.status.success() {
        let stderr = String::from_utf8_lossy(&market.stderr);
        if !stderr.contains("already") {
            anyhow::bail!("claude plugins marketplace add failed: {stderr}");
        }
    }

    let install = Command::new("claude")
        .args(["plugins", "install", PLUGIN_REF, "--scope", scope])
        .output()
        .context("running 'claude plugins install' -- is Claude Code CLI installed?")?;
    if !install.status.success() {
        let stderr = String::from_utf8_lossy(&install.stderr);
        anyhow::bail!("claude plugins install failed: {stderr}");
    }

    if json_mode {
        print_output(
            sinks,
            true,
            &json!({
                "installed": true,
                "marketplace": PLUGIN_MARKETPLACE_SOURCE,
                "plugin": PLUGIN_REF,
                "scope": scope,
            }),
        )
    } else {
        writeln!(
            sinks.stderr,
            "Plugin {PLUGIN_REF} installed from {PLUGIN_MARKETPLACE_SOURCE} at {scope} scope.",
        )
        .context("writing to stderr")?;
        Ok(())
    }
}

fn plugin_install_update(sinks: &mut IoSinks<'_>, json_mode: bool) -> Result<()> {
    use std::process::Command;
    let market_update = Command::new("claude")
        .args(["plugins", "marketplace", "update", PLUGIN_MARKETPLACE_NAME])
        .output()
        .context("running 'claude plugins marketplace update' -- is Claude Code CLI installed?")?;
    if !market_update.status.success() {
        let stderr = String::from_utf8_lossy(&market_update.stderr);
        anyhow::bail!("claude plugins marketplace update failed: {stderr}");
    }

    let plugin_update = Command::new("claude")
        .args(["plugins", "update", PLUGIN_REF])
        .output()
        .context("running 'claude plugins update' -- is Claude Code CLI installed?")?;
    if !plugin_update.status.success() {
        let stderr = String::from_utf8_lossy(&plugin_update.stderr);
        anyhow::bail!("claude plugins update failed: {stderr}");
    }

    if json_mode {
        print_output(
            sinks,
            true,
            &json!({
                "updated": true,
                "marketplace": PLUGIN_MARKETPLACE_NAME,
                "plugin": PLUGIN_REF,
            }),
        )
    } else {
        writeln!(sinks.stderr, "Plugin {PLUGIN_REF} updated.").context("writing to stderr")?;
        Ok(())
    }
}

pub fn cmd_plugin(sinks: &mut IoSinks<'_>, action: &PluginAction, json_mode: bool) -> Result<()> {
    use std::process::Command;

    let scope_for = |local: bool| if local { "project" } else { "user" };

    match action {
        PluginAction::Install { local } => {
            let scope = scope_for(*local);
            if plugin_is_installed()? {
                plugin_install_update(sinks, json_mode)
            } else {
                plugin_install_fresh(sinks, scope, json_mode)
            }
        }
        PluginAction::Uninstall { local } => {
            let scope = scope_for(*local);
            let output = Command::new("claude")
                .args(["plugins", "uninstall", PLUGIN_REF, "--scope", scope])
                .output()
                .context("running 'claude plugins uninstall' -- is Claude Code CLI installed?")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("claude plugins uninstall failed: {stderr}");
            }

            if json_mode {
                print_output(sinks, true, &json!({ "uninstalled": true, "scope": scope }))
            } else {
                writeln!(
                    sinks.stderr,
                    "Plugin {PLUGIN_REF} uninstalled from {scope} scope.",
                )
                .context("writing to stderr")?;
                Ok(())
            }
        }
        PluginAction::Test { local } => {
            let scope = scope_for(*local);
            let status_str = if plugin_is_installed()? {
                "installed"
            } else {
                "not_installed"
            };
            if json_mode {
                print_output(
                    sinks,
                    true,
                    &json!({ "status": status_str, "scope": scope }),
                )
            } else {
                writeln!(sinks.stderr, "Plugin status ({scope}): {status_str}")
                    .context("writing to stderr")?;
                Ok(())
            }
        }
    }
}

pub fn cmd_mcp(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    startup_flags: &IdentityFlags,
    startup_assets_dir: Option<&str>,
    mcp_action: Option<&McpAction>,
    json_mode: bool,
) -> Result<()> {
    use std::process::Command;

    // Default to Run when no subcommand given (bare `remargin mcp`).
    match mcp_action {
        None | Some(McpAction::Run) => mcp::run(system, cwd, startup_flags, startup_assets_dir),
        Some(McpAction::Install { user }) => {
            let bin = env::current_exe().context("resolving remargin binary path")?;
            let bin_str = bin.display().to_string();
            let scope = if *user { "user" } else { "project" };

            // Remove first to make the operation idempotent.
            let _: Result<_, _> = Command::new("claude")
                .args(["mcp", "remove", "remargin"])
                .output()
                .map(drop);

            let output = Command::new("claude")
                .args(["mcp", "add", "remargin", "-s", scope, "--", &bin_str, "mcp"])
                .output()
                .context("running 'claude mcp add' -- is Claude Code CLI installed?")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("claude mcp add failed: {stderr}");
            }

            if json_mode {
                print_output(
                    sinks,
                    true,
                    &json!({
                        "installed": true,
                        "scope": scope,
                        "binary": bin_str,
                    }),
                )
            } else {
                writeln!(
                    sinks.stderr,
                    "MCP server registered ({scope} scope): {bin_str}"
                )
                .context("writing to stderr")?;
                Ok(())
            }
        }
        Some(McpAction::Uninstall) => {
            let output = Command::new("claude")
                .args(["mcp", "remove", "remargin"])
                .output()
                .context("running 'claude mcp remove' -- is Claude Code CLI installed?")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("claude mcp remove failed: {stderr}");
            }

            if json_mode {
                print_output(sinks, true, &json!({ "uninstalled": true }))
            } else {
                writeln!(sinks.stderr, "MCP server unregistered.").context("writing to stderr")?;
                Ok(())
            }
        }
        Some(McpAction::Test) => {
            let output = Command::new("claude")
                .args(["mcp", "list"])
                .output()
                .context("running 'claude mcp list' -- is Claude Code CLI installed?")?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let registered = stdout.lines().any(|l| l.contains("remargin"));
            let status_str = if registered {
                "registered"
            } else {
                "not_registered"
            };

            if json_mode {
                print_output(sinks, true, &json!({ "status": status_str }))
            } else {
                writeln!(sinks.stderr, "MCP status: {status_str}").context("writing to stderr")?;
                Ok(())
            }
        }
    }
}

fn recipients_display(status: &RecipientStatus) -> String {
    match status {
        RecipientStatus::Ok => "ok".to_owned(),
        RecipientStatus::Unknown(bad) => format!("unknown({})", bad.join(", ")),
        _ => status.as_str().to_owned(),
    }
}

pub fn cmd_verify(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    config: &ResolvedConfig,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let report = operations::verify::verify_and_refresh(system, &path, config)?;

    if json_mode {
        print_output(sinks, true, &report.to_json())?;
    } else {
        for row in &report.results {
            let chk = if row.checksum_ok { "ok" } else { "FAIL" };
            let recipients_str = recipients_display(&row.recipients);
            out(
                sinks,
                &format!(
                    "{}: author={} line={} checksum={} signature={} recipients={}",
                    row.id,
                    row.author,
                    row.line,
                    chk,
                    row.signature.as_str(),
                    recipients_str,
                ),
            )?;
        }
    }

    if report.ok {
        Ok(())
    } else {
        let failures: Vec<String> = report
            .results
            .iter()
            .filter(|r| {
                !r.checksum_ok
                    || r.signature.as_str() != "valid"
                    || matches!(r.recipients, RecipientStatus::Unknown(_))
            })
            .map(|r| {
                format!(
                    "{} (checksum={} signature={} recipients={})",
                    r.id,
                    if r.checksum_ok { "ok" } else { "FAIL" },
                    r.signature.as_str(),
                    recipients_display(&r.recipients),
                )
            })
            .collect();
        anyhow::bail!("integrity check failed: {}", failures.join(", "));
    }
}

pub fn cmd_write(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    wp: &WriteParams<'_>,
) -> Result<()> {
    let target_buf = expand_cli_path(system, wp.path)?;
    let target = target_buf.as_path();

    let body = match wp.content {
        Some(s) => String::from(s),
        None => read_stdin()?,
    };

    let outcome = document::write(system, cwd, target, &body, config, wp.opts)?;

    // A no-op prints a one-line human message in text mode instead of
    // the usual "written: ... / binary: ... / raw: ..." block; JSON mode
    // still returns a single payload, now with `noop: true` alongside
    // the existing fields so callers can branch on it.
    if outcome.noop && !wp.json_mode {
        return out(
            sinks,
            &format!("{}: no changes (already up to date)", wp.path),
        );
    }

    print_output(
        sinks,
        wp.json_mode,
        &outcome.to_json(wp.path, wp.opts.binary, wp.opts.raw),
    )
}
