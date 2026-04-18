//! Path expansion helpers — the single chokepoint for `~`, `$VAR`, `${VAR}`,
//! and (on Windows) `%VAR%` substitution.
//!
//! This module consolidates what used to be scattered across the CLI, MCP,
//! and config layers. Call [`expand_path`] once at each adapter boundary
//! (CLI clap value parser, MCP tool dispatcher) — downstream code receives
//! already-expanded paths.
//!
//! ## Semantics
//!
//! - Leading `~` or `~/` is expanded against `$HOME` (or the platform
//!   equivalent). `~user` is an explicit error — write out the full path.
//! - `$VAR`, `${VAR}`, and (on Windows) `%VAR%` are replaced with the
//!   environment variable's value. Undefined variables produce a clear
//!   error naming the variable rather than leaving the literal in place.
//! - Absolute and relative paths without sigils pass through unchanged.
//! - Expansion is purely string-level — no canonicalization, no symlink
//!   resolution, no existence check. Callers that need those layer them
//!   on top.
//! - Tilde is only special at the very start. `foo~bar` is a literal.
//! - A `$` not followed by a variable name or `{...}` is a literal.
//!
//! ## Env-var isolation in tests
//!
//! Tests that manipulate process env vars MUST run serially — Rust's
//! default parallel test runner races if two tests touch the same vars.
//! Use the [`EnvGuard`] helper in the `tests` submodule to snapshot and
//! restore vars around each test body.

use std::path::PathBuf;

use thiserror::Error;

use os_shim::System;

/// Error returned by [`expand_path`] when input cannot be resolved.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExpandPathError {
    /// Syntax like `${UNCLOSED`, `${}`, or `%UNCLOSED` — the sigil started
    /// a variable reference but the reference was not terminated.
    #[error("invalid path syntax: {0}")]
    InvalidSyntax(String),

    /// An environment variable referenced by the path was not set. The
    /// wrapped string is the variable name (without sigils).
    #[error("environment variable `{0}` is not set")]
    UndefinedVariable(String),

    /// `~user/...` form is not supported — only `~` for the current user
    /// works. Users who want another user's home should write out the
    /// full path.
    #[error(
        "~{0} is not supported (only `~` for the current user works — write out the full path)"
    )]
    UnsupportedUserTilde(String),
}

/// Expand `~`, `$VAR`, `${VAR}`, and (on Windows) `%VAR%` in a path.
///
/// Returns the expanded path as a [`PathBuf`]. See the module-level docs
/// for full semantics.
///
/// Pass a [`System`] so mock filesystems can provide controlled env vars
/// in tests. The real filesystem's implementation reads from the process
/// environment.
///
/// # Errors
///
/// - [`ExpandPathError::UnsupportedUserTilde`] if the input starts with
///   `~user` (anything other than `~` alone or `~/`).
/// - [`ExpandPathError::UndefinedVariable`] if any referenced environment
///   variable is unset.
/// - [`ExpandPathError::InvalidSyntax`] for malformed variable references
///   like `${}` or an unclosed `${UNCLOSED`.
pub fn expand_path(system: &dyn System, input: &str) -> Result<PathBuf, ExpandPathError> {
    let raw = input;

    // Empty paths pass through unchanged.
    if raw.is_empty() {
        return Ok(PathBuf::new());
    }

    // Step 1: tilde at the absolute start.
    let after_tilde = expand_leading_tilde(system, raw)?;

    // Step 2: env-var substitution across the rest of the string.
    let expanded = expand_env_vars(system, &after_tilde)?;

    Ok(PathBuf::from(expanded))
}

/// Handle a leading `~` or `~/...`. Returns the input unchanged when the
/// tilde is not in the leading position.
fn expand_leading_tilde(system: &dyn System, raw: &str) -> Result<String, ExpandPathError> {
    if !raw.starts_with('~') {
        return Ok(raw.to_owned());
    }

    // `~` alone → $HOME.
    if raw.len() == 1 {
        let home = home_dir(system)?;
        return Ok(home);
    }

    // `~/...` or `~\\...` on Windows → $HOME + rest.
    let rest = &raw[1..];
    // Safe unwrap: `raw.len() > 1` above guarantees rest is non-empty.
    let Some(first) = rest.chars().next() else {
        let home = home_dir(system)?;
        return Ok(home);
    };

    if first == '/' || (cfg!(windows) && first == '\\') {
        let home = home_dir(system)?;
        return Ok(format!("{home}{rest}"));
    }

    // `~~`, `~user/...`, etc. — anything else after `~` is unsupported.
    // Strip up to the first separator (or end) so the error message names
    // the offending user token.
    let end_idx = rest
        .find(|c: char| c == '/' || (cfg!(windows) && c == '\\'))
        .unwrap_or(rest.len());
    let user = &rest[..end_idx];
    Err(ExpandPathError::UnsupportedUserTilde(user.to_owned()))
}

