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
use std::path::{Component, Path, PathBuf, is_separator};

use anyhow::Result;
use os_shim::System;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::permissions::resolve::{
    ResolvedPermissions, TrustedRootPath, resolve_permissions, trusted_root_anchor,
};
use crate::permissions::op_guard::dot_folder_reallowed;

/// Verbs from the [`verb_guidance`] vocabulary whose effect on a path *at or
/// above* a trusted root destroys or relocates the protected subtree: `rm`
/// (recursive removal), `mv` (relocation / rename, or an overwrite when the
/// ancestor is the destination), `dd` and `tee` (raw / truncating writes).
/// Every other verb in that vocabulary only reads (`cat`, `less`, `head`,
/// `tail`, `grep`, `find`, `ls`, and `cp` reading from the ancestor) or edits
/// a single named file (`sed`, `awk`, `vim`, ...) -- none can recurse through
/// the ancestor into the subtree, so a benign `ls /realm` or `cat /realm/x`
/// stays allowed. Shell redirect writes (`>`, `>>`) are the non-verb member
/// of this destructive set and are detected separately.
const ANCESTOR_DESTRUCTIVE_VERBS: &[&str] = &["dd", "mv", "rm", "tee"];

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

/// A trusted root that a candidate path sits at or above -- the payload of
/// the ancestor-gap match.
struct AncestorMatch {
    realm_root: PathBuf,
    trusted_root: PathBuf,
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
            let canonical = canonicalize_existing_prefix(system, &absolute);
            let resolved = match resolve_for_target(system, &canonical) {
                Ok(value) => value,
                Err(err) => {
                    return PretoolOutcome::Fail(format!("permissions resolve failed: {err}"));
                }
            };
            if path_is_restricted(&resolved, &canonical) {
                return PretoolOutcome::Deny(build_decision(&tool_name, &canonical));
            }
            // Grep / Glob search a subtree recursively, so a search root at or
            // above a trusted root sweeps the protected subtree even though the
            // root itself is not at/below a trusted root. Read / Write / Edit
            // touch only the named path, never its subtree, so they are exempt.
            if is_search_tool(&tool_name) {
                match matching_trusted_root_ancestor(system, &canonical) {
                    Ok(Some(_)) => {
                        return PretoolOutcome::Deny(build_decision(&tool_name, &canonical));
                    }
                    Ok(None) => {}
                    Err(err) => {
                        return PretoolOutcome::Fail(format!("permissions resolve failed: {err}"));
                    }
                }
            }
            PretoolOutcome::SilentAllow
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

/// `Grep` and `Glob` walk their `path` root recursively; every other gated
/// Path tool touches only the exact path named.
fn is_search_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Glob" | "Grep")
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
///
/// One carve-out: a path the realm re-allowed through `allow_dot_folders`
/// is not restricted, mirroring the projected `<root>/<folder>/**`
/// re-allow. An explicit `deny_ops` match is more specific than that
/// re-allow, so it is checked first and never lifted — matching the op
/// guard, which denies `deny_ops` before consulting `allow_dot_folders`.
fn path_is_restricted(resolved: &ResolvedPermissions, candidate: &Path) -> bool {
    // A realm locked to an empty allow-set (`trusted_roots: []`) denies
    // every target under it. `candidate`'s realm is resolved by walking up
    // from it, so a lock in `resolved` is an ancestor's — the candidate is
    // inside the locked realm by construction. Mirrors the op guard's
    // `find_trusted_roots_violation` fallback so the layers cannot diverge.
    if resolved.locked_to_empty_roots() {
        return true;
    }
    if resolved
        .deny_ops
        .iter()
        .any(|entry| candidate == entry.path || candidate.starts_with(&entry.path))
    {
        return true;
    }
    let under_root = resolved
        .trusted_roots
        .iter()
        .any(|entry| root_covers_word(&entry.path, candidate));
    under_root && !reallowed_dot_folder_path(resolved, candidate)
}

/// `true` when `candidate` sits inside a dot-folder the realm explicitly
/// re-allowed via `allow_dot_folders`. Delegates the dot-folder walk to
/// the op guard's shared helper so the hook and the guard cannot diverge.
fn reallowed_dot_folder_path(resolved: &ResolvedPermissions, candidate: &Path) -> bool {
    let allowed = resolved.allow_dot_folder_names();
    resolved
        .trusted_roots
        .iter()
        .any(|entry| dot_folder_reallowed(trusted_root_anchor(entry), candidate, &allowed))
}

/// Directory of the `.remargin.yaml` that governs `candidate` — the realm
/// the no-equivalent fallback names. Mirrors `path_is_restricted`'s match
/// order (`deny_ops`, then `trusted_roots`, then a `trusted_roots: []` lock)
/// so the named realm is the one that actually triggered the deny.
fn realm_root_for(resolved: &ResolvedPermissions, candidate: &Path) -> PathBuf {
    let source = resolved
        .deny_ops
        .iter()
        .find(|entry| candidate == entry.path || candidate.starts_with(&entry.path))
        .map(|entry| entry.source_file.as_path())
        .or_else(|| {
            resolved
                .trusted_roots
                .iter()
                .find(|entry| root_covers_word(&entry.path, candidate))
                .map(|entry| entry.source_file.as_path())
        })
        .or(resolved.trusted_roots_lock.as_deref());
    source.map_or_else(
        || candidate.parent().unwrap_or(candidate).to_path_buf(),
        |file| file.parent().unwrap_or(file).to_path_buf(),
    )
}

/// `Some` when `candidate` is an ancestor of (or equal to) a trusted root
/// declared by the realm resolved *from the candidate itself*.
/// [`resolve_for_target`] deliberately walks up from the candidate's parent,
/// so a candidate that is a realm root never observes its own
/// `.remargin.yaml`; resolving from the candidate directly is the only way to
/// see a trusted root nested beneath it. `path_is_restricted` (the normal,
/// verb-independent check) already covers every candidate at or below a
/// trusted root, so this is consulted only for the complementary ancestor
/// case.
///
/// Documented blind spot, out of scope: a candidate ABOVE the realm root
/// (`rm -rf ~/src` wiping a realm at `~/src/vault`) is undetectable -- the
/// upward walk from the candidate never reaches the realm's `.remargin.yaml`,
/// which lives below it. Only candidates at or below the realm root are
/// covered -- the same reach the retired projected deny rules had.
fn matching_trusted_root_ancestor(
    system: &dyn System,
    candidate: &Path,
) -> Result<Option<AncestorMatch>> {
    let resolved = resolve_permissions(system, candidate)?;
    Ok(resolved.trusted_roots.iter().find_map(|entry| {
        let anchor = trusted_root_anchor(entry);
        word_covers_root(candidate, anchor).then(|| AncestorMatch {
            realm_root: entry
                .source_file
                .parent()
                .unwrap_or(&entry.source_file)
                .to_path_buf(),
            trusted_root: anchor.to_path_buf(),
        })
    }))
}

/// `true` when `word`'s components are a prefix (equal length or shorter) of
/// the trusted-root `anchor`'s components -- `word` is an ancestor of, or
/// equal to, `anchor`. The mirror of [`root_covers_word`], which asks the
/// opposite (the anchor is a prefix of the word). Component comparison is the
/// same glob-aware match, so a word whose glob metacharacters sit *below* the
/// realm root (`/r/a*` onto a deeper root `/r/a/secret`) is caught without
/// touching disk. A glob at the realm-root level (`/r*`) is not: resolving the
/// governing realm walks literal parent components, so `/r*` never reaches
/// `/r/.remargin.yaml`. Literal ancestor matching is the guaranteed scope.
fn word_covers_root(word: &Path, anchor: &Path) -> bool {
    let word_components: Vec<&OsStr> = word.components().map(Component::as_os_str).collect();
    let anchor_components: Vec<&OsStr> = anchor.components().map(Component::as_os_str).collect();
    if word_components.len() > anchor_components.len() {
        return false;
    }
    word_components
        .iter()
        .zip(&anchor_components)
        .all(|(word_component, anchor_component)| {
            component_matches(word_component, anchor_component)
        })
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
    let mut cd_active = false;
    for tokens in &commands {
        if let Some(outcome) = evaluate_simple_command(system, tokens, &mut cwd, &mut cd_active) {
            return outcome;
        }
    }
    PretoolOutcome::SilentAllow
}

/// Resolve one simple command's path-shaped words against the realm that
/// governs each. Returns `Some` to short-circuit the whole command (a
/// `Deny` or a fail-closed `Fail`); `None` to keep scanning. A run with no
/// path evidence is a bare word (a verb, subcommand, or flag) and is a path
/// candidate only once a tracked `cd` has moved `cwd` and the word is an
/// argument — otherwise the bare verb would resolve to `<cwd>/<verb>` and
/// self-deny a wildcard realm rooted at the cwd. Tracks `cd` into `cwd`
/// (and flips `cd_active`) for later commands.
fn evaluate_simple_command(
    system: &dyn System,
    tokens: &[String],
    cwd: &mut PathBuf,
    cd_active: &mut bool,
) -> Option<PretoolOutcome> {
    let verb_info = command_verb(tokens);
    let verb = verb_info.map(|(_, name)| name);
    let verb_idx = verb_info.map(|(idx, _)| idx);

    // The remargin CLI is the sanctioned surface; do not gate its args.
    if verb == Some("remargin") {
        return None;
    }

    let destructive_verb = verb.is_some_and(is_ancestor_destructive_verb);
    // A redirect operator (`>`, `>>`, `2>`) makes the following word a write
    // target; carried across tokens so `> /r` and glued `>/r` both register.
    let mut redirect_pending = false;
    for (idx, token) in tokens.iter().enumerate() {
        let redirect_target = redirect_pending || token_has_glued_redirect(token);
        let token_is_operator = is_redirect_operator(token);
        redirect_pending = token_is_operator;
        if token_is_operator {
            continue;
        }
        let is_argument = verb_idx.is_some_and(|boundary| idx > boundary);
        for run in path_runs(token) {
            let is_candidate = has_path_evidence(run) || (is_argument && *cd_active);
            if !is_candidate {
                continue;
            }
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
                let realm_root = realm_root_for(&resolved, &candidate);
                return Some(PretoolOutcome::Deny(build_bash_decision(
                    &candidate.display().to_string(),
                    &realm_root,
                    verb,
                    tokens,
                    verb_idx,
                )));
            }
            // Ancestor gap: the word is not itself at/below a trusted root, but
            // a destructive verb or a redirect write on a word at or above one
            // would reach into the protected subtree. Reads (`cat`, `ls`) of an
            // ancestor stay allowed -- only the destructive set gates here.
            if destructive_verb || redirect_target {
                match matching_trusted_root_ancestor(system, &candidate) {
                    Ok(Some(found)) => {
                        return Some(PretoolOutcome::Deny(build_ancestor_destructive_decision(
                            &candidate.display().to_string(),
                            &found.trusted_root,
                            &found.realm_root,
                        )));
                    }
                    Ok(None) => {}
                    Err(err) => {
                        return Some(PretoolOutcome::Fail(format!(
                            "permissions resolve failed: {err}"
                        )));
                    }
                }
            }
        }
    }

