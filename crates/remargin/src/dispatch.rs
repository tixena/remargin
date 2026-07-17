//! Dispatch layer: entry point, error classification, identity wiring,
//! and the thin `handle_*` routers that unpack parsed `Commands` variants
//! and call the `handlers::cmd_*` orchestration functions.
//!
//! `main.rs` calls `dispatch::run` as its sole action after parsing the CLI.

use std::io::{Read as _, stdin as stdin_handle};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde_json::json;

use crate::handlers;
use crate::io::{
    IoSinks, expand_cli_path, expand_cli_pathbuf, inject_elapsed_ms, parse_line_range,
    resolve_comment_content,
};
use crate::params::{
    AckParams, ActivityOutputMode, ActivityParams, CommentParams, CpParams, EditParams,
    GetImageParams, GetOutputMode, GetParams, MvParams, PromptSetParams, ReactParams,
    ReplaceParams, RestrictParams, SearchParams, SignParams, WriteParams,
};
use crate::{
    AssetsArgs, ClaudeAction, Cli, Commands, IdentityArgs, OutputArgs, PermissionsAction,
    PlanAction, PlanClaudeAction, PluginAction, PretoolAction, PromptAction, SessionGuardAction,
    UnrestrictedArgs,
};
use remargin_core::config::identity::IdentityFlags;
use remargin_core::config::{self, ResolvedConfig};
use remargin_core::document;
use remargin_core::operations;
use remargin_core::operations::replace;
use remargin_core::permissions::pretool::{PretoolOutcome, pretool};
use remargin_core::permissions::session_guard::{GuardOutcome, session_guard};

pub const EXIT_ERROR: u8 = 1;
pub const EXIT_LINT: u8 = 2;
pub const EXIT_INTEGRITY: u8 = 3;
pub const EXIT_ATTACHMENT: u8 = 4;
pub const EXIT_PRESERVATION: u8 = 5;
pub const EXIT_SKILL: u8 = 6;
pub const EXIT_NOT_FOUND: u8 = 7;
pub const EXIT_AMBIGUOUS: u8 = 8;
/// Claude Code's `PreToolUse` hook contract maps exit 2 to "block the
/// tool call and feed stderr back to the model". Use the same value
/// for fail-closed pretool outcomes so the hook signal is intact.
pub const EXIT_PRETOOL_FAIL: u8 = 2;
/// Marker prefix in the error message so the top-level error mapper
/// can route pretool failures to exit code 2 (Claude Code's blocking
/// signal) without mistaking them for general CLI errors.
pub const PRETOOL_FAIL_SENTINEL: &str = "__remargin_pretool_fail__:";
/// Gitignore-style "no match" sentinel returned by
/// `permissions check` when the path is unrestricted.
/// Numerically equal to [`EXIT_ERROR`] so existing tooling that branches
/// on `1 vs 0` still works; the `main` harness recognises the sentinel
/// to skip the "error: ..." render that would otherwise prepend the
/// gitignore-style result.
pub const EXIT_NOT_RESTRICTED: u8 = 1;
/// Internal marker substring used by [`cmd_permissions`] to communicate
/// "not restricted" to [`classify_error`] without leaking through
/// stderr.
pub const PERMISSIONS_NOT_RESTRICTED_MARKER: &str = "__remargin_permissions_check_not_restricted__";

pub const fn subcommand_output(cmd: &Commands) -> Option<&OutputArgs> {
    match cmd {
        Commands::Ack { output_args, .. }
        | Commands::Activity { output_args, .. }
        | Commands::Batch { output_args, .. }
        | Commands::Comment { output_args, .. }
        | Commands::Comments { output_args, .. }
        | Commands::Delete { output_args, .. }
        | Commands::Doctor { output_args, .. }
        | Commands::Edit { output_args, .. }
        | Commands::Get { output_args, .. }
        | Commands::Identity { output_args, .. }
        | Commands::Keygen { output_args, .. }
        | Commands::Lint { output_args, .. }
        | Commands::Ls { output_args, .. }
        | Commands::Cp { output_args, .. }
        | Commands::Mcp { output_args, .. }
        | Commands::Metadata { output_args, .. }
        | Commands::Mv { output_args, .. }
        | Commands::Prompt { output_args, .. }
        | Commands::Purge { output_args, .. }
        | Commands::Query { output_args, .. }
        | Commands::React { output_args, .. }
        | Commands::Replace { output_args, .. }
        | Commands::Registry { output_args, .. }
        | Commands::ResolveMode { output_args, .. }
        | Commands::Rm { output_args, .. }
        | Commands::GetImage { output_args, .. }
        | Commands::Sandbox { output_args, .. }
        | Commands::Search { output_args, .. }
        | Commands::Sign { output_args, .. }
        | Commands::Verify { output_args, .. }
        | Commands::Write { output_args, .. } => Some(output_args),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { output_args, .. } => Some(output_args),
        Commands::Claude { action } => Some(claude_action_output(action)),
        Commands::Permissions { action } => Some(permissions_action_output(action)),
        Commands::Plan { action, .. } => Some(plan_action_output(action)),
        Commands::Version => None,
    }
}

/// Pull the per-action [`OutputArgs`] from a [`ClaudeAction`] variant.
const fn claude_action_output(action: &ClaudeAction) -> &OutputArgs {
    match action {
        ClaudeAction::Plugin { output_args, .. }
        | ClaudeAction::Pretool { output_args, .. }
        | ClaudeAction::SessionGuard { output_args, .. }
        | ClaudeAction::Restrict { output_args, .. }
        | ClaudeAction::Unrestrict { output_args, .. } => output_args,
    }
}