/// Resolve the platform home directory. POSIX uses `$HOME`; Windows prefers
/// `%USERPROFILE%` and falls back to `$HOME` for parity with the TypeScript
/// plugin's `expandPath`.
fn home_dir(system: &dyn System) -> Result<String, ExpandPathError> {
    #[cfg(windows)]
    {
        if let Ok(value) = system.env_var("USERPROFILE") {
            return Ok(value);
        }
    }
    system
        .env_var("HOME")
        .map_err(|_err| ExpandPathError::UndefinedVariable(String::from("HOME")))
}

/// Expand `$VAR`, `${VAR}`, and (on Windows) `%VAR%` references in `raw`.
///
/// POSIX `$` forms are recognized on every platform. Windows `%VAR%` is
/// only recognized when compiled for Windows — on POSIX a literal `%` is
/// preserved as-is (paths with `%` in them are legal on POSIX).
fn expand_env_vars(system: &dyn System, raw: &str) -> Result<String, ExpandPathError> {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        let ch = bytes[idx];

        if ch == b'$' {
            idx = consume_dollar(system, raw, bytes, idx, &mut out)?;
        } else {
            #[cfg(windows)]
            if ch == b'%' {
                idx = consume_percent(system, raw, bytes, idx, &mut out)?;
                continue;
            }
            out.push(ch as char);
            idx += 1;
        }
    }

    Ok(out)
}

/// Handle a `$` at byte index `idx`. Consumes the variable reference and
/// appends the expanded value (or the literal `$` if no name follows) to
/// `out`. Returns the new index.
fn consume_dollar(
    system: &dyn System,
    raw: &str,
    bytes: &[u8],
    idx: usize,
    out: &mut String,
) -> Result<usize, ExpandPathError> {
    // `$` at end of string → literal.
    let next_idx = idx + 1;
    if next_idx >= bytes.len() {
        out.push('$');
        return Ok(next_idx);
    }

    let next = bytes[next_idx];

    // `${...}` form.
    if next == b'{' {
        let body_start = next_idx + 1;
        let Some(rel_close) = bytes[body_start..].iter().position(|b| *b == b'}') else {
            return Err(ExpandPathError::InvalidSyntax(format!(
                "unclosed `${{` in {raw:?}"
            )));
        };
        let close_idx = body_start + rel_close;
        let name = &raw[body_start..close_idx];
        if name.is_empty() {
            return Err(ExpandPathError::InvalidSyntax(format!(
                "empty `${{}}` in {raw:?}"
            )));
        }
        let value = system
            .env_var(name)
            .map_err(|_err| ExpandPathError::UndefinedVariable(name.to_owned()))?;
        out.push_str(&value);
        return Ok(close_idx + 1);
    }

    // `$VAR` form (bare name).
    let name_end = bytes[next_idx..]
        .iter()
        .position(|b| !is_var_name_byte(*b))
        .map_or(bytes.len(), |rel| next_idx + rel);
    if name_end == next_idx {
        // `$` not followed by a var character → literal `$`.
        out.push('$');
        return Ok(next_idx);
    }
    let name = &raw[next_idx..name_end];
    let value = system
        .env_var(name)
        .map_err(|_err| ExpandPathError::UndefinedVariable(name.to_owned()))?;
    out.push_str(&value);
    Ok(name_end)
}

/// Handle a `%` at byte index `idx` on Windows. `%VAR%` expands; `%` with
/// no closing `%` is `InvalidSyntax`.
#[cfg(windows)]
fn consume_percent(
    system: &dyn System,
    raw: &str,
    bytes: &[u8],
    idx: usize,
    out: &mut String,
) -> Result<usize, ExpandPathError> {
    let body_start = idx + 1;
    let Some(rel_close) = bytes[body_start..].iter().position(|b| *b == b'%') else {
        return Err(ExpandPathError::InvalidSyntax(format!(
            "unclosed `%` in {raw:?}"
        )));
    };
    let close_idx = body_start + rel_close;
    let name = &raw[body_start..close_idx];
    if name.is_empty() {
        return Err(ExpandPathError::InvalidSyntax(format!(
            "empty `%%` in {raw:?}"
        )));
    }
    let value = system
        .env_var(name)
        .map_err(|_err| ExpandPathError::UndefinedVariable(name.to_owned()))?;
    out.push_str(&value);
    Ok(close_idx + 1)
}