    // A non-restricted `cd` moves the base directory for later words; from
    // there on bare argument words are meaningful path candidates.
    if verb == Some("cd")
        && let Some((idx, _)) = verb_info
        && let Some(dir_token) = tokens.get(idx + 1)
        && let Some(run) = path_runs(dir_token).next()
    {
        *cwd = resolve_run(system, run, cwd);
        *cd_active = true;
    }
    None
}

/// A run carries path evidence when it names a path rather than a bare
/// word: it contains a path separator, starts with `~`, is `.`/`..`, or
/// carries a glob metacharacter. Bare words (`ls`, `status`, `--release`)
/// have none, so a command verb resolving against the cwd cannot itself
/// match a wildcard realm rooted at that cwd.
fn has_path_evidence(run: &str) -> bool {
    run.chars().any(is_separator)
        || run.starts_with('~')
        || run == "."
        || run == ".."
        || run.contains(is_glob_meta)
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

/// Byte length of a leading shell redirection operator on `token`: optional
/// file-descriptor digits or `&`, then `>` or `>>`. `>`, `>>`, `2>`, `1>>`,
/// `&>` all match; a plain path never does. Input redirects (`<`) never write
/// and so are not matched.
fn redirect_operator_len(token: &str) -> Option<usize> {
    let bytes = token.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx].is_ascii_digit() || bytes[idx] == b'&') {
        idx += 1;
    }
    if bytes.get(idx) != Some(&b'>') {
        return None;
    }
    idx += 1;
    if bytes.get(idx) == Some(&b'>') {
        idx += 1;
    }
    Some(idx)
}