/// Pull the per-action [`OutputArgs`] from a [`PlanClaudeAction`] variant.
const fn plan_claude_action_output(action: &PlanClaudeAction) -> &OutputArgs {
    match action {
        PlanClaudeAction::Restrict { output_args, .. }
        | PlanClaudeAction::Unrestrict { output_args, .. } => output_args,
    }
}

/// Pull the per-action [`OutputArgs`] from a [`PermissionsAction`]
/// variant. Both `show` and `check` flatten an `OutputArgs`.
const fn permissions_action_output(action: &PermissionsAction) -> &OutputArgs {
    match action {
        PermissionsAction::Show { output_args } | PermissionsAction::Check { output_args, .. } => {
            output_args
        }
    }
}

/// Pull the per-action [`OutputArgs`] from a [`PlanAction`] variant.
/// Every plan sub-action flattens an `OutputArgs`.
const fn plan_action_output(action: &PlanAction) -> &OutputArgs {
    match action {
        PlanAction::Ack { output_args, .. }
        | PlanAction::Batch { output_args, .. }
        | PlanAction::Comment { output_args, .. }
        | PlanAction::Cp { output_args, .. }
        | PlanAction::Delete { output_args, .. }
        | PlanAction::Edit { output_args, .. }
        | PlanAction::Mv { output_args, .. }
        | PlanAction::Purge { output_args, .. }
        | PlanAction::React { output_args, .. }
        | PlanAction::SandboxAdd { output_args, .. }
        | PlanAction::SandboxRemove { output_args, .. }
        | PlanAction::Sign { output_args, .. }
        | PlanAction::Write { output_args, .. } => output_args,
        PlanAction::Claude { action: claude } => plan_claude_action_output(claude),
    }
}

/// Reject `--compact` on subcommands that do not emit the compact
/// columnar contract. `OutputArgs` is flattened everywhere, so a single
/// gate here keeps the flag from being silently ignored. `get`, `query`,
/// and `activity` wire compact today; the follow-up search task extends
/// the allow-set.
fn reject_unsupported_compact(cmd: &Commands) -> Result<()> {
    let compact = subcommand_output(cmd).is_some_and(|o| o.compact);
    if compact
        && !matches!(
            cmd,
            Commands::Activity { .. } | Commands::Get { .. } | Commands::Query { .. }
        )
    {
        bail!("--compact is not supported for this subcommand");
    }
    Ok(())
}

fn classify_error(err: &anyhow::Error) -> u8 {
    let msg = format!("{err:#}");
    if msg.contains(PERMISSIONS_NOT_RESTRICTED_MARKER) {
        EXIT_NOT_RESTRICTED
    } else if msg.contains(PRETOOL_FAIL_SENTINEL) {
        EXIT_PRETOOL_FAIL
    } else if msg.contains("Lint error") {
        EXIT_LINT
    } else if msg.contains("checksum") || msg.contains("signature") || msg.contains("integrity") {
        EXIT_INTEGRITY
    } else if msg.contains("attachment not found") {
        EXIT_ATTACHMENT
    } else if msg.contains("was removed") || msg.contains("preservation") {
        EXIT_PRESERVATION
    } else if msg.contains("skill") && msg.contains("not installed") {
        EXIT_SKILL
    } else if msg.contains("ambiguous: comment") {
        EXIT_AMBIGUOUS
    } else if msg.contains("not found") {
        EXIT_NOT_FOUND
    } else {
        EXIT_ERROR
    }
}

/// Build an [`IdentityFlags`] plus an optional `--assets-dir` value from
/// per-subcommand arg groups. The adapter boundary is where `~` /
/// `$VAR` get expanded, so the core never sees unexpanded path sigils.
///
/// The returned flags are consumed by
/// [`config::ResolvedConfig::resolve`], which picks the appropriate
/// branch of [`config::identity::resolve_identity`] — a single whole
/// identity comes out, never a mixture of fields from different files.
pub fn build_identity_flags(
    system: &dyn System,
    identity_args: &IdentityArgs,
    assets_args: Option<&AssetsArgs>,
) -> Result<(IdentityFlags, Option<String>)> {
    let assets_dir = match assets_args.and_then(|a| a.assets_dir()) {
        Some(raw) => Some(expand_cli_path(system, raw)?.to_string_lossy().into_owned()),
        None => None,
    };

    let config_path = match identity_args.config() {
        Some(raw) => Some(expand_cli_path(system, &raw.to_string_lossy())?),
        None => None,
    };

    let key = match identity_args.key() {
        Some(raw) => {
            // `--key` accepts a bare name shorthand (e.g. `mykey` →
            // `~/.ssh/mykey`). Expand only when the raw value contains
            // a path sigil — bare names are resolved later by
            // `resolve_key_path`.
            if raw.starts_with('~') || raw.contains('$') {
                Some(expand_cli_path(system, raw)?.to_string_lossy().into_owned())
            } else {
                Some(String::from(raw))
            }
        }
        None => None,
    };

    let author_type = match identity_args.author_type() {
        Some(raw) => Some(config::parse_author_type(raw)?),
        None => None,
    };

    let mut flags = IdentityFlags::default();
    flags.author_type = author_type;
    flags.config_path = config_path;
    flags.identity = identity_args.identity().map(String::from);
    flags.key = key;

    Ok((flags, assets_dir))
}

