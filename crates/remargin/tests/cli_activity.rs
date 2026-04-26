//! `remargin activity` CLI + MCP integration tests (rem-g3sy.4 /
//! T34).

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::Path;
    use std::process::Output;

    use assert_cmd::Command;
    use os_shim::System as _;
    use os_shim::real::RealSystem;
    use remargin_core::config::ResolvedConfig;
    use remargin_core::config::identity::IdentityFlags;
    use remargin_core::config::parse_author_type;
    use remargin_core::mcp;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    fn realm_with(files: &[(&str, &str)]) -> TempDir {
        let realm = TempDir::new().unwrap();
        fs::write(
            realm.path().join(".remargin.yaml"),
            "identity: alice\ntype: human\n",
        )
        .unwrap();
        for (rel, body) in files {
            let path = realm.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
        }
        realm
    }

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn assert_status(out: &Output, expected: i32) {
        let actual = out.status.code();
        assert_eq!(
            actual,
            Some(expected),
            "remargin exited with {:?}\nstdout: {}\nstderr: {}",
            actual,
            str::from_utf8(&out.stdout).unwrap(),
            str::from_utf8(&out.stderr).unwrap(),
        );
    }

    fn doc(id: &str, author: &str, ts: &str) -> String {
        format!(
            "---\ntitle: t\n---\n\n# Body\n\n```remargin\n---\nid: {id}\nauthor: {author}\ntype: human\nts: {ts}\nchecksum: sha256:t\n---\nBody.\n```\n"
        )
    }

    /// JSON output is the default: `remargin activity` returns
    /// the structured `ActivityResult` as pretty-printed JSON on
    /// stdout.
    #[test]
    fn json_output_is_default() {
        let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
        let out = run_in(
            realm.path(),
            &["activity", "--identity", "alice", "--type", "human"],
        );
        assert_status(&out, 0);
        let stdout = str::from_utf8(&out.stdout).unwrap();
        let value: Value = serde_json::from_str(stdout).unwrap();
        let files = value["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        let changes = files[0]["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["kind"], json!("comment"));
    }

    /// `--pretty` switches to the human-readable timeline; output
    /// goes to stderr so stdout stays clean for CLI piping.
    #[test]
    fn pretty_output_renders_timeline() {
        let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
        let out = run_in(
            realm.path(),
            &[
                "activity",
                "--pretty",
                "--identity",
                "alice",
                "--type",
                "human",
            ],
        );
        assert_status(&out, 0);
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(stderr.contains("comment"), "{stderr}");
        assert!(stderr.contains("c1 by bob"), "{stderr}");
    }

    /// `--since` parses ISO 8601 and applies as an explicit
    /// cutoff. A comment before the cutoff is dropped.
    #[test]
    fn since_cutoff_filters_comments() {
        let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
        let out = run_in(
            realm.path(),
            &[
                "activity",
                "--since",
                "2026-04-06T13:00:00-04:00",
                "--identity",
                "alice",
                "--type",
                "human",
            ],
        );
        assert_status(&out, 0);
        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert!(value["files"].as_array().unwrap().is_empty());
    }

    /// `--since` with malformed input errors with a clear
    /// message.
    #[test]
    fn malformed_since_errors() {
        let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
        let out = run_in(
            realm.path(),
            &[
                "activity",
                "--since",
                "not-a-date",
                "--identity",
                "alice",
                "--type",
                "human",
            ],
        );
        assert_ne!(out.status.code(), Some(0_i32));
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(stderr.contains("--since"), "{stderr}");
    }

    /// `--pretty` and `--json` together is rejected.
    #[test]
    fn pretty_and_json_are_mutually_exclusive() {
        let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
        let out = run_in(
            realm.path(),
            &[
                "activity",
                "--pretty",
                "--json",
                "--identity",
                "alice",
                "--type",
                "human",
            ],
        );
        assert_ne!(out.status.code(), Some(0_i32));
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(stderr.contains("mutually exclusive"), "{stderr}");
    }

    /// MCP parity: `mcp__remargin__activity` returns a payload
    /// with the same structural shape as the CLI `--json` output.
    #[test]
    fn mcp_activity_matches_cli_shape() {
        let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
        let cli = run_in(
            realm.path(),
            &["activity", "--identity", "alice", "--type", "human"],
        );
        assert_status(&cli, 0);
        let cli_payload: Value =
            serde_json::from_str(str::from_utf8(&cli.stdout).unwrap()).unwrap();

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let mut flags = IdentityFlags::default();
        flags.identity = Some(String::from("alice"));
        flags.author_type = Some(parse_author_type("human").unwrap());
        let config = ResolvedConfig::resolve(&system, &base, &flags, None).unwrap();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "activity",
                "arguments": {}
            }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        let response_str = mcp::process_request(&system, &base, &config, &request_str)
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&response_str).unwrap();
        let result = response.get("result").unwrap();
        let content = result.get("content").and_then(Value::as_array).unwrap();
        let text = content[0].get("text").and_then(Value::as_str).unwrap();
        let mcp_payload: Value = serde_json::from_str(text).unwrap();

        let cli_files = cli_payload["files"].as_array().unwrap();
        let mcp_files = mcp_payload["files"].as_array().unwrap();
        assert_eq!(cli_files.len(), mcp_files.len());
        // Both should report exactly one file with one comment change.
        assert_eq!(cli_files.len(), 1);
        let cli_changes = cli_files[0]["changes"].as_array().unwrap();
        let mcp_changes = mcp_files[0]["changes"].as_array().unwrap();
        assert_eq!(cli_changes.len(), mcp_changes.len());
        assert_eq!(cli_changes[0]["kind"], mcp_changes[0]["kind"]);
        assert_eq!(cli_changes[0]["comment_id"], mcp_changes[0]["comment_id"]);
    }
}
