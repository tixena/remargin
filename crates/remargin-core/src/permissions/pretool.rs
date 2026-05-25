//! `remargin claude pretool` core — Claude Code `PreToolUse` hook
//! dispatcher.
//!
//! Reads a `PreToolUse` event JSON envelope, extracts the path(s) the
//! tool is about to touch, and emits Claude Code's `PreToolUse`
//! decision JSON. Silent allow for unrestricted paths; deny with a
//! per-tool contextual message for restricted paths. Fail-closed on
//! any internal error (the CLI handler maps that to exit 2).
//!
//! Pure (no stdin / stdout / `process::exit` / `panic` in the happy
//! path or in `Fail`): the CLI handler is the only piece that touches
//! I/O, so unit tests run without spawning the binary.

#[cfg(test)]
mod tests;

use std::path::{Component, Path, PathBuf};

use os_shim::System;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::permissions::resolve::{
    ResolvedPermissions, ResolvedTrustedRoot, TrustedRootPath, resolve_permissions,
};
use crate::permissions::claude_sync::BASH_MUTATORS;
use crate::permissions::op_guard::target_is_sanctioned;

const WRAPPER_PREFIXES: &[WrapperPrefix] = &[WrapperPrefix {
    has_proxy_subcommand: true,
    name: "rtk",
}];

struct WrapperPrefix {
    has_proxy_subcommand: bool,
    name: &'static str,
}

/// Decision JSON shape Claude Code expects on stdout.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Decision {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: DecisionInner,
}

/// Inner `hookSpecificOutput` body — pinned to the `PreToolUse` schema.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecisionInner {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: &'static str,
    #[serde(rename = "permissionDecision")]
    pub permission_decision: PermissionDecision,
    #[serde(rename = "permissionDecisionReason")]
    pub permission_decision_reason: String,
}

/// Decision values Claude Code accepts.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

/// `PreToolUse` event envelope from Claude Code on stdin.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct PreToolUseEvent {
    pub cwd: PathBuf,
    pub tool_input: Value,
    pub tool_name: String,
}

/// Outcome of `pretool`. The caller emits stdout / sets exit code.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PretoolOutcome {
    /// Restricted path touched. Emit the decision JSON; exit 0.
    Deny(Decision),
    /// Malformed input or unexpected internal error. Emit nothing on
    /// stdout; write reason to stderr; exit 2 (fail-closed).
    Fail(String),
    /// No restricted path touched (or tool not gated). Emit nothing;
    /// exit 0.
    SilentAllow,
}

/// Per-tool extracted shape that drives the decision.
enum ToolTarget {
    /// `Bash` command — the verb gates whether the path-substring
    /// check runs.
    BashCommand { command: String },
    /// `Glob`, `Grep`, unknown tool — never deny.
    NoCheck,
    /// File-touching tool (`Read`, `Write`, `Edit`, `NotebookEdit`).
    Path { path: PathBuf, tool_name: String },
}

/// Top-level entry point. Parses stdin, resolves permissions against
/// the realm anchored at `event.cwd`, and returns the outcome.
#[must_use]
pub fn pretool(system: &dyn System, stdin_bytes: &[u8]) -> PretoolOutcome {
    let event: PreToolUseEvent = match serde_json::from_slice(stdin_bytes) {
        Ok(value) => value,
        Err(err) => return PretoolOutcome::Fail(format!("malformed PreToolUse event: {err}")),
    };

    let target = match extract_target(&event) {
        Ok(target) => target,
        Err(reason) => return PretoolOutcome::Fail(reason),
    };

    let resolved = match resolve_permissions(system, &event.cwd) {
        Ok(value) => value,
        Err(err) => return PretoolOutcome::Fail(format!("permissions resolve failed: {err}")),
    };

    match target {
        ToolTarget::NoCheck => PretoolOutcome::SilentAllow,
        ToolTarget::Path { tool_name, path } => {
            let absolute = absolutise(&event.cwd, &path);
            let canonical = system.canonicalize(&absolute).unwrap_or(absolute);
            if path_is_restricted(&resolved, &canonical) {
                PretoolOutcome::Deny(build_decision(&tool_name, &canonical))
            } else {
                PretoolOutcome::SilentAllow
            }
        }
        ToolTarget::BashCommand { command } => bash_decision(&resolved, &command),
    }
}

