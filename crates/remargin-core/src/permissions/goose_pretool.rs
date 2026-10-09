//! `remargin goose pretool` core — goose `PreToolUse` hook adapter.
//!
//! Reads goose's `PreToolUse` envelope, maps it onto the shared
//! [`ToolTarget`] seam, and renders goose's verdict from the outcome
//! [`decide`] returns. The decision engine is not duplicated here: this
//! module is envelope translation and nothing else, so the two hosts
//! cannot enforce different boundaries.
//!
//! Three differences from Claude Code's surface drive the translation:
//! goose namespaces tools (`developer__text_editor`, `developer__shell`),
//! names the edited file `tool_input.path` rather than
//! `tool_input.file_path`, and folds the editing verb into
//! `tool_input.command`.
//!
//! The namespacing runs both ways: a deny message points the agent at a
//! remargin op, so [`decide`] renders those op names with
//! [`ToolPrefix::GOOSE`] — a goose session has no `mcp__remargin__*` tool
//! to call, and an unfollowable redirect is the retry loop the guidance
//! exists to end.
//!
//! Fail-closed by construction. goose treats a hook that crashes, times
//! out, or prints nothing as permission to proceed, so every path that
//! cannot reach a confident allow — an unparseable envelope, a gated tool
//! missing the field naming its target, an internal resolve failure —
//! renders [`GooseVerdict::Block`]. A guard that fails open is not a
//! guard.
//!
//! Pure (no stdin / stdout / exit): the CLI handler is the only piece
//! that touches I/O, so unit tests run without spawning the binary.

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use os_shim::System;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::permissions::pretool::{PretoolOutcome, ToolPrefix, ToolTarget, decide};

/// goose's builtin file-editing tool; its `tool_input.command` carries the editing verb.
const TOOL_TEXT_EDITOR: &str = "developer__text_editor";

const TOOL_SHELL: &str = "developer__shell";

/// goose namespaces a tool as `<extension>__<tool>`; a host that re-prefixes MCP extensions
/// yields the second form.
const REMARGIN_TOOL_PREFIXES: &[&str] = &[ToolPrefix::GOOSE.as_str(), ToolPrefix::CLAUDE.as_str()];

/// Input keys naming a filesystem path on a tool outside the gated set.
const UNGATED_PATH_KEYS: &[&str] = &["path", "file_path"];

/// Block payload goose reads from stdout.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct BlockDecision {
    pub decision: &'static str,
    pub reason: String,
}

impl BlockDecision {
    #[must_use]
    pub fn new(reason: &str) -> Self {
        Self {
            decision: "block",
            reason: String::from(reason),
        }
    }
}

/// Outcome of [`goose_pretool`]. The caller renders it onto goose's two
/// protocol channels.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GooseVerdict {
    /// No managed path touched, or the tool is not gated: emit nothing and exit 0.
    Allow,
    /// Emit the block object on stdout and `reason` on stderr; exit 2.
    Block { reason: String },
}

/// goose's `PreToolUse` event envelope on stdin.
///
/// `working_dir` is optional in the type only so its absence surfaces as a
/// block rather than a parse failure with no diagnostic: a gated tool
/// without it cannot root a relative target.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct PreToolUseEvent {
    pub tool_input: Value,
    pub tool_name: String,
    pub working_dir: Option<PathBuf>,
}

/// Top-level entry point. Parses stdin, maps the tool onto a
/// [`ToolTarget`], and renders the engine's outcome as goose's verdict.
#[must_use]
pub fn goose_pretool(system: &dyn System, stdin_bytes: &[u8]) -> GooseVerdict {
    let event: PreToolUseEvent = match serde_json::from_slice(stdin_bytes) {
        Ok(value) => value,
        Err(err) => {
            return blocked(&format!(
                "remargin could not parse the goose PreToolUse event ({err}). The guard blocks \
                 what it cannot read."
            ));
        }
    };

    let target = match goose_target(&event) {
        Ok(Some(target)) => target,
        Ok(None) => return GooseVerdict::Allow,
        Err(reason) => return blocked(&reason),
    };

    let Some(cwd) = event.working_dir.as_deref() else {
        return blocked(&format!(
            "The goose PreToolUse event for `{}` carries no working_dir, so remargin cannot resolve \
             the directory this call would run in. Use the remargin MCP tools for managed content.",
            event.tool_name,
        ));
    };

    verdict_for(decide(system, &target, cwd, ToolPrefix::GOOSE))
}

fn blocked(reason: &str) -> GooseVerdict {
    GooseVerdict::Block {
        reason: String::from(reason),
    }
}

