//! Tests for path expansion: tilde, environment variables, passthrough and the error cases.

use std::path::PathBuf;

use os_shim::mock::MemorySystem;

use super::{ExpandPathError, expand_path};

/// A mock system with `HOME` set.
fn make_system_with_home(home: &str) -> MemorySystem {
    MemorySystem::new().with_env("HOME", home).unwrap()
}

fn expand_with_home(home: &str, input: &str) -> Result<PathBuf, ExpandPathError> {
    let system = make_system_with_home(home);
    expand_path(&system, input)
}

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
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
    let result = expand_path(&system, "$").unwrap();
    assert_eq!(result, PathBuf::from("$"));
}

#[test]
fn dollar_then_slash_is_literal() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "$/foo").unwrap();
    assert_eq!(result, PathBuf::from("$/foo"));
}

#[test]
fn empty_braces_errors() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "${}");
    assert!(matches!(result, Err(ExpandPathError::InvalidSyntax(_))));
}

#[test]
fn unclosed_braces_errors() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "${UNCLOSED");
    assert!(matches!(result, Err(ExpandPathError::InvalidSyntax(_))));
}

#[test]
fn tilde_plus_env_var_composes() {
    let system = MemorySystem::new()
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

#[test]
fn absolute_path_passthrough() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "/absolute/path").unwrap();
    assert_eq!(result, PathBuf::from("/absolute/path"));
}

#[test]
fn relative_dot_path_passthrough() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "./relative/path").unwrap();
    assert_eq!(result, PathBuf::from("./relative/path"));
}

#[test]
fn relative_dotdot_path_passthrough() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "../parent/path").unwrap();
    assert_eq!(result, PathBuf::from("../parent/path"));
}

#[test]
fn bare_filename_passthrough() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "just-a-name.md").unwrap();
    assert_eq!(result, PathBuf::from("just-a-name.md"));
}

#[test]
fn empty_string_passthrough() {
    let system = MemorySystem::new();
    let result = expand_path(&system, "").unwrap();
    assert_eq!(result, PathBuf::new());
}

#[cfg(windows)]
#[test]
fn windows_userprofile_expands() {
    let system = MemorySystem::new()
        .with_env("USERPROFILE", r"C:\Users\alice")
        .unwrap();
    let result = expand_path(&system, "%USERPROFILE%").unwrap();
    assert_eq!(result, PathBuf::from(r"C:\Users\alice"));
}

#[cfg(windows)]
#[test]
fn windows_userprofile_with_path_preserves_backslash() {
    let system = MemorySystem::new()
        .with_env("USERPROFILE", r"C:\Users\alice")
        .unwrap();
    let result = expand_path(&system, r"%USERPROFILE%\foo").unwrap();
    assert_eq!(result, PathBuf::from(r"C:\Users\alice\foo"));
}

#[cfg(windows)]
#[test]
fn windows_undefined_percent_var_errors() {
    let system = MemorySystem::new();
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
    let system = MemorySystem::new();
    let result = expand_path(&system, "%UNCLOSED");
    assert!(matches!(result, Err(ExpandPathError::InvalidSyntax(_))));
}

#[cfg(windows)]
#[test]
fn windows_posix_dollar_home_also_works() {
    let system = MemorySystem::new()
        .with_env("HOME", r"C:\Users\alice")
        .unwrap();
    let result = expand_path(&system, "$HOME").unwrap();
    assert_eq!(result, PathBuf::from(r"C:\Users\alice"));
}

/// The CLI and MCP share this helper, so one table of inputs covers both.
#[test]
fn adapter_parity_table() {
    let system = MemorySystem::new()
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