/// A handful of subcommands run entirely without a [`ResolvedConfig`]
/// (`Version`, `Identity`, `ResolveMode`, `Keygen`, `Skill`, `Obsidian`).
/// `Identity` is a read-only diagnostic — it calls
/// [`config::ResolvedConfig::resolve`] inside its own handler so a
/// branch-3 walk miss surfaces as `{ "found": false }` instead of
/// bailing the whole process. Returning `true` here
/// short-circuits the config load in [`run`].
const fn subcommand_is_config_free(cmd: &Commands) -> bool {
    match cmd {
        Commands::Version
        | Commands::Activity { .. }
        | Commands::Claude { .. }
        | Commands::Doctor { .. }
        | Commands::Identity { .. }
        | Commands::Permissions { .. }
        | Commands::ResolveMode { .. }
        | Commands::Keygen { .. } => true,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => true,
        Commands::Ack { .. }
        | Commands::Batch { .. }
        | Commands::Comment { .. }
        | Commands::Comments { .. }
        | Commands::Cp { .. }
        | Commands::Delete { .. }
        | Commands::Edit { .. }
        | Commands::Get { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Mcp { .. }
        | Commands::Metadata { .. }
        | Commands::Mv { .. }
        | Commands::Plan { .. }
        | Commands::Prompt { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Replace { .. }
        | Commands::Registry { .. }
        | Commands::Rm { .. }
        | Commands::GetImage { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Verify { .. }
        | Commands::Write { .. } => false,
    }
}

/// Fetch the [`IdentityArgs`] flatten for subcommands that declare one.
///
/// Subcommands that do not resolve identity (lint, query, search, ls,
/// get, metadata, registry, comments, version, keygen, resolve-mode,
/// skill, obsidian) return `None`; callers use the
/// [`IdentityArgs::default`] to build an empty [`IdentityFlags`].
const fn subcommand_identity(cmd: &Commands) -> Option<&IdentityArgs> {
    match cmd {
        Commands::Ack { identity_args, .. }
        | Commands::Activity { identity_args, .. }
        | Commands::Batch { identity_args, .. }
        | Commands::Comment { identity_args, .. }
        | Commands::Cp { identity_args, .. }
        | Commands::Delete { identity_args, .. }
        | Commands::Edit { identity_args, .. }
        | Commands::Identity { identity_args, .. }
        | Commands::Mcp { identity_args, .. }
        | Commands::Mv { identity_args, .. }
        | Commands::Plan { identity_args, .. }
        | Commands::Prompt { identity_args, .. }
        | Commands::Purge { identity_args, .. }
        | Commands::Query { identity_args, .. }
        | Commands::React { identity_args, .. }
        | Commands::Replace { identity_args, .. }
        | Commands::Rm { identity_args, .. }
        | Commands::Sandbox { identity_args, .. }
        | Commands::Sign { identity_args, .. }
        | Commands::Verify { identity_args, .. }
        | Commands::Write { identity_args, .. } => Some(identity_args),
        Commands::Claude { .. }
        | Commands::Comments { .. }
        | Commands::Doctor { .. }
        | Commands::Get { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Metadata { .. }
        | Commands::Permissions { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::GetImage { .. }
        | Commands::Search { .. }
        | Commands::Version => None,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => None,
    }
}

/// Fetch the [`AssetsArgs`] flatten for subcommands that write
/// attachments.
const fn subcommand_assets(cmd: &Commands) -> Option<&AssetsArgs> {
    match cmd {
        Commands::Batch { assets_args, .. }
        | Commands::Comment { assets_args, .. }
        | Commands::Edit { assets_args, .. } => Some(assets_args),
        Commands::Ack { .. }
        | Commands::Activity { .. }
        | Commands::Claude { .. }
        | Commands::Comments { .. }
        | Commands::Cp { .. }
        | Commands::Delete { .. }
        | Commands::Doctor { .. }
        | Commands::Get { .. }
        | Commands::Identity { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Mcp { .. }
        | Commands::Metadata { .. }
        | Commands::Mv { .. }
        | Commands::Permissions { .. }
        | Commands::Plan { .. }
        | Commands::Prompt { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Replace { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Rm { .. }
        | Commands::GetImage { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Verify { .. }
        | Commands::Version
        | Commands::Write { .. } => None,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => None,
    }
}

/// Fetch the [`UnrestrictedArgs`] flatten for subcommands that touch
/// arbitrary filesystem paths.
const fn subcommand_unrestricted(cmd: &Commands) -> Option<&UnrestrictedArgs> {
    match cmd {
        Commands::Cp {
            unrestricted_args, ..
        }
        | Commands::Get {
            unrestricted_args, ..
        }
        | Commands::Ls {
            unrestricted_args, ..
        }
        | Commands::Metadata {
            unrestricted_args, ..
        }
        | Commands::Rm {
            unrestricted_args, ..
        }
        | Commands::GetImage {
            unrestricted_args, ..
        }
        | Commands::Replace {
            unrestricted_args, ..
        }
        | Commands::Write {
            unrestricted_args, ..
        } => Some(unrestricted_args),
        Commands::Ack { .. }
        | Commands::Activity { .. }
        | Commands::Batch { .. }
        | Commands::Claude { .. }
        | Commands::Comment { .. }
        | Commands::Comments { .. }
        | Commands::Delete { .. }
        | Commands::Doctor { .. }
        | Commands::Edit { .. }
        | Commands::Identity { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Mcp { .. }
        | Commands::Mv { .. }
        | Commands::Permissions { .. }
        | Commands::Plan { .. }
        | Commands::Prompt { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Verify { .. }
        | Commands::Version => None,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => None,
    }
}

pub fn run(cli: &Cli, system: &dyn System, cwd: &Path, sinks: &mut IoSinks<'_>) -> ExitCode {
    let json_mode = subcommand_output(cli.cmd()).is_some_and(|o| o.json);

    match dispatch(cli, system, cwd, sinks) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let err_msg = format!("{err:#}");
            let is_silent_sentinel = err_msg.contains(PERMISSIONS_NOT_RESTRICTED_MARKER);
            let exit_code = classify_error(&err);
            let verify_failure = err.downcast_ref::<operations::verify::VerifyFailure>();
            let subset_failure = err.downcast_ref::<operations::verify::SubsetGateFailure>();
            if is_silent_sentinel {
                // Sentinel for `permissions check`.
                // Output already emitted on the success path; we only
                // need the gitignore-style exit code, no "error: ..."
                // render.
            } else if let Some(reason) = err_msg.strip_prefix(PRETOOL_FAIL_SENTINEL) {
                // Pretool fail-closed: Claude Code reads stderr and
                // feeds it back to the model. No "error: " prefix —
                // just the bare reason.
                let _ = writeln!(sinks.stderr, "{reason}");
            } else if json_mode {
                let payload = subset_failure
                    .map(operations::verify::SubsetGateFailure::to_json)
                    .or_else(|| verify_failure.map(operations::verify::VerifyFailure::to_json))
                    .unwrap_or_else(|| json!({ "error": err_msg }));
                let error_json = inject_elapsed_ms(&payload);
                let _ = writeln!(
                    sinks.stderr,
                    "{}",
                    serde_json::to_string_pretty(&error_json).unwrap_or_default()
                );
            } else if let Some(sg) = subset_failure {
                let _ = writeln!(sinks.stderr, "error: {}\n\n{}", sg.headline(), sg.hint());
            } else if let Some(vf) = verify_failure {
                let _ = writeln!(sinks.stderr, "error: {}", vf.human_text());
            } else {
                let _ = writeln!(sinks.stderr, "error: {err_msg}");
            }
            ExitCode::from(exit_code)
        }
    }
}

fn dispatch(cli: &Cli, system: &dyn System, cwd: &Path, sinks: &mut IoSinks<'_>) -> Result<()> {
    reject_unsupported_compact(cli.cmd())?;

    let output = subcommand_output(cli.cmd());
    let json_mode = output.is_some_and(|o| o.json);

    if try_dispatch_config_free(cli, system, cwd, sinks)?.is_some() {
        return Ok(());
    }

    let default_identity = IdentityArgs::default();
    // Feature-gated: with `unrestricted`, this is a derived `Default` on a
    // regular struct; without it, a unit struct. Both spell as `UnrestrictedArgs::default()`
    // but clippy flags the unit-struct case as `default_constructed_unit_structs`.
    #[cfg(feature = "unrestricted")]
    let default_unrestricted = UnrestrictedArgs::default();
    #[cfg(not(feature = "unrestricted"))]
    let default_unrestricted = UnrestrictedArgs;
    let identity_args = subcommand_identity(cli.cmd()).unwrap_or(&default_identity);
    let assets_args = subcommand_assets(cli.cmd());
    let unrestricted_args = subcommand_unrestricted(cli.cmd()).unwrap_or(&default_unrestricted);

    let (flags, assets_dir) = build_identity_flags(system, identity_args, assets_args)?;

    // The Mcp subcommand forwards its flags directly to `mcp::run` so
    // per-tool identity fields can still be declared on each request.
    // Branch out early.
    if let Commands::Mcp { action, .. } = cli.cmd() {
        return handlers::cmd_mcp(
            sinks,
            system,
            cwd,
            &flags,
            assets_dir.as_deref(),
            action.as_ref(),
            json_mode,
        );
    }

    let mut final_config = ResolvedConfig::resolve(system, cwd, &flags, assets_dir.as_deref())?;
    final_config.unrestricted = unrestricted_args.unrestricted();

    dispatch_with_config(sinks, cli, system, cwd, &final_config)
}

/// Handle every config-free subcommand. Returns `Ok(Some(()))` when a
/// matching arm ran, `Ok(None)` when the subcommand needs the
/// config-aware dispatch path.
fn try_dispatch_config_free(
    cli: &Cli,
    system: &dyn System,
    cwd: &Path,
    sinks: &mut IoSinks<'_>,
) -> Result<Option<()>> {
    match cli.cmd() {
        Commands::Version => handle_version(sinks).map(Some),
        Commands::Identity { .. } => handle_identity(cli.cmd(), sinks, system, cwd).map(Some),
        Commands::ResolveMode { .. } => {
            handle_resolve_mode(cli.cmd(), sinks, system, cwd).map(Some)
        }
        Commands::Keygen { .. } => handle_keygen(cli.cmd(), sinks, system).map(Some),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => handle_obsidian(cli.cmd(), sinks, system, cwd).map(Some),
        Commands::Activity { .. } => handle_activity(cli.cmd(), sinks, system, cwd).map(Some),
        Commands::Doctor { .. } => handlers::cmd_doctor(sinks, system, cwd, cli.cmd()).map(Some),
        Commands::Permissions { action } => {
            handlers::cmd_permissions(sinks, system, cwd, action).map(Some)
        }
        Commands::Claude { action } => handle_claude(action, sinks, system, cwd).map(Some),
        _ => {
            debug_assert!(
                !subcommand_is_config_free(cli.cmd()),
                "config-free subcommand fell through short-circuit"
            );
            Ok(None)
        }
    }
}

fn handle_version(sinks: &mut IoSinks<'_>) -> Result<()> {
    writeln!(sinks.stderr, "remargin {}", env!("CARGO_PKG_VERSION")).context("writing to stderr")
}

fn handle_identity(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Identity {
        action,
        identity_args,
        output_args,
    } = command
    else {
        bail!("internal: handle_identity called with wrong subcommand");
    };
    handlers::cmd_identity(
        sinks,
        system,
        cwd,
        action.as_ref(),
        identity_args,
        output_args.json,
    )
}

fn handle_prompt(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Prompt {
        action,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_prompt called with wrong subcommand");
    };
    let cmd_json = output_args.json;
    match action {
        PromptAction::Resolve {
            file,
            output_args: a,
        } => handlers::cmd_prompt_resolve(sinks, system, cwd, file, cmd_json || a.json),
        PromptAction::Set {
            folder,
            name,
            prompt,
            output_args: a,
        } => handlers::cmd_prompt_set(
            sinks,
            system,
            &PromptSetParams {
                config,
                cwd,
                folder,
                json_mode: cmd_json || a.json,
                name,
                prompt_flag: prompt.as_deref(),
            },
        ),
        PromptAction::Delete {
            folder,
            output_args: a,
        } => handlers::cmd_prompt_delete(sinks, system, cwd, config, folder, cmd_json || a.json),
        PromptAction::List {
            folder,
            output_args: a,
        } => handlers::cmd_prompt_list(sinks, system, cwd, folder, cmd_json || a.json),
    }
}

fn handle_resolve_mode(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::ResolveMode {
        cwd: cwd_arg,
        output_args,
    } = command
    else {
        bail!("internal: handle_resolve_mode called with wrong subcommand");
    };
    let cwd_expanded = cwd_arg
        .as_deref()
        .map(|c| expand_cli_pathbuf(system, c))
        .transpose()?;
    let start_dir = cwd_expanded.as_deref().unwrap_or(cwd);
    handlers::cmd_resolve_mode(sinks, system, start_dir, output_args.json)
}

fn handle_keygen(command: &Commands, sinks: &mut IoSinks<'_>, system: &dyn System) -> Result<()> {
    let Commands::Keygen {
        output: keygen_output,
        ..
    } = command
    else {
        bail!("internal: handle_keygen called with wrong subcommand");
    };
    let expanded_output = expand_cli_pathbuf(system, keygen_output)?;
    handlers::cmd_keygen(sinks, system, &expanded_output)
}

#[cfg(feature = "obsidian")]
fn handle_obsidian(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Obsidian {
        action,
        output_args,
    } = command
    else {
        bail!("internal: handle_obsidian called with wrong subcommand");
    };
    handlers::cmd_obsidian(sinks, system, cwd, action, output_args.json)
}

fn handle_plugin(
    sinks: &mut IoSinks<'_>,
    action: &PluginAction,
    output_args: &OutputArgs,
) -> Result<()> {
    handlers::cmd_plugin(sinks, action, output_args.json)
}

fn handle_activity(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Activity {
        path,
        since,
        pretty,
        identity_args,
        output_args,
    } = command
    else {
        bail!("internal: handle_activity called with wrong subcommand");
    };
    // --pretty and --json (hence --compact) are mutually exclusive.
    if *pretty && output_args.json {
        bail!("--pretty and --json are mutually exclusive");
    }
    // clap enforces `--compact` requires `--json`, so compact implies json.
    let output = if output_args.compact {
        ActivityOutputMode::Compact
    } else if *pretty {
        ActivityOutputMode::Pretty
    } else {
        ActivityOutputMode::Json
    };
    let p = ActivityParams {
        explicit_path: path.as_deref(),
        identity_args,
        output,
        since: since.as_deref(),
    };
    handlers::cmd_activity(sinks, system, cwd, &p)
}

fn handle_claude(
    action: &ClaudeAction,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    match action {
        ClaudeAction::Plugin {
            action: plugin_action,
            output_args,
        } => handle_plugin(sinks, plugin_action, output_args),
        ClaudeAction::Pretool {
            action: pretool_action,
            output_args,
        } => handle_claude_pretool_action(sinks, system, pretool_action.as_ref(), output_args.json),
        ClaudeAction::SessionGuard {
            action: guard_action,
            output_args,
        } => handle_claude_session_guard_action(
            sinks,
            system,
            cwd,
            guard_action.as_ref(),
            output_args.json,
        ),
        ClaudeAction::Restrict {
            path,
            also_deny_bash,
            cli_allowed,
            user_settings,
            output_args,
        } => {
            let p = RestrictParams {
                also_deny_bash,
                cli_allowed: *cli_allowed,
                json_mode: output_args.json,
                path,
                user_settings_explicit: user_settings.as_deref(),
            };
            handlers::cmd_restrict(sinks, system, cwd, &p)
        }
        ClaudeAction::Unrestrict {
            path,
            strict,
            user_settings,
            output_args,
        } => handlers::cmd_unprotect(
            sinks,
            system,
            cwd,
            path,
            *strict,
            user_settings.as_deref(),
            output_args.json,
        ),
    }
}

/// Route `remargin claude pretool [subcommand]`. With no subcommand
/// (or `dispatch`), runs the stdin/stdout hook dispatcher. The
/// install / uninstall / test variants manage the hook entry in a
/// Claude settings file.
fn handle_claude_pretool_action(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    action: Option<&PretoolAction>,
    json_mode: bool,
) -> Result<()> {
    match action {
        None | Some(PretoolAction::Dispatch) => handle_claude_pretool_dispatch(sinks, system),
        Some(PretoolAction::Install { local }) => {
            handlers::cmd_pretool_install(sinks, system, *local, json_mode)
        }
        Some(PretoolAction::Uninstall { local }) => {
            handlers::cmd_pretool_uninstall(sinks, system, *local, json_mode)
        }
        Some(PretoolAction::Test { local }) => {
            handlers::cmd_pretool_test(sinks, system, *local, json_mode)
        }
    }
}

/// Reads the `PreToolUse` event JSON from stdin, runs the core
/// [`remargin_core::permissions::pretool::pretool`] function, and
/// emits the outcome. Fail-closed: any failure exits via
/// [`anyhow::bail!`] so the surrounding runner returns a non-zero
/// status (mapped to Claude Code's blocking semantics).
fn handle_claude_pretool_dispatch(sinks: &mut IoSinks<'_>, system: &dyn System) -> Result<()> {
    let mut buf = Vec::new();
    stdin_handle()
        .read_to_end(&mut buf)
        .context("reading stdin for claude pretool")?;
    match pretool(system, &buf) {
        PretoolOutcome::SilentAllow => Ok(()),
        PretoolOutcome::Deny(decision) => {
            let json = serde_json::to_string(&decision).context("serializing pretool decision")?;
            writeln!(sinks.stdout, "{json}").context("writing claude pretool decision")
        }
        PretoolOutcome::Fail(reason) => Err(anyhow::anyhow!("{PRETOOL_FAIL_SENTINEL}{reason}")),
        _ => Err(anyhow::anyhow!(
            "{PRETOOL_FAIL_SENTINEL}unexpected pretool outcome",
        )),
    }
}

/// Route `remargin claude session-guard [subcommand]`. With no subcommand
/// (or `dispatch`), runs the `SessionStart` guard. The install /
/// uninstall / test variants manage the hook entry in a Claude settings
/// file.
fn handle_claude_session_guard_action(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: Option<&SessionGuardAction>,
    json_mode: bool,
) -> Result<()> {
    match action {
        None | Some(SessionGuardAction::Dispatch) => {
            handle_claude_session_guard_dispatch(sinks, system, cwd)
        }
        Some(SessionGuardAction::Install { local }) => {
            handlers::cmd_session_guard_install(sinks, system, *local, json_mode)
        }
        Some(SessionGuardAction::Uninstall { local }) => {
            handlers::cmd_session_guard_uninstall(sinks, system, *local, json_mode)
        }
        Some(SessionGuardAction::Test { local }) => {
            handlers::cmd_session_guard_test(sinks, system, *local, json_mode)
        }
    }
}

/// Runs the `SessionStart` guard core and emits the outcome. Always exits
/// 0: `SessionStart` has no blocking or decision control, and JSON is
/// honored only on exit 0, so the diagnostic JSON on stdout is the
/// strongest available signal (`additionalContext` into Claude's context,
/// `systemMessage` to the user). A clean session emits nothing.
fn handle_claude_session_guard_dispatch(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    match session_guard(system, cwd) {
        GuardOutcome::Fail(diagnostic) => {
            let json = serde_json::to_string(&diagnostic)
                .context("serializing session guard diagnostic")?;
            writeln!(sinks.stdout, "{json}").context("writing session guard diagnostic")
        }
        GuardOutcome::Ok => Ok(()),
        _ => Err(anyhow::anyhow!("unexpected session guard outcome")),
    }
}

fn dispatch_with_config(
    sinks: &mut IoSinks<'_>,
    cli: &Cli,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    match cli.cmd() {
        Commands::Ack { .. } => handle_ack(cli.cmd(), sinks, system, cwd, config),
        Commands::Batch { .. } => handle_batch(cli.cmd(), sinks, system, cwd, config),
        Commands::Comment { .. } => handle_comment(cli.cmd(), sinks, system, cwd, config),
        Commands::Comments { .. } => handle_comments(cli.cmd(), sinks, system, cwd),
        Commands::Cp { .. } => handle_cp(cli.cmd(), sinks, system, cwd, config),
        Commands::Delete { .. } => handle_delete(cli.cmd(), sinks, system, cwd, config),
        Commands::Edit { .. } => handle_edit(cli.cmd(), sinks, system, cwd, config),
        Commands::Get { .. } => handle_get(cli.cmd(), sinks, system, cwd, config),
        Commands::Lint { .. } => handle_lint(cli.cmd(), sinks, system, cwd),
        Commands::Ls { .. } => handle_ls(cli.cmd(), sinks, system, cwd, config),
        Commands::Metadata { .. } => handle_metadata(cli.cmd(), sinks, system, cwd, config),
        Commands::Mv { .. } => handle_mv(cli.cmd(), sinks, system, cwd, config),
        Commands::Plan { .. } => handle_plan(cli.cmd(), sinks, system, cwd, config),
        Commands::Prompt { .. } => handle_prompt(cli.cmd(), sinks, system, cwd, config),
        Commands::Purge { .. } => handle_purge(cli.cmd(), sinks, system, cwd, config),
        Commands::Query { .. } => handle_query(cli.cmd(), sinks, system, cwd, config),
        Commands::React { .. } => handle_react(cli.cmd(), sinks, system, cwd, config),
        Commands::Replace { .. } => handle_replace(cli.cmd(), sinks, system, cwd, config),
        Commands::Registry { .. } => handle_registry(cli.cmd(), sinks, system, cwd),
        Commands::Rm { .. } => handle_rm(cli.cmd(), sinks, system, cwd, config),
        Commands::GetImage { .. } => handle_get_image(cli.cmd(), sinks, system, cwd, config),
        Commands::Sandbox { .. } => handle_sandbox(cli.cmd(), sinks, system, cwd, config),
        Commands::Search { .. } => handle_search(cli.cmd(), sinks, system, cwd),
        Commands::Sign { .. } => handle_sign(cli.cmd(), sinks, system, cwd, config),
        Commands::Verify { .. } => handle_verify(cli.cmd(), sinks, system, cwd, config),
        Commands::Write { .. } => handle_write(cli.cmd(), sinks, system, cwd, config),
        Commands::Version
        | Commands::Activity { .. }
        | Commands::Claude { .. }
        | Commands::Doctor { .. }
        | Commands::Identity { .. }
        | Commands::Mcp { .. }
        | Commands::Keygen { .. }
        | Commands::Permissions { .. }
        | Commands::ResolveMode { .. } => Ok(()),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => Ok(()),
    }
}

fn handle_ack(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Ack {
        file,
        ids,
        path,
        remove,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_ack called with wrong subcommand");
    };
    let ap = AckParams {
        file: file.as_deref(),
        ids,
        json_mode: output_args.json,
        remove: *remove,
        search_path: path,
    };
    handlers::cmd_ack(sinks, system, cwd, config, &ap)
}

fn handle_batch(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Batch {
        file,
        ops,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_batch called with wrong subcommand");
    };
    handlers::cmd_batch(sinks, system, cwd, config, file, ops, output_args.json)
}

fn handle_comment(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Comment {
        file,
        content,
        after_comment,
        after_heading,
        after_line,
        attach,
        auto_ack,
        no_auto_ack,
        comment_file,
        remargin_kind,
        reply_to,
        sandbox,
        to,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_comment called with wrong subcommand");
    };
    let resolved_content =
        resolve_comment_content(system, cwd, content.as_ref(), comment_file.as_ref())?;
    let cp = CommentParams {
        after_comment: after_comment.as_deref(),
        after_heading: after_heading.as_deref(),
        after_line: *after_line,
        attachments: attach,
        auto_ack: tri_state_flag(*auto_ack, *no_auto_ack),
        content: &resolved_content,
        file,
        json_mode: output_args.json,
        remargin_kind,
        reply_to: reply_to.as_deref(),
        sandbox: *sandbox,
        to,
    };
    handlers::cmd_comment(sinks, system, cwd, config, &cp)
}

/// Map paired clap booleans to `Option<bool>`: `--flag` → Some(true),
/// `--no-flag` → Some(false), neither → None. The `conflicts_with` clap
/// attributes guarantee only one can be true at a time.
pub const fn tri_state_flag(yes: bool, no: bool) -> Option<bool> {
    if yes {
        Some(true)
    } else if no {
        Some(false)
    } else {
        None
    }
}

fn handle_comments(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Comments {
        file,
        pretty,
        remargin_kind,
        output_args,
    } = command
    else {
        bail!("internal: handle_comments called with wrong subcommand");
    };
    handlers::cmd_comments(
        sinks,
        system,
        cwd,
        file,
        remargin_kind,
        output_args.json,
        *pretty,
    )
}

fn handle_delete(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Delete {
        file,
        ids,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_delete called with wrong subcommand");
    };
    handlers::cmd_delete(sinks, system, cwd, config, file, ids, output_args.json)
}

fn handle_edit(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Edit {
        file,
        id,
        content,
        remargin_kind,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_edit called with wrong subcommand");
    };
    // When no --kind flags are provided we preserve the stored list; any
    // occurrence (even `--kind x` once) replaces the full list — consistent
    // with how `--to` works.
    let kind_replacement = (!remargin_kind.is_empty()).then_some(remargin_kind.as_slice());
    let p = EditParams {
        content,
        file,
        id,
        json_mode: output_args.json,
        remargin_kind: kind_replacement,
    };
    handlers::cmd_edit(sinks, system, cwd, config, &p)
}

fn handle_get(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Get {
        path,
        binary,
        start,
        end,
        line_numbers,
        out,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_get called with wrong subcommand");
    };
    // clap enforces `--compact` requires `--json`, so compact implies json.
    let output = if output_args.compact {
        GetOutputMode::Compact
    } else if output_args.json {
        GetOutputMode::Json
    } else {
        GetOutputMode::Text
    };
    let gp = GetParams {
        binary: *binary,
        end: *end,
        line_numbers: *line_numbers,
        out: out.as_deref(),
        output,
        path,
        start: *start,
    };
    handlers::cmd_get(sinks, system, cwd, config, &gp)
}

fn handle_lint(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Lint { file, output_args } = command else {
        bail!("internal: handle_lint called with wrong subcommand");
    };
    handlers::cmd_lint(sinks, system, cwd, file, output_args.json)
}

fn handle_ls(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Ls {
        path, output_args, ..
    } = command
    else {
        bail!("internal: handle_ls called with wrong subcommand");
    };
    handlers::cmd_ls(sinks, system, cwd, config, path, output_args.json)
}

fn handle_metadata(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Metadata {
        path, output_args, ..
    } = command
    else {
        bail!("internal: handle_metadata called with wrong subcommand");
    };
    handlers::cmd_metadata(sinks, system, cwd, config, path, output_args.json)
}

fn handle_cp(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Cp {
        src,
        dst,
        force,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_cp called with wrong subcommand");
    };
    let p = CpParams {
        dst: dst.as_str(),
        force: *force,
        json_mode: output_args.json,
        src: src.as_str(),
    };
    handlers::cmd_cp(sinks, system, cwd, config, &p)
}

fn handle_mv(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Mv {
        src,
        dst,
        force,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_mv called with wrong subcommand");
    };
    let p = MvParams {
        dst: dst.as_str(),
        force: *force,
        json_mode: output_args.json,
        src: src.as_str(),
    };
    handlers::cmd_mv(sinks, system, cwd, config, &p)
}

fn handle_plan(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Plan { action, .. } = command else {
        bail!("internal: handle_plan called with wrong subcommand");
    };
    handlers::cmd_plan(
        sinks,
        system,
        cwd,
        config,
        action,
        plan_action_output(action).json,
    )
}

fn handle_purge(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Purge {
        file,
        output_args,
        recursive,
        ..
    } = command
    else {
        bail!("internal: handle_purge called with wrong subcommand");
    };
    handlers::cmd_purge(
        sinks,
        system,
        cwd,
        config,
        file,
        *recursive,
        output_args.json,
    )
}

fn handle_query(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let q = handlers::build_query_params(command)?;
    handlers::cmd_query(sinks, system, cwd, config, &q)
}

fn handle_react(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::React {
        file,
        id,
        emoji,
        remove,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_react called with wrong subcommand");
    };
    let r = ReactParams {
        emoji: emoji.as_str(),
        file: file.as_str(),
        id: id.as_str(),
        json_mode: output_args.json,
        remove: *remove,
    };
    handlers::cmd_react(sinks, system, cwd, config, &r)
}

fn handle_replace(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Replace {
        pattern,
        replacement,
        path,
        regex,
        ignore_case,
        dry_run,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_replace called with wrong subcommand");
    };
    let options = replace::ReplaceOptions::new(
        String::from(pattern.as_str()),
        String::from(replacement.as_str()),
    )
    .regex(*regex)
    .ignore_case(*ignore_case)
    .dry_run(*dry_run);
    let r = ReplaceParams {
        json_mode: output_args.json,
        options,
        path: path.as_str(),
    };
    handlers::cmd_replace(sinks, system, cwd, config, &r)
}

fn handle_registry(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Registry {
        action,
        output_args,
    } = command
    else {
        bail!("internal: handle_registry called with wrong subcommand");
    };
    handlers::cmd_registry(sinks, system, cwd, action, output_args.json)
}

fn handle_rm(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Rm {
        file, output_args, ..
    } = command
    else {
        bail!("internal: handle_rm called with wrong subcommand");
    };
    handlers::cmd_rm(sinks, system, cwd, config, file, output_args.json)
}

fn handle_sandbox(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Sandbox {
        action,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_sandbox called with wrong subcommand");
    };
    handlers::cmd_sandbox(sinks, system, cwd, config, action, output_args.json)
}

fn handle_get_image(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::GetImage {
        path,
        crop,
        format,
        max_bytes,
        max_dimension,
        out,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_get_image called with wrong subcommand");
    };
    let sp = GetImageParams {
        crop: crop.as_deref(),
        format: format.as_deref(),
        json_mode: output_args.json,
        max_bytes: *max_bytes,
        max_dimension: *max_dimension,
        out: out.as_deref(),
        path,
    };
    handlers::cmd_get_image(sinks, system, cwd, config, &sp)
}

fn handle_search(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Search {
        pattern,
        path,
        regex,
        scope,
        context,
        ignore_case,
        limit,
        offset,
        output_args,
    } = command
    else {
        bail!("internal: handle_search called with wrong subcommand");
    };
    let s = SearchParams {
        context: *context,
        ignore_case: *ignore_case,
        json_mode: output_args.json,
        limit: *limit,
        offset: *offset,
        path: path.as_str(),
        pattern: pattern.as_str(),
        regex: *regex,
        scope: scope.as_str(),
    };
    handlers::cmd_search(sinks, system, cwd, &s)
}

fn handle_sign(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Sign {
        file,
        ids,
        all_mine,
        repair_checksum,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_sign called with wrong subcommand");
    };
    let sp = SignParams {
        all_mine: *all_mine,
        file,
        ids,
        json_mode: output_args.json,
        repair_checksum: *repair_checksum,
    };
    handlers::cmd_sign(sinks, system, cwd, config, &sp)
}

fn handle_verify(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Verify {
        file, output_args, ..
    } = command
    else {
        bail!("internal: handle_verify called with wrong subcommand");
    };
    handlers::cmd_verify(sinks, system, cwd, file, config, output_args.json)
}

fn handle_write(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Write {
        path,
        content,
        binary,
        create,
        lines,
        raw,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_write called with wrong subcommand");
    };
    let line_range = lines.as_deref().map(parse_line_range).transpose()?;
    handlers::cmd_write(
        sinks,
        system,
        cwd,
        config,
        &WriteParams {
            content: content.as_deref(),
            json_mode: output_args.json,
            opts: document::WriteOptions::new()
                .binary(*binary)
                .create(*create)
                .lines(line_range)
                .raw(*raw),
            path,
        },
    )
}