/// Render the engine's outcome as a goose verdict.
///
/// `Fail` becomes a block, not a pass-through: goose already fails open on
/// a hook that cannot answer, so the one case where remargin knows it
/// could not evaluate the call is exactly the case that must deny.
fn verdict_for(outcome: PretoolOutcome) -> GooseVerdict {
    match outcome {
        PretoolOutcome::SilentAllow => GooseVerdict::Allow,
        PretoolOutcome::Deny(decision) => GooseVerdict::Block {
            reason: decision.hook_specific_output.permission_decision_reason,
        },
        PretoolOutcome::Fail(reason) => blocked(&format!(
            "remargin could not evaluate this tool call ({reason}), so it is blocked rather than \
             waved through. Use the remargin MCP tools for managed content."
        )),
    }
}

/// Map the envelope onto the engine's target shape.
///
/// `Ok(None)` is a confident allow — remargin's own MCP surface, or a tool
/// naming no shape the engine gates. `Err` is the fail-closed path: the
/// tool is gated but the envelope does not say enough to resolve it.
fn goose_target(event: &PreToolUseEvent) -> Result<Option<ToolTarget>, String> {
    if is_remargin_tool(&event.tool_name) {
        return Ok(None);
    }
    match event.tool_name.as_str() {
        TOOL_TEXT_EDITOR => text_editor_target(event).map(Some),
        TOOL_SHELL => shell_target(event).map(Some),
        _ => Ok(ungated_target(event)),
    }
}

/// remargin's MCP tools are the sanctioned surface and are never
/// intercepted — gating them would leave the agent no way to reach managed
/// content at all.
fn is_remargin_tool(tool_name: &str) -> bool {
    REMARGIN_TOOL_PREFIXES
        .iter()
        .any(|prefix| tool_name.starts_with(prefix))
}

fn text_editor_target(event: &PreToolUseEvent) -> Result<ToolTarget, String> {
    let command = required_str(&event.tool_input, "command", &event.tool_name)?;
    let tool_name = text_editor_tool_name(command).ok_or_else(|| {
        format!(
            "remargin does not recognize `{command}` as a `{}` verb, so it cannot tell whether \
             this call would touch a remargin-managed path. Use the remargin MCP tools for \
             managed content.",
            event.tool_name,
        )
    })?;
    let path = required_str(&event.tool_input, "path", &event.tool_name)?;
    Ok(ToolTarget::Path {
        path: PathBuf::from(path),
        tool_name: String::from(tool_name),
    })
}

/// Claude-side tool class for a goose text-editor verb. The class selects
/// which remargin op the deny message names, so a mutating verb must not
/// map to the read class. An unlisted verb yields `None` and blocks.
fn text_editor_tool_name(command: &str) -> Option<&'static str> {
    match command {
        "view" => Some("Read"),
        "write" => Some("Write"),
        "insert" | "str_replace" | "undo_edit" => Some("Edit"),
        _ => None,
    }
}

fn shell_target(event: &PreToolUseEvent) -> Result<ToolTarget, String> {
    let command = required_str(&event.tool_input, "command", &event.tool_name)?;
    Ok(ToolTarget::BashCommand {
        command: String::from(command),
    })
}

/// A tool outside the gated set is waved past only when it names none of
/// the shapes the engine resolves. A `path` or a `command` on an unknown
/// extension is the same filesystem reach under another name, so it goes
/// through the engine; the tool name flows through to the deny message,
/// where an unrecognized name lands on the generic remargin-op guidance.
fn ungated_target(event: &PreToolUseEvent) -> Option<ToolTarget> {
    for key in UNGATED_PATH_KEYS {
        if let Some(path) = optional_str(&event.tool_input, key) {
            return Some(ToolTarget::Path {
                path: PathBuf::from(path),
                tool_name: event.tool_name.clone(),
            });
        }
    }
    optional_str(&event.tool_input, "command").map(|command| ToolTarget::BashCommand {
        command: String::from(command),
    })
}

fn optional_str<'input>(input: &'input Value, key: &str) -> Option<&'input str> {
    input.get(key).and_then(Value::as_str)
}

fn required_str<'input>(
    input: &'input Value,
    key: &str,
    tool: &str,
) -> Result<&'input str, String> {
    optional_str(input, key).ok_or_else(|| {
        format!(
            "The goose PreToolUse event for `{tool}` carries no tool_input.{key}, so remargin \
             cannot tell what this call would touch. Use the remargin MCP tools for managed \
             content."
        )
    })
}
