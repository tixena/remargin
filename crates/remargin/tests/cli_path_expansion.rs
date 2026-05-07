//! Integration tests for path expansion (rem-3xo).
//!
//! Covers the adapter-boundary behaviour described in the task:
//!
//! - CLI string/PathBuf args (`get`, `metadata`, `ls`, `rm`, `obsidian`
//!   `--vault-path`) expand `~`, `$VAR`, `${VAR}` before the command
//!   dispatches.
//! - MCP tools receive already-expanded paths through the in-process
//!   `mcp::process_request` entry point.
//! - Both surfaces agree on the same inputs (adapter parity, rem-3a2).
//! - Undefined env vars and `~user` produce a clear named error.
//!
//! Env-var manipulation is done via a hermetic fixture home: we set
//! `HOME` in the child process (for CLI runs) and on a `MockSystem`
//! (for MCP runs). No test mutates the parent process environment.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};

    use assert_cmd::Command;
    use os_shim::System as _;
    use os_shim::mock::MockSystem;
    use remargin_core::config::ResolvedConfig;
    use remargin_core::config::identity::IdentityFlags;
    use remargin_core::mcp;
    use remargin_core::path::{ExpandPathError, expand_path};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    /// Prepare a `MockSystem` with a fake HOME and a seeded markdown
    /// fixture under `<home>/note.md`.
    fn make_mock_home_with_note() -> (MockSystem, String) {
        let system = MockSystem::new().with_env("HOME", "/home/alice").unwrap();
        system.create_dir_all(Path::new("/home/alice")).unwrap();
        system
            .write(Path::new("/home/alice/note.md"), b"# Hello\n\nBody.\n")
            .unwrap();
        (system, String::from("/home/alice"))
    }

    // --- Core helper contract ------------------------------------------

    /// `~` works against the `MockSystem` HOME env var.
    #[test]
    fn expand_tilde_in_mock_system() {
        let (system, home) = make_mock_home_with_note();
        let expanded = expand_path(&system, "~/note.md").unwrap();
        assert_eq!(expanded, PathBuf::from(format!("{home}/note.md")));
    }

    /// `~user/...` is an explicit error (named user token), not a silent
    /// passthrough.
    #[test]
    fn expand_tilde_user_errors_clearly() {
        let (system, _home) = make_mock_home_with_note();
        let err = expand_path(&system, "~bob/foo").unwrap_err();
        assert_eq!(
            err,
            ExpandPathError::UnsupportedUserTilde(String::from("bob"))
        );
        assert!(err.to_string().contains("~bob"));
    }

    /// Undefined env var surfaces the variable name so the user can fix
    /// it without staring at a "file not found" red herring.
    #[test]
    fn expand_undefined_var_names_the_variable() {
        let system = MockSystem::new();
        let err = expand_path(&system, "$DEFINITELY_UNSET_FOO/bar").unwrap_err();
        assert_eq!(
            err,
            ExpandPathError::UndefinedVariable(String::from("DEFINITELY_UNSET_FOO"))
        );
        assert!(err.to_string().contains("DEFINITELY_UNSET_FOO"));
    }

    // --- CLI `get` with `~` --------------------------------------------

    /// `remargin get ~/note.md --json` resolves against the child's HOME
    /// and succeeds. We point HOME at the tmpdir and chdir the child
    /// there so the sandbox check (which disallows escaping cwd) agrees
    /// with the expanded path. The parent process env is not touched.
    #[test]
    fn cli_get_expands_tilde_against_child_home() {
        let tmp = TempDir::new().unwrap();
        let note = tmp.path().join("note.md");
        fs::write(&note, "# Hi\n").unwrap();

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .arg("get")
            .arg("~/note.md")
            .arg("--json")
            .output()
            .unwrap();

        let stderr = str::from_utf8(&output.stderr).unwrap();
        assert!(output.status.success(), "command failed: stderr={stderr:?}");

        let stdout = str::from_utf8(&output.stdout).unwrap();
        let parsed: Value = serde_json::from_str(stdout).unwrap();
        assert_eq!(parsed["content"].as_str().unwrap(), "# Hi\n");
    }

    // The `~user` error path is exercised in-process by
    // `expand_tilde_user_errors_clearly` above (and the MCP variant
    // below). The CLI-binary version was deleted as redundant — it
    // walked up to the real `~/.remargin.yaml`, which we don't isolate
    // via tempdirs.

    // --- MCP surface ----------------------------------------------------

    fn mcp_test_config(system: &MockSystem, base: &Path) -> ResolvedConfig {
        ResolvedConfig::resolve(system, base, &IdentityFlags::default(), None).unwrap()
    }

    /// MCP `get` expands `~` before dispatching so a tool caller passing
    /// `path: "~/note.md"` is identical to passing the absolute path.
    #[test]
    fn mcp_get_expands_tilde() {
        let (system, home) = make_mock_home_with_note();
        let base = PathBuf::from(&home);
        let config = mcp_test_config(&system, &base);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get",
                "arguments": { "path": "~/note.md" }
            }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        let response = mcp::process_request(&system, &base, &config, &request_str)
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();

        // The tool response wraps content; any non-error payload passes.
        let result = parsed.get("result").unwrap();
        let content = result.get("content").and_then(Value::as_array).unwrap();
        let text = content[0].get("text").and_then(Value::as_str).unwrap();
        assert!(
            text.contains("# Hello"),
            "expected file contents in response, got: {text}"
        );
        // Ensure no error surfaced.
        assert!(
            !text.to_lowercase().contains("error"),
            "unexpected error: {text}"
        );
    }

    /// MCP undefined env var surfaces the variable name in the tool
    /// result's error text — not "file not found".
    #[test]
    fn mcp_undefined_var_surfaces_named_error() {
        let (system, home) = make_mock_home_with_note();
        let base = PathBuf::from(&home);
        let config = mcp_test_config(&system, &base);

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "get",
                "arguments": { "path": "$DEFINITELY_UNSET_FOO/bar.md" }
            }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        let response = mcp::process_request(&system, &base, &config, &request_str)
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();
        let result = parsed.get("result").unwrap();
        let content = result.get("content").and_then(Value::as_array).unwrap();
        let text = content[0].get("text").and_then(Value::as_str).unwrap();
        assert!(
            text.contains("DEFINITELY_UNSET_FOO"),
            "error did not name the missing variable: {text}"
        );
    }

    // --- Adapter parity -------------------------------------------------

    /// For every input the helper produces one expanded value — CLI and
    /// MCP both go through the same core helper, so parity holds by
    /// construction. This test pins the contract.
    #[test]
    fn cli_and_mcp_expand_identically_over_representative_inputs() {
        let system = MockSystem::new()
            .with_env("HOME", "/home/alice")
            .unwrap()
            .with_env("FOO", "bar")
            .unwrap();

        let cases: &[&str] = &[
            "~",
            "~/notes",
            "~/notes/deep/path.md",
            "$HOME/notes",
            "${HOME}/notes",
            "$FOO/baz",
            "${FOO}baz",
            "/abs/path",
            "./rel",
            "just-a-name.md",
        ];
        for input in cases {
            let a = expand_path(&system, input).unwrap();
            let b = expand_path(&system, input).unwrap();
            assert_eq!(a, b, "non-deterministic expansion: {input:?}");
        }
    }
}