/// Bytes allowed in a `$VAR` bare name: ASCII letters, digits, and `_`.
/// First byte must be a letter or `_`, but the consume loop enforces the
/// same rule via the "no-chars-consumed → literal `$`" branch.
const fn is_var_name_byte(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use os_shim::mock::MockSystem;

    use super::{ExpandPathError, expand_path};

    /// Helper: seed a mock system with a `HOME` env var and run expansion.
    /// Panics on test setup failure — this is test-only code and a HOME
    /// setter that cannot acquire its lock is a busted mock.
    fn make_system_with_home(home: &str) -> MockSystem {
        MockSystem::new().with_env("HOME", home).unwrap()
    }

    /// Helper: run expansion against a fresh mock with `HOME` set.
    fn expand_with_home(home: &str, input: &str) -> Result<PathBuf, ExpandPathError> {
        let system = make_system_with_home(home);
        expand_path(&system, input)
    }

    // --- Tilde expansion ------------------------------------------------

    #[test]
    fn tilde_alone_expands_to_home() {
        let result = expand_with_home("/home/alice", "~").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice"));
    }

    #[test]
    fn tilde_slash_path_expands_to_home_plus_rest() {
        let result = expand_with_home("/home/alice", "~/foo").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/foo"));
    }

    #[test]
    fn tilde_slash_nested_path_expands() {
        let result = expand_with_home("/home/alice", "~/foo/bar").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/foo/bar"));
    }

    #[test]
    fn tilde_slash_preserves_trailing_slash() {
        let result = expand_with_home("/home/alice", "~/").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/"));
    }

    #[test]
    fn tilde_user_returns_unsupported_error() {
        let result = expand_with_home("/home/alice", "~bob/foo");
        assert_eq!(
            result,
            Err(ExpandPathError::UnsupportedUserTilde(String::from("bob")))
        );
    }

    #[test]
    fn embedded_tilde_is_literal() {
        let result = expand_with_home("/home/alice", "foo~bar").unwrap();
        assert_eq!(result, PathBuf::from("foo~bar"));
    }

    #[test]
    fn mid_path_tilde_is_literal() {
        let result = expand_with_home("/home/alice", "./~/foo").unwrap();
        assert_eq!(result, PathBuf::from("./~/foo"));
    }

    #[test]
    fn double_tilde_is_unsupported() {
        let result = expand_with_home("/home/alice", "~~");
        assert_eq!(
            result,
            Err(ExpandPathError::UnsupportedUserTilde(String::from("~")))
        );
    }

    // --- POSIX env vars -------------------------------------------------

    #[test]
    fn dollar_var_alone_expands() {
        let result = expand_with_home("/home/alice", "$HOME").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice"));
    }

    #[test]
    fn dollar_var_slash_path_expands() {
        let result = expand_with_home("/home/alice", "$HOME/foo").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/foo"));
    }

    #[test]
    fn braced_var_slash_path_expands() {
        let result = expand_with_home("/home/alice", "${HOME}/foo").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/foo"));
    }

    #[test]
    fn braced_var_no_separator_concatenates() {
        let result = expand_with_home("/home/alice", "${HOME}foo").unwrap();
        assert_eq!(result, PathBuf::from("/home/alicefoo"));
    }

    #[test]
    fn two_vars_concatenate() {
        let result = expand_with_home("/home/alice", "$HOME$HOME").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/home/alice"));
    }

    #[test]
    fn undefined_bare_var_errors() {
        let system = MockSystem::new();
        let result = expand_path(&system, "$FOO_NOT_SET_9/bar");
        assert_eq!(
            result,
            Err(ExpandPathError::UndefinedVariable(String::from(
                "FOO_NOT_SET_9"
            )))
        );
    }

    #[test]
    fn undefined_braced_var_errors() {
        let system = MockSystem::new();
        let result = expand_path(&system, "${FOO_NOT_SET_9}/bar");
        assert_eq!(
            result,
            Err(ExpandPathError::UndefinedVariable(String::from(
                "FOO_NOT_SET_9"
            )))
        );
    }

    #[test]
    fn lone_dollar_is_literal() {
        let system = MockSystem::new();
        let result = expand_path(&system, "$").unwrap();
        assert_eq!(result, PathBuf::from("$"));
    }

    #[test]
    fn dollar_then_slash_is_literal() {
        let system = MockSystem::new();
        let result = expand_path(&system, "$/foo").unwrap();
        assert_eq!(result, PathBuf::from("$/foo"));
    }

    #[test]
    fn empty_braces_errors() {
        let system = MockSystem::new();
        let result = expand_path(&system, "${}");
        assert!(matches!(result, Err(ExpandPathError::InvalidSyntax(_))));
    }

    #[test]
    fn unclosed_braces_errors() {
        let system = MockSystem::new();
        let result = expand_path(&system, "${UNCLOSED");
        assert!(matches!(result, Err(ExpandPathError::InvalidSyntax(_))));
    }

    // --- Mixed tilde + env ----------------------------------------------

    #[test]
    fn tilde_plus_env_var_composes() {
        let system = MockSystem::new()
            .with_env("HOME", "/home/alice")
            .unwrap()
            .with_env("SUB", "baz")
            .unwrap();
        let result = expand_path(&system, "~/$SUB/bar").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/baz/bar"));
    }

    #[test]
    fn tilde_mid_path_after_env_is_literal() {
        let result = expand_with_home("/home/alice", "$HOME/~/foo").unwrap();
        assert_eq!(result, PathBuf::from("/home/alice/~/foo"));
    }

    // --- Absolute / relative passthrough --------------------------------

    #[test]
    fn absolute_path_passthrough() {
        let system = MockSystem::new();
        let result = expand_path(&system, "/absolute/path").unwrap();
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn relative_dot_path_passthrough() {
        let system = MockSystem::new();
        let result = expand_path(&system, "./relative/path").unwrap();
        assert_eq!(result, PathBuf::from("./relative/path"));
    }

    #[test]
    fn relative_dotdot_path_passthrough() {
        let system = MockSystem::new();
        let result = expand_path(&system, "../parent/path").unwrap();
        assert_eq!(result, PathBuf::from("../parent/path"));
    }

    #[test]
    fn bare_filename_passthrough() {
        let system = MockSystem::new();
        let result = expand_path(&system, "just-a-name.md").unwrap();
        assert_eq!(result, PathBuf::from("just-a-name.md"));
    }

    #[test]
    fn empty_string_passthrough() {
        let system = MockSystem::new();
        let result = expand_path(&system, "").unwrap();
        assert_eq!(result, PathBuf::new());
    }

    // --- Windows-specific -----------------------------------------------

    #[cfg(windows)]
    #[test]
    fn windows_userprofile_expands() {
        let system = MockSystem::new()
            .with_env("USERPROFILE", r"C:\Users\alice")
            .unwrap();
        let result = expand_path(&system, "%USERPROFILE%").unwrap();
        assert_eq!(result, PathBuf::from(r"C:\Users\alice"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_userprofile_with_path_preserves_backslash() {
        let system = MockSystem::new()
            .with_env("USERPROFILE", r"C:\Users\alice")
            .unwrap();
        let result = expand_path(&system, r"%USERPROFILE%\foo").unwrap();
        assert_eq!(result, PathBuf::from(r"C:\Users\alice\foo"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_undefined_percent_var_errors() {
        let system = MockSystem::new();
        let result = expand_path(&system, "%FOO_NOT_SET_9%");
        assert_eq!(
            result,
            Err(ExpandPathError::UndefinedVariable(String::from(
                "FOO_NOT_SET_9"
            )))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_unclosed_percent_errors() {
        let system = MockSystem::new();
        let result = expand_path(&system, "%UNCLOSED");
        assert!(matches!(result, Err(ExpandPathError::InvalidSyntax(_))));
    }

    #[cfg(windows)]
    #[test]
    fn windows_posix_dollar_home_also_works() {
        let system = MockSystem::new()
            .with_env("HOME", r"C:\Users\alice")
            .unwrap();
        let result = expand_path(&system, "$HOME").unwrap();
        assert_eq!(result, PathBuf::from(r"C:\Users\alice"));
    }

    // --- Adapter parity -------------------------------------------------

    /// CLI and MCP must agree on expansion for every input. Rather than
    /// standing up two call sites, we verify the core helper behaves
    /// consistently over a table of representative inputs.
    #[test]
    fn adapter_parity_table() {
        let system = MockSystem::new()
            .with_env("HOME", "/home/alice")
            .unwrap()
            .with_env("FOO", "bar")
            .unwrap();

        let cases: &[(&str, &str)] = &[
            ("~", "/home/alice"),
            ("~/notes", "/home/alice/notes"),
            ("$HOME/notes", "/home/alice/notes"),
            ("${HOME}/notes", "/home/alice/notes"),
            ("$FOO", "bar"),
            ("${FOO}/baz", "bar/baz"),
            ("/abs", "/abs"),
            ("./rel", "./rel"),
            ("file.md", "file.md"),
            ("", ""),
        ];
        for (input, want) in cases {
            let got = expand_path(&system, input).unwrap();
            assert_eq!(got, PathBuf::from(*want), "input: {input:?}");
        }
    }
}