fn extract_target(event: &PreToolUseEvent) -> Result<ToolTarget, String> {
    match event.tool_name.as_str() {
        "Read" | "Write" | "Edit" => Ok(ToolTarget::Path {
            path: required_path(&event.tool_input, "file_path", &event.tool_name)?,
            tool_name: event.tool_name.clone(),
        }),
        "NotebookEdit" => Ok(ToolTarget::Path {
            path: required_path(&event.tool_input, "notebook_path", &event.tool_name)?,
            tool_name: event.tool_name.clone(),
        }),
        "Bash" => Ok(ToolTarget::BashCommand {
            command: required_string(&event.tool_input, "command", &event.tool_name)?,
        }),
        _ => Ok(ToolTarget::NoCheck),
    }
}

fn required_path(input: &Value, key: &str, tool: &str) -> Result<PathBuf, String> {
    required_string(input, key, tool).map(PathBuf::from)
}

fn required_string(input: &Value, key: &str, tool: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| format!("missing tool_input.{key} for {tool}"))
}

fn absolutise(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    lexical_normalize(&joined)
}

/// Lexically resolve `.` and `..` components without touching disk —
/// `MockSystem`'s `canonicalize` is a join-only stub, so the hook can
/// only collapse parent traversals from the event's `cwd` by hand.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

/// True when `path` falls under any `trusted_roots` entry or any
/// `deny_ops` entry — i.e. when the realm has declared the path as
/// remargin-managed. Mirrors the deny rules `claude restrict` writes
/// into `.claude/settings.local.json` so this layer and Claude's
/// native-tool denies stay aligned.
fn path_is_restricted(resolved: &ResolvedPermissions, canonical: &Path) -> bool {
    if resolved
        .deny_ops
        .iter()
        .any(|entry| canonical == entry.path || canonical.starts_with(&entry.path))
    {
        return true;
    }
    target_is_sanctioned(canonical, &resolved.trusted_roots) && !resolved.trusted_roots.is_empty()
}

fn bash_decision(resolved: &ResolvedPermissions, command: &str) -> PretoolOutcome {
    let Some(verb) = first_verb(command) else {
        return PretoolOutcome::SilentAllow;
    };

    if !verb_triggers_check(verb, &resolved.trusted_roots) {
        return PretoolOutcome::SilentAllow;
    }

    if let Some(matched) = first_restricted_substring_match(command, &resolved.trusted_roots) {
        return PretoolOutcome::Deny(build_bash_decision(&matched, Some(verb)));
    }

    PretoolOutcome::SilentAllow
}

/// Pull the verb token from a Bash command. Skips leading whitespace,
/// `KEY=value` env-var prefixes (`FOO=bar cat /x` → `cat`), and known
/// command-wrapper prefixes (`rtk sed file` → `sed`,
/// `rtk proxy sed file` → `sed`).
fn first_verb(command: &str) -> Option<&str> {
    let mut iter = command
        .split_whitespace()
        .skip_while(|tok| is_env_assignment(tok));

    loop {
        let candidate = iter.next()?;
        let mut matched: Option<&WrapperPrefix> = None;
        for wrapper in WRAPPER_PREFIXES {
            if candidate == wrapper.name {
                matched = Some(wrapper);
                break;
            }
        }
        let Some(wrapper) = matched else {
            return Some(candidate);
        };
        if wrapper.has_proxy_subcommand {
            let mut peek = iter.clone();
            if peek.next() == Some("proxy") {
                iter = peek;
            }
        }
    }
}

fn is_env_assignment(token: &str) -> bool {
    let Some(eq_idx) = token.find('=') else {
        return false;
    };
    let name = &token[..eq_idx];
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.chars().next().is_some_and(|c| !c.is_ascii_digit())
}

fn verb_triggers_check(verb: &str, trusted_roots: &[ResolvedTrustedRoot]) -> bool {
    if bash_mutator_verbs().any(|known| known == verb) {
        return true;
    }
    trusted_roots
        .iter()
        .flat_map(|entry| entry.also_deny_bash.iter())
        .any(|extra| extra == verb)
}