/// The whole token is a redirect operator (`>`, `2>>`), so the next token is
/// the write target.
fn is_redirect_operator(token: &str) -> bool {
    redirect_operator_len(token) == Some(token.len())
}

/// The token glues a redirect operator onto its target (`>/r`, `2>>/r`), so
/// the token's own path runs are the write target.
fn token_has_glued_redirect(token: &str) -> bool {
    redirect_operator_len(token).is_some_and(|len| len < token.len())
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
    canonicalize_existing_prefix(system, &normalized)
}

/// Canonicalize the deepest ancestor of `absolute` that exists on disk,
/// then rejoin the not-yet-created tail. `canonicalize` fails outright for
/// any nonexistent leaf — every new-file `Write` — so a blanket
/// failure-inside-a-realm deny would block legitimate writes to
/// unrestricted subpaths. Resolving the existing prefix instead defeats a
/// symlinked directory in the prefix of a missing leaf
/// (`<realm>/link/new.md`, where `link` points elsewhere): a purely lexical
/// path would be checked under the link name, not its target. Restriction
/// is therefore always resolved on a real path — never silently allowed as
/// unchecked. `MockSystem`'s join-only `canonicalize` never fails, so this
/// collapses to plain canonicalization there.
fn canonicalize_existing_prefix(system: &dyn System, absolute: &Path) -> PathBuf {
    let mut tail: Vec<&OsStr> = Vec::new();
    let mut current = absolute;
    loop {
        if let Ok(canonical) = system.canonicalize(current) {
            let mut resolved = canonical;
            for component in tail.iter().rev() {
                resolved.push(component);
            }
            return resolved;
        }
        let Some(name) = current.file_name() else {
            return absolute.to_path_buf();
        };
        tail.push(name);
        let Some(parent) = current.parent() else {
            return absolute.to_path_buf();
        };
        current = parent;
    }
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

fn build_bash_decision(
    matched_path: &str,
    realm_root: &Path,
    verb: Option<&str>,
    tokens: &[String],
    verb_idx: Option<usize>,
) -> Decision {
    let reason = verb
        .and_then(|name| verb_guidance(name, matched_path, tokens, verb_idx))
        .map_or_else(
            || no_equivalent_message(matched_path, realm_root),
            |guidance| {
                format!(
                    "This shell command would touch the remargin-managed path {matched_path}. \
                     {guidance}"
                )
            },
        );
    Decision {
        hook_specific_output: DecisionInner {
            hook_event_name: "PreToolUse",
            permission_decision: PermissionDecision::Deny,
            permission_decision_reason: reason,
        },
    }
}

fn is_ancestor_destructive_verb(verb: &str) -> bool {
    ANCESTOR_DESTRUCTIVE_VERBS.contains(&verb)
}

/// Per-verb redirect, with the blocked command's own arguments carried into
/// the exact replacement op so the next call is a copy-paste. `None` means
/// the verb has no remargin equivalent — the caller emits the realm-scoped
/// no-equivalent message instead.
fn verb_guidance(
    verb: &str,
    matched_path: &str,
    tokens: &[String],
    verb_idx: Option<usize>,
) -> Option<String> {
    Some(match verb {
        "sed" | "awk" => format!(
            "Use `mcp__remargin__get path={matched_path}` with `start_line`/`end_line` for reads, \
             or `mcp__remargin__write path={matched_path}` partial for in-place edits."
        ),
        "cat" | "less" | "more" => {
            format!("Use `mcp__remargin__get path={matched_path}` (text mode by default).")
        }
        "head" | "tail" => format!(
            "Use `mcp__remargin__get path={matched_path}` with bounded `start_line`/`end_line` \
             (consult `mcp__remargin__metadata` first)."
        ),
        "grep" | "rg" | "ag" => {
            let pattern = first_non_flag_arg(tokens, verb_idx).unwrap_or("<pattern>");
            format!(
                "Use `mcp__remargin__search pattern={pattern} path={matched_path}` (file-scoped; \
                 respects comment / body distinction)."
            )
        }
        "find" => format!(
            "Use `mcp__remargin__query` for comment/file enumeration, or \
             `mcp__remargin__ls path={matched_path}` for listings."
        ),
        "ls" => format!("Use `mcp__remargin__ls path={matched_path}`."),
        "mv" => {
            let (src, dst) = mv_src_dst(tokens, verb_idx, matched_path);
            format!(
                "Use `mcp__remargin__mv src={src} dst={dst}` -- preserves comment IDs + thread \
                 state."
            )
        }
        "rm" => format!(
            "Use `mcp__remargin__rm path={matched_path}` (sandbox-aware) or `mcp__remargin__purge` \
             when you mean drop comments only."
        ),
        "cp" => String::from(
            "Use `mcp__remargin__cp` -- copies the file under remargin's guards (markdown is \
             copied body-only so the duplicate gets a clean comment history).",
        ),
        "tee" | "dd" => format!(
            "Use `mcp__remargin__write path={matched_path}` instead of redirecting output to the \
             file."
        ),
        "vim" | "nvim" | "nano" | "code" => format!(
            "Use `mcp__remargin__write path={matched_path}` or `mcp__remargin__edit \
             path={matched_path}` for managed paths -- your editor would bypass the \
             comment-preservation guarantees."
        ),
        "git" => String::from(
            "If the managed path is being staged or moved by git, run the matching \
             `mcp__remargin__*` op first (mv / rm / write), then let git track the result.",
        ),
        _ => return None,
    })
}

/// The fallback when the blocked verb has no remargin equivalent (a build,
/// a script writing into the realm). It names the realm and rules the work
/// out of it — deliberately no alternative op, no mention of unrestricting
/// or asking. Explaining the rule is what ends the agent's retry loop.
fn no_equivalent_message(matched_path: &str, realm_root: &Path) -> String {
    let realm = realm_root.display();
    format!(
        "This shell command would touch the remargin-managed path {matched_path} inside the realm \
         rooted at {realm}. This kind of work does not belong inside the realm -- it belongs \
         outside {realm}, and there is no remargin operation that performs it."
    )
}

/// Deny message for a destructive command (or redirect write) whose target
/// sits at or above a trusted root. Names the target, the managed subtree it
/// would reach, and the realm -- then rules the work out rather than offering
/// a bogus single-file redirect, since no remargin op deletes a realm root or
/// its subtree wholesale.
fn build_ancestor_destructive_decision(
    target: &str,
    trusted_root: &Path,
    realm_root: &Path,
) -> Decision {
    let root = trusted_root.display();
    let realm = realm_root.display();
    let reason = format!(
        "This shell command targets {target}, which sits at or above the remargin-managed subtree \
         rooted at {root} inside the realm at {realm}. A destructive operation on {target} would \
         reach into and damage that managed subtree, and no remargin operation deletes the realm \
         wholesale. Operate on specific managed paths with the mcp__remargin__* tools, or do this \
         work outside {realm}."
    );
    Decision {
        hook_specific_output: DecisionInner {
            hook_event_name: "PreToolUse",
            permission_decision: PermissionDecision::Deny,
            permission_decision_reason: reason,
        },
    }
}

/// Arguments of a simple command that are not option flags — the words after
/// the verb that do not start with `-`.
fn non_flag_args(tokens: &[String], verb_idx: Option<usize>) -> impl Iterator<Item = &str> {
    let start = verb_idx.map_or(0, |idx| idx + 1);
    tokens
        .iter()
        .skip(start)
        .map(String::as_str)
        .filter(|token| !token.starts_with('-'))
}

fn first_non_flag_arg(tokens: &[String], verb_idx: Option<usize>) -> Option<&str> {
    non_flag_args(tokens, verb_idx).next()
}

/// The `mv` source and destination — its first two non-flag arguments. Falls
/// back to the matched path for a source it could not recover.
fn mv_src_dst<'cmd>(
    tokens: &'cmd [String],
    verb_idx: Option<usize>,
    matched_path: &'cmd str,
) -> (&'cmd str, &'cmd str) {
    let mut args = non_flag_args(tokens, verb_idx);
    let src = args.next().unwrap_or(matched_path);
    let dst = args.next().unwrap_or("<dst>");
    (src, dst)
}

fn message_for(tool: &str, path: &Path) -> String {
    let p = path.display();
    match tool {
        "Read" => format!("Path {p} is remargin-managed. Use mcp__remargin__get path={p} instead."),
        "Write" | "MultiEdit" => {
            format!("Path {p} is remargin-managed. Use mcp__remargin__write path={p} instead.")
        }
        "Edit" => {
            format!("Path {p} is remargin-managed. Use mcp__remargin__edit path={p} instead.")
        }
        "NotebookEdit" => format!(
            "Path {p} is remargin-managed. Use mcp__remargin__write path={p} (notebook edits are \
             text edits here)."
        ),
        "Grep" => format!(
            "Path {p} is remargin-managed. Use mcp__remargin__search path={p} (file-scoped; \
             respects comment / body distinction)."
        ),
        "Glob" => format!("Path {p} is remargin-managed. Use mcp__remargin__ls path={p}."),
        _ => format!("Path {p} is remargin-managed; use the appropriate remargin MCP tool."),
    }
}
