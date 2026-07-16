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

use core::mem;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use os_shim::System;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::permissions::resolve::{
    ResolvedPermissions, TrustedRootPath, resolve_permissions,
};

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
    /// `Bash` command — every path-shaped word is resolved against the
    /// realm governing it and denied if it lands inside one.
    BashCommand { command: String },
    /// Unknown / ungated tool — never deny.
    NoCheck,
    /// Path-touching tool (`Read`, `Write`, `Edit`, `MultiEdit`,
    /// `NotebookEdit`, `Grep`, `Glob`).
    Path { path: PathBuf, tool_name: String },
}

/// Top-level entry point. Parses stdin, extracts the target, and
/// resolves permissions from the realm that governs it.
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

    match target {
        ToolTarget::NoCheck => PretoolOutcome::SilentAllow,
        ToolTarget::Path { tool_name, path } => {
            // event.cwd's only job is rooting a relative target; the
            // governing realm is resolved from the target itself.
            let absolute = absolutise(&event.cwd, &path);
            let canonical = system.canonicalize(&absolute).unwrap_or(absolute);
            let resolved = match resolve_for_target(system, &canonical) {
                Ok(value) => value,
                Err(err) => {
                    return PretoolOutcome::Fail(format!("permissions resolve failed: {err}"));
                }
            };
            if path_is_restricted(&resolved, &canonical) {
                PretoolOutcome::Deny(build_decision(&tool_name, &canonical))
            } else {
                PretoolOutcome::SilentAllow
            }
        }
        ToolTarget::BashCommand { command } => {
            // cli_allowed is a folder policy keyed off the session cwd;
            // path restriction is resolved per-word from the target.
            let policy = match resolve_permissions(system, &event.cwd) {
                Ok(value) => value,
                Err(err) => {
                    return PretoolOutcome::Fail(format!("permissions resolve failed: {err}"));
                }
            };
            bash_decision(system, policy.cli_allowed(), &command, &event.cwd)
        }
    }
}

/// Resolve the realm governing `canonical` by walking up from its
/// parent directory — independent of the session cwd. Shared so the
/// Bash branch can resolve each path-shaped word the same way.
///
/// # Errors
///
/// Surfaces the same parse-time errors as [`resolve_permissions`].
pub fn resolve_for_target(system: &dyn System, canonical: &Path) -> Result<ResolvedPermissions> {
    let start = canonical.parent().unwrap_or(canonical);
    resolve_permissions(system, start)
}