/// The verb half of every `BASH_MUTATORS` template — first
/// whitespace-separated token, deduped naturally.
fn bash_mutator_verbs() -> impl Iterator<Item = &'static str> {
    BASH_MUTATORS
        .iter()
        .filter_map(|template| template.split_whitespace().next())
}

fn first_restricted_substring_match(
    command: &str,
    trusted_roots: &[ResolvedTrustedRoot],
) -> Option<String> {
    for entry in trusted_roots {
        let needle = match &entry.path {
            TrustedRootPath::Absolute(path) => path.display().to_string(),
            TrustedRootPath::Wildcard { realm_root } => realm_root.display().to_string(),
        };
        if !needle.is_empty() && command.contains(&needle) {
            return Some(needle);
        }
    }
    None
}

fn build_decision(tool: &str, path: &Path) -> Decision {
    Decision {
        hook_specific_output: DecisionInner {
            hook_event_name: "PreToolUse",
            permission_decision: PermissionDecision::Deny,
            permission_decision_reason: message_for(tool, path),
        },
    }
}

fn build_bash_decision(matched_path: &str, verb: Option<&str>) -> Decision {
    let suffix = verb.and_then(verb_guidance).unwrap_or(
        "There is no direct shell substitute -- use the appropriate remargin MCP tool for the \
         underlying operation, or do not access this path through shell.",
    );
    Decision {
        hook_specific_output: DecisionInner {
            hook_event_name: "PreToolUse",
            permission_decision: PermissionDecision::Deny,
            permission_decision_reason: format!(
                "This shell command would touch the remargin-managed path {matched_path}. \
                 {suffix}"
            ),
        },
    }
}

fn verb_guidance(verb: &str) -> Option<&'static str> {
    Some(match verb {
        "sed" | "awk" => {
            "Use `mcp__remargin__get` with `start_line`/`end_line` for reads, or \
             `mcp__remargin__write` partial for in-place edits."
        }
        "cat" | "less" | "more" => "Use `mcp__remargin__get` (text mode by default).",
        "head" | "tail" => {
            "Use `mcp__remargin__get` with bounded `start_line`/`end_line` (consult \
             `mcp__remargin__metadata` first)."
        }
        "grep" | "rg" | "ag" => {
            "Use `mcp__remargin__search` (file-scoped; respects comment / body distinction)."
        }
        "find" => {
            "Use `mcp__remargin__query` for comment/file enumeration, or `mcp__remargin__ls` for \
             listings."
        }
        "mv" => "Use `mcp__remargin__mv` -- preserves comment IDs + thread state.",
        "rm" => {
            "Use `mcp__remargin__rm` (sandbox-aware) or `mcp__remargin__purge` when you mean drop \
             comments only."
        }
        "cp" => {
            "Use `mcp__remargin__get` + `mcp__remargin__write {create: true}` -- `cp` bypasses \
             frontmatter injection on markdown."
        }
        "tee" | "dd" => "Use `mcp__remargin__write` instead of redirecting output to the file.",
        "vim" | "nvim" | "nano" | "code" => {
            "Use `mcp__remargin__write` or `mcp__remargin__edit` for managed paths -- your editor \
             would bypass the comment-preservation guarantees."
        }
        "git" => {
            "If the managed path is being staged or moved by git, run the matching \
             `mcp__remargin__*` op first (mv / rm / write), then let git track the result."
        }
        _ => return None,
    })
}

fn message_for(tool: &str, path: &Path) -> String {
    let p = path.display();
    match tool {
        "Read" => format!("Path {p} is remargin-managed. Use mcp__remargin__get instead."),
        "Write" => format!("Path {p} is remargin-managed. Use mcp__remargin__write instead."),
        "Edit" => format!("Path {p} is remargin-managed. Use mcp__remargin__edit instead."),
        "NotebookEdit" => format!(
            "Path {p} is remargin-managed. Use mcp__remargin__write (notebook edits are text \
             edits here)."
        ),
        _ => format!("Path {p} is remargin-managed; use the appropriate remargin MCP tool."),
    }
}
