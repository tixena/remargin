//! End-to-end activity integration tests (rem-g3sy.5 / T35).
//!
//! Exercises the full stack — sandbox-add timestamp refresh
//! (rem-g3sy.1), edit-stamps-`edited_at` (rem-g3sy.2),
//! `gather_activity` (rem-g3sy.3), CLI / MCP wiring (rem-g3sy.4) —
//! against real-filesystem temp dirs.

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

    fn realm() -> TempDir {
        let realm = TempDir::new().unwrap();
        fs::write(
            realm.path().join(".remargin.yaml"),
            "identity: alice\ntype: human\nmode: open\n",
        )
        .unwrap();
        realm
    }

    fn write_md(realm: &TempDir, rel: &str, body: &str) {
        let path = realm.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
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

    fn doc_with_one_comment(id: &str, author: &str, ts: &str) -> String {
        format!(
            "---\ntitle: t\n---\n\n# Body\n\n```remargin\n---\nid: {id}\nauthor: {author}\ntype: human\nts: {ts}\nchecksum: sha256:t\n---\nBody.\n```\n"
        )
    }

    /// E1: initial-touch fallback returns everything for a caller
    /// who has never acted in the file.
    #[test]
    fn initial_touch_fallback() {
        let realm = realm();
        write_md(
            &realm,
            "note.md",
            &doc_with_one_comment("c1", "bob", "2026-04-06T12:00:00-04:00"),
        );
        let out = run_in(
            realm.path(),
            &["activity", "--identity", "alice", "--type", "human"],
        );
        assert_status(&out, 0);
        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["files"].as_array().unwrap().len(), 1);
    }

    /// E3: --since explicit cutoff filters across all files.
    #[test]
    fn since_explicit_cutoff_filters_globally() {
        let realm = realm();
        write_md(
            &realm,
            "a.md",
            &doc_with_one_comment("c1", "bob", "2026-04-06T12:00:00-04:00"),
        );
        write_md(
            &realm,
            "b.md",
            &doc_with_one_comment("c2", "bob", "2026-04-07T12:00:00-04:00"),
        );
        let out = run_in(
            realm.path(),
            &[
                "activity",
                "--since",
                "2026-04-06T18:00:00-04:00",
                "--identity",
                "alice",
                "--type",
                "human",
            ],
        );
        assert_status(&out, 0);
        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        let files = value["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0]["path"].as_str().unwrap().ends_with("b.md"));
    }

    /// E11: empty result. With no managed `.md` files in the realm,
    /// `files` is empty and `newest_ts_overall` is null.
    #[test]
    fn empty_result_when_no_changes() {
        let realm = realm();
        let out = run_in(
            realm.path(),
            &["activity", "--identity", "alice", "--type", "human"],
        );
        assert_status(&out, 0);
        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert!(value["files"].as_array().unwrap().is_empty());
        assert!(value["newest_ts_overall"].is_null());
    }

    /// E12: a path outside any realm errors with a clear message
    /// and a non-zero exit code.
    #[test]
    fn path_outside_realm_errors() {
        let outsider = TempDir::new().unwrap();
        write_md(
            &outsider,
            "note.md",
            &doc_with_one_comment("c1", "bob", "2026-04-06T12:00:00-04:00"),
        );
        // No .remargin.yaml in the tempdir: the realm walk fails.
        let out = run_in(
            outsider.path(),
            &["activity", "--identity", "alice", "--type", "human"],
        );
        assert_ne!(out.status.code(), Some(0_i32));
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("outside any .remargin.yaml"),
            "expected outside-realm error, got: {stderr}"
        );
    }

    /// E13: --identity drives the caller. With carol declared,
    /// the per-file last-action cutoff uses carol's activity (and
    /// since carol has none, the initial-touch fallback returns
    /// everything).
    #[test]
    fn identity_flag_drives_caller() {
        let realm = realm();
        write_md(
            &realm,
            "note.md",
            &doc_with_one_comment("c1", "bob", "2026-04-06T12:00:00-04:00"),
        );
        let out = run_in(
            realm.path(),
            &["activity", "--identity", "carol", "--type", "human"],
        );
        assert_status(&out, 0);
        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["files"].as_array().unwrap().len(), 1);
    }

    /// E10: MCP / CLI parity. Same realm + identity through both
    /// surfaces produces structurally identical JSON.
    #[test]
    fn mcp_and_cli_match() {
        let realm = realm();
        write_md(
            &realm,
            "note.md",
            &doc_with_one_comment("c1", "bob", "2026-04-06T12:00:00-04:00"),
        );

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

        // Compare the structural shape: file count, change count,
        // and the comment_id of the first change.
        let cli_files = cli_payload["files"].as_array().unwrap();
        let mcp_files = mcp_payload["files"].as_array().unwrap();
        assert_eq!(cli_files.len(), mcp_files.len());
        assert_eq!(cli_files.len(), 1);
        let cli_changes = cli_files[0]["changes"].as_array().unwrap();
        let mcp_changes = mcp_files[0]["changes"].as_array().unwrap();
        assert_eq!(cli_changes.len(), mcp_changes.len());
        assert_eq!(cli_changes[0]["comment_id"], mcp_changes[0]["comment_id"]);
    }
}