fn extract_target(event: &PreToolUseEvent) -> Result<ToolTarget, String> {
    match event.tool_name.as_str() {
        "Read" | "Write" | "Edit" | "MultiEdit" => Ok(ToolTarget::Path {
            path: required_path(&event.tool_input, "file_path", &event.tool_name)?,
            tool_name: event.tool_name.clone(),
        }),
        "NotebookEdit" => Ok(ToolTarget::Path {
            path: required_path(&event.tool_input, "notebook_path", &event.tool_name)?,
            tool_name: event.tool_name.clone(),
        }),
        // `Grep` / `Glob` may omit `path`, defaulting the search root to
        // the session cwd; the absent optional field must not fail-closed.
        "Grep" | "Glob" => Ok(ToolTarget::Path {
            path: optional_path(&event.tool_input, "path").unwrap_or_else(|| event.cwd.clone()),
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

fn optional_path(input: &Value, key: &str) -> Option<PathBuf> {
    input.get(key).and_then(Value::as_str).map(PathBuf::from)
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

/// The single definition of "restricted", shared by the Path branch and
/// the per-word Bash branch so the two cannot drift. True when
/// `candidate` falls under any `deny_ops` entry or is covered by any
/// `trusted_roots` entry — i.e. when the realm has declared the path as
/// remargin-managed. Trusted-root coverage is glob-aware so a bash word
/// carrying glob metacharacters (`/r/sec*ret/foo`) that could expand into
/// a root is denied without touching disk; a literal path (every
/// Path-branch target) matches by plain component equality, unchanged.
fn path_is_restricted(resolved: &ResolvedPermissions, candidate: &Path) -> bool {
    if resolved
        .deny_ops
        .iter()
        .any(|entry| candidate == entry.path || candidate.starts_with(&entry.path))
    {
        return true;
    }
    resolved
        .trusted_roots
        .iter()
        .any(|entry| root_covers_word(&entry.path, candidate))
}

/// Parse `command` into simple commands, resolve every path-shaped word
/// against the realm that governs it, and deny the first one that lands
/// inside a protected realm — regardless of verb. The verb is no longer
/// a gate; it only selects the deny-message guidance.
fn bash_decision(
    system: &dyn System,
    cli_allowed: bool,
    command: &str,
    event_cwd: &Path,
) -> PretoolOutcome {
    let commands = split_into_simple_commands(command);

    // Folder-level CLI policy: deny any `remargin` CLI invocation when
    // the effective policy is false (nearest-wins, default = allowed).
    if !cli_allowed && first_verb_is_remargin(&commands) {
        return PretoolOutcome::Deny(build_cli_denied_decision());
    }

    let mut cwd = event_cwd.to_path_buf();
    for tokens in &commands {
        if let Some(outcome) = evaluate_simple_command(system, tokens, &mut cwd) {
            return outcome;
        }
    }
    PretoolOutcome::SilentAllow
}

/// Resolve one simple command's path-shaped words. Returns `Some` to
/// short-circuit the whole command (a `Deny` or a fail-closed `Fail`);
/// `None` to keep scanning. Tracks `cd` into `cwd` for later commands.
fn evaluate_simple_command(
    system: &dyn System,
    tokens: &[String],
    cwd: &mut PathBuf,
) -> Option<PretoolOutcome> {
    let verb_info = command_verb(tokens);
    let verb = verb_info.map(|(_, name)| name);

    // The remargin CLI is the sanctioned surface; do not gate its args.
    if verb == Some("remargin") {
        return None;
    }

    for token in tokens {
        for run in path_runs(token) {
            let candidate = resolve_run(system, run, cwd);
            let resolved = match resolve_for_target(system, &candidate) {
                Ok(value) => value,
                Err(err) => {
                    return Some(PretoolOutcome::Fail(format!(
                        "permissions resolve failed: {err}"
                    )));
                }
            };
            if path_is_restricted(&resolved, &candidate) {
                return Some(PretoolOutcome::Deny(build_bash_decision(
                    &candidate.display().to_string(),
                    verb,
                )));
            }
        }
    }

    // A non-restricted `cd` moves the base directory for later words.
    if verb == Some("cd")
        && let Some((idx, _)) = verb_info
        && let Some(dir_token) = tokens.get(idx + 1)
        && let Some(run) = path_runs(dir_token).next()
    {
        *cwd = resolve_run(system, run, cwd);
    }
    None
}

fn first_verb_is_remargin(commands: &[Vec<String>]) -> bool {
    commands
        .first()
        .and_then(|tokens| command_verb(tokens))
        .map(|(_, name)| name)
        == Some("remargin")
}

/// Split the command line into simple commands on `&&`, `||`, `;`, `|`,
/// and subshell / command-substitution boundaries (`(`, `)`, `$(`,
/// backticks), quote- and escape-aware. Subshells and substitutions are
/// flattened: their inner commands join the sequence, which is
/// fail-closed for restriction detection.
fn split_into_simple_commands(command: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = command.chars().collect();
    let mut commands: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut word_started = false;
    let mut idx = 0;

    while idx < chars.len() {
        let ch = chars[idx];
        match ch {
            '\'' => {
                word_started = true;
                idx += 1;
                while idx < chars.len() && chars[idx] != '\'' {
                    word.push(chars[idx]);
                    idx += 1;
                }
                idx += 1;
            }
            '"' => {
                word_started = true;
                idx += 1;
                while idx < chars.len() && chars[idx] != '"' {
                    if chars[idx] == '\\'
                        && matches!(chars.get(idx + 1), Some('"' | '\\' | '$' | '`'))
                    {
                        word.push(chars[idx + 1]);
                        idx += 2;
                    } else {
                        word.push(chars[idx]);
                        idx += 1;
                    }
                }
                idx += 1;
            }
            '\\' => {
                if let Some(next) = chars.get(idx + 1) {
                    word.push(*next);
                    word_started = true;
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            _ if ch.is_whitespace() => {
                flush_word(&mut current, &mut word, &mut word_started);
                idx += 1;
            }
            '$' if chars.get(idx + 1) == Some(&'(') => {
                flush_word(&mut current, &mut word, &mut word_started);
                flush_command(&mut commands, &mut current);
                idx += 2;
            }
            '&' | '|' | ';' | '(' | ')' | '`' => {
                flush_word(&mut current, &mut word, &mut word_started);
                flush_command(&mut commands, &mut current);
                if matches!(ch, '&' | '|') && chars.get(idx + 1) == Some(&ch) {
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            _ => {
                word.push(ch);
                word_started = true;
                idx += 1;
            }
        }
    }
    flush_word(&mut current, &mut word, &mut word_started);
    flush_command(&mut commands, &mut current);
    commands
}

fn flush_word(current: &mut Vec<String>, word: &mut String, word_started: &mut bool) {
    if *word_started {
        current.push(mem::take(word));
        *word_started = false;
    }
}

fn flush_command(commands: &mut Vec<Vec<String>>, current: &mut Vec<String>) {
    if !current.is_empty() {
        commands.push(mem::take(current));
    }
}

/// The verb of a simple command: skip leading `KEY=value` env prefixes
/// and known command-wrapper prefixes (`rtk`, `rtk proxy`). Returns the
/// verb's index and text so the caller can find a following `cd` target.
fn command_verb(tokens: &[String]) -> Option<(usize, &str)> {
    let mut idx = 0;
    while tokens
        .get(idx)
        .is_some_and(|token| is_env_assignment(token))
    {
        idx += 1;
    }
    loop {
        let candidate = tokens.get(idx)?;
        let mut wrapper: Option<&WrapperPrefix> = None;
        for entry in WRAPPER_PREFIXES {
            if candidate.as_str() == entry.name {
                wrapper = Some(entry);
                break;
            }
        }
        let Some(entry) = wrapper else {
            return Some((idx, candidate.as_str()));
        };
        idx += 1;
        if entry.has_proxy_subcommand && tokens.get(idx).map(String::as_str) == Some("proxy") {
            idx += 1;
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

/// Maximal path-shaped runs inside a token: the token split on
/// characters that cannot appear unquoted in a bare path, so an
/// absolute path embedded in an interpreter argument
/// (`open('/r/secret/f','w')`) is still recovered as `/r/secret/f`.
/// Glob metacharacters (`*`, `?`, `[`, `]`) are kept in the run.
fn path_runs(token: &str) -> impl Iterator<Item = &str> {
    token.split(is_run_break).filter(|run| !run.is_empty())
}

const fn is_run_break(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\'' | '"'
                | '`'
                | '('
                | ')'
                | ','
                | '='
                | ':'
                | '<'
                | '>'
                | '!'
                | '{'
                | '}'
                | '|'
                | '&'
                | ';'
                | '$'
        )
}

/// Absolutize a path run against `base_cwd` (honoring a tracked `cd`),
/// expand a leading `~`, lexically normalize, and canonicalize the same
/// way the `Path` branch does so symlinks resolve into the realm.
fn resolve_run(system: &dyn System, run: &str, base_cwd: &Path) -> PathBuf {
    let expanded = expand_tilde(system, run);
    let raw = Path::new(&expanded);
    let absolute = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base_cwd.join(raw)
    };
    let normalized = lexical_normalize(&absolute);
    system.canonicalize(&normalized).unwrap_or(normalized)
}

fn expand_tilde(system: &dyn System, run: &str) -> String {
    if run == "~" {
        return system.env_var("HOME").unwrap_or_else(|_err| run.to_owned());
    }
    if let Some(rest) = run.strip_prefix("~/")
        && let Ok(home) = system.env_var("HOME")
    {
        return format!("{home}/{rest}");
    }
    run.to_owned()
}

/// True when the glob-aware trusted root `root` covers `candidate`.
/// Component-wise: each anchor component must match the aligned word
/// component, where a word component with glob metacharacters matches
/// as a pattern (so `/r/sec*ret/foo` covers the `secret` root).
fn root_covers_word(root: &TrustedRootPath, candidate: &Path) -> bool {
    let anchor = match root {
        TrustedRootPath::Absolute(path) => path.as_path(),
        TrustedRootPath::Wildcard { realm_root } => realm_root.as_path(),
    };
    let anchor_components: Vec<&OsStr> = anchor.components().map(Component::as_os_str).collect();
    let word_components: Vec<&OsStr> = candidate.components().map(Component::as_os_str).collect();
    if word_components.len() < anchor_components.len() {
        return false;
    }
    anchor_components
        .iter()
        .zip(&word_components)
        .all(|(anchor_component, word_component)| {
            component_matches(word_component, anchor_component)
        })
}

/// `pattern` (a candidate path component, possibly with glob
/// metacharacters) matches `literal` (a trusted-root component). Plain
/// components compare by equality; glob components match with `*` / `?`,
/// and `[...]` classes are treated as a single-character wildcard
/// (over-matching is safe here — it only widens denial).
fn component_matches(pattern: &OsStr, literal: &OsStr) -> bool {
    let pattern_str = pattern.to_string_lossy();
    if !pattern_str.contains(is_glob_meta) {
        return pattern == literal;
    }
    let literal_chars: Vec<char> = literal.to_string_lossy().chars().collect();
    wildcard_matches(&declassify(&pattern_str), &literal_chars)
}

const fn is_glob_meta(c: char) -> bool {
    matches!(c, '*' | '?' | '[')
}

/// Rewrite each `[...]` class to a single `?` so the matcher only has to
/// reason about `*` and `?`. An unterminated `[` is left literal.
fn declassify(pattern: &str) -> Vec<char> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut idx = 0;
    while idx < chars.len() {
        if chars[idx] == '['
            && let Some(close) = class_close(&chars, idx)
        {
            out.push('?');
            idx = close + 1;
            continue;
        }
        out.push(chars[idx]);
        idx += 1;
    }
    out
}

fn class_close(chars: &[char], open: usize) -> Option<usize> {
    let mut idx = open + 1;
    if chars.get(idx).is_some_and(|c| *c == '!' || *c == '^') {
        idx += 1;
    }
    // A `]` in the first position is a literal member, not the close.
    if chars.get(idx) == Some(&']') {
        idx += 1;
    }
    while idx < chars.len() {
        if chars[idx] == ']' {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

/// Wildcard match with `*` (any run, including empty) and `?` (exactly
/// one character). Standard single-pass matcher with `*` backtracking.
fn wildcard_matches(pattern: &[char], text: &[char]) -> bool {
    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut star: Option<(usize, usize)> = None;
    while t_idx < text.len() {
        if pattern
            .get(p_idx)
            .is_some_and(|c| *c == '?' || *c == text[t_idx])
        {
            p_idx += 1;
            t_idx += 1;
        } else if pattern.get(p_idx) == Some(&'*') {
            star = Some((p_idx, t_idx));
            p_idx += 1;
        } else if let Some((star_p, star_t)) = star {
            p_idx = star_p + 1;
            t_idx = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    while pattern.get(p_idx) == Some(&'*') {
        p_idx += 1;
    }
    p_idx == pattern.len()
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

fn build_cli_denied_decision() -> Decision {
    Decision {
        hook_specific_output: DecisionInner {
            hook_event_name: "PreToolUse",
            permission_decision: PermissionDecision::Deny,
            permission_decision_reason: String::from(
                "The remargin CLI is denied for agents in this folder (cli_allowed: false). \
                 Use the mcp__remargin__* tools instead.",
            ),
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
            "Use `mcp__remargin__cp` -- copies the file under remargin's guards (markdown is \
             copied body-only so the duplicate gets a clean comment history)."
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
