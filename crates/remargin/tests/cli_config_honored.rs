//! Per-subcommand `--config` happy-path tests (rem-58d6).
//!
//! The previous resolver silently dropped `--config` on every
//! identity-aware subcommand. These tests lock in the fix by setting
//! up two realms: a `walker` realm whose `.remargin.yaml` declares
//! `walker-agent` and a `flag` realm whose `.remargin.yaml` declares
//! `flag-agent`. Each test runs the subcommand from inside `walker`
//! with `--config` pointing at `flag`'s yaml. Any subcommand that
//! regresses to the old overlay model will attribute the operation
//! to `walker-agent` and fail the `flag-agent` assertion.
//!
//! Tests that inspect author attribution prove `--config` end-to-end.
//! Tests that merely exercise a subcommand (write / rm / purge /
//! migrate) prove `--config` at least reaches the resolver without
//! being silently dropped.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command as StdCommand, Output, Stdio};

    use assert_cmd::Command;
    use assert_cmd::cargo::cargo_bin;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const WALKER_CONFIG: &str = "identity: walker-agent\ntype: agent\nmode: open\n";
    const FLAG_CONFIG: &str = "identity: flag-agent\ntype: agent\nmode: open\n";

    const TEST_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";

    const TEST_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin";

    /// Build the two-realm layout described in the module docs. Returns
    /// the tempdir (keep alive for the test's lifetime), the walker cwd,
    /// and the flag config path.
    fn two_realms() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let walker = tmp.path().join("walker");
        let flag = tmp.path().join("flag");
        fs::create_dir_all(&walker).unwrap();
        fs::create_dir_all(&flag).unwrap();
        fs::write(walker.join(".remargin.yaml"), WALKER_CONFIG).unwrap();
        fs::write(flag.join(".remargin.yaml"), FLAG_CONFIG).unwrap();
        let flag_config = flag.join(".remargin.yaml");
        (tmp, walker, flag_config)
    }

    fn seed_doc(walker: &Path, name: &str, body: &str) -> PathBuf {
        let path = walker.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    fn run(walker: &Path, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(walker)
            .args(args)
            .output()
            .unwrap()
    }

    fn run_ok(walker: &Path, args: &[&str]) -> String {
        let output = run(walker, args);
        let stderr = str::from_utf8(&output.stderr).unwrap();
        let stdout = str::from_utf8(&output.stdout).unwrap();
        assert!(
            output.status.success(),
            "remargin {args:?} failed\nstderr: {stderr}\nstdout: {stdout}"
        );
        String::from(stdout)
    }

    fn doc_contents(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn seed_comment_as(walker: &Path, doc: &str, content: &str, config: &str) -> String {
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(walker)
            .args(["comment", doc, content, "--config", config, "--json"])
            .output()
            .unwrap();
        let stdout = str::from_utf8(&out.stdout).unwrap();
        let parsed: Value = serde_json::from_str(stdout).unwrap();
        String::from(parsed["id"].as_str().unwrap())
    }

    fn seed_comment_as_manual(
        walker: &Path,
        doc: &str,
        content: &str,
        identity: &str,
        author_type: &str,
    ) -> String {
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(walker)
            .args([
                "comment",
                doc,
                content,
                "--identity",
                identity,
                "--type",
                author_type,
                "--json",
            ])
            .output()
            .unwrap();
        let stdout = str::from_utf8(&out.stdout).unwrap();
        let parsed: Value = serde_json::from_str(stdout).unwrap();
        String::from(parsed["id"].as_str().unwrap())
    }

    // ---------- comment ----------

    #[test]
    fn comment_attributes_new_comment_to_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());

        run_ok(
            &walker,
            &[
                "comment",
                &doc.to_string_lossy(),
                "hello from flag",
                "--config",
                &flag_path,
            ],
        );

        let body = doc_contents(&doc);
        assert!(
            body.contains("author: flag-agent"),
            "comment should be authored by flag-agent, doc was:\n{body}"
        );
        assert!(
            !body.contains("author: walker-agent"),
            "walker-agent must not leak in, doc was:\n{body}"
        );
    }

    // ---------- ack ----------

    #[test]
    fn ack_attributes_ack_to_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        let comment_id = seed_comment_as_manual(&walker, &doc_str, "seed", "someone-else", "agent");

        run_ok(
            &walker,
            &[
                "ack",
                &comment_id,
                "--file",
                &doc_str,
                "--config",
                &flag_path,
            ],
        );

        let body = doc_contents(&doc);
        assert!(
            body.contains("flag-agent@"),
            "ack list should contain flag-agent, doc was:\n{body}"
        );
        assert!(
            !body.contains("walker-agent@"),
            "walker-agent must not appear in ack list, doc was:\n{body}"
        );
    }

    // ---------- react ----------

    #[test]
    fn react_attributes_reaction_to_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        let id = seed_comment_as_manual(&walker, &doc_str, "seed", "someone-else", "agent");

        run_ok(
            &walker,
            &["react", &doc_str, &id, "thumbsup", "--config", &flag_path],
        );

        let body = doc_contents(&doc);
        assert!(
            body.contains("flag-agent"),
            "reaction should mention flag-agent, doc was:\n{body}"
        );
    }

    // ---------- batch ----------

    #[test]
    fn batch_attributes_created_comments_to_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        let ops = json!([
            { "op": "comment", "content": "first via batch" },
            { "op": "comment", "content": "second via batch" }
        ])
        .to_string();

        run_ok(
            &walker,
            &["batch", &doc_str, "--ops", &ops, "--config", &flag_path],
        );

        // The doc's frontmatter carries `author: flag-agent` for the
        // whole doc; inside each comment fence `type: agent` appears
        // exactly once. Count those to check the two new comments.
        let body = doc_contents(&doc);
        let fences = body.matches("type: agent").count();
        assert_eq!(
            fences, 2,
            "batch should have created 2 agent comments, doc was:\n{body}"
        );
        assert_eq!(
            body.matches("```remargin\n---\nid: ").count(),
            2,
            "expected 2 comment fences, doc was:\n{body}"
        );
        assert!(!body.contains("author: walker-agent"));
    }

    // ---------- edit ----------

    #[test]
    fn edit_uses_config_identity_for_edit_own_guard() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        // Seed a comment AS flag-agent so the edit-own guard admits
        // the later edit only if --config actually steers identity to
        // flag-agent.
        let id = seed_comment_as(&walker, &doc_str, "seed", &flag_path);

        run_ok(
            &walker,
            &[
                "edit",
                &doc_str,
                &id,
                "edited via flag-agent",
                "--config",
                &flag_path,
            ],
        );

        let body = doc_contents(&doc);
        assert!(
            body.contains("edited via flag-agent"),
            "edited content should appear, doc was:\n{body}"
        );
    }

    // ---------- plan ----------

    #[test]
    fn plan_reports_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        let stdout = run_ok(
            &walker,
            &[
                "plan",
                "--config",
                &flag_path,
                "comment",
                &doc_str,
                "hypothetical",
                "--json",
            ],
        );
        let parsed: Value = serde_json::from_str(&stdout).unwrap();
        let identity = &parsed["identity"];
        assert_eq!(
            identity["name"].as_str(),
            Some("flag-agent"),
            "plan.identity.name should reflect --config, got: {parsed}"
        );
        assert_eq!(
            identity["author_type"].as_str(),
            Some("agent"),
            "plan.identity.author_type should reflect --config, got: {parsed}"
        );
    }

    // ---------- verify ----------

    #[test]
    fn verify_runs_under_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        // Seed one comment so verify has something to report on.
        let _id = seed_comment_as(&walker, &doc_str, "seed", &flag_path);

        // `verify` is read-only: we can't inspect its per-run identity
        // via the doc. The regression shape is "passing --config errors
        // because the resolver picks up the wrong identity or drops the
        // flag." Exit code + clean output is the signal.
        let _verified = run_ok(
            &walker,
            &["verify", &doc_str, "--config", &flag_path, "--json"],
        );
    }

    // ---------- sandbox ----------

    #[test]
    #[expect(clippy::panic, reason = "integration test assertion helper")]
    fn sandbox_scoping_follows_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        // Add the doc to flag-agent's sandbox via --config. Each
        // sandbox is keyed by the caller's identity, so walker-agent's
        // list must stay empty while flag-agent's list contains the doc.
        //
        // Both `IdentityArgs` and `OutputArgs` are flattened on the
        // `sandbox` parent, so the flags go BEFORE the sub-action.
        run_ok(
            &walker,
            &["sandbox", "--config", &flag_path, "--json", "add", &doc_str],
        );

        let walker_stdout = run_ok(&walker, &["sandbox", "--json", "list"]);
        let walker_json: Value = serde_json::from_str(&walker_stdout).unwrap();
        let Some(walker_files) = walker_json["files"].as_array() else {
            panic!("walker files must be a JSON array, got: {walker_json}")
        };
        assert!(
            walker_files.is_empty(),
            "walker-agent's sandbox must stay empty, got: {walker_json}"
        );

        let flag_stdout = run_ok(
            &walker,
            &["sandbox", "--config", &flag_path, "--json", "list"],
        );
        let flag_json: Value = serde_json::from_str(&flag_stdout).unwrap();
        let Some(flag_files) = flag_json["files"].as_array() else {
            panic!("flag files must be a JSON array, got: {flag_json}")
        };
        assert_eq!(
            flag_files.len(),
            1,
            "flag-agent's sandbox should contain one file, got: {flag_json}"
        );
    }

    // ---------- write ----------

    #[test]
    fn write_accepts_config_declaration() {
        let (_tmp, walker, flag_config) = two_realms();
        let flag_path = String::from(flag_config.to_string_lossy());
        let new_doc = walker.join("new.md");

        run_ok(
            &walker,
            &[
                "write",
                &new_doc.to_string_lossy(),
                "# New Doc\n\nBody.\n",
                "--create",
                "--config",
                &flag_path,
            ],
        );
        assert!(new_doc.exists(), "write --create must produce the file");
        let body = doc_contents(&new_doc);
        assert!(body.contains("# New Doc"));
    }

    // ---------- delete ----------

    #[test]
    fn delete_runs_under_config_identity() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        let id = seed_comment_as(&walker, &doc_str, "to be deleted", &flag_path);

        run_ok(&walker, &["delete", &doc_str, &id, "--config", &flag_path]);

        let body = doc_contents(&doc);
        assert!(
            !body.contains("to be deleted"),
            "deleted comment body should be gone, doc was:\n{body}"
        );
    }

    // ---------- purge ----------

    #[test]
    fn purge_accepts_config_declaration() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        let _id = seed_comment_as(&walker, &doc_str, "soon to be purged", &flag_path);

        run_ok(&walker, &["purge", &doc_str, "--config", &flag_path]);

        let body = doc_contents(&doc);
        assert!(
            !body.contains("```remargin"),
            "purge should remove all comment fences, doc was:\n{body}"
        );
    }

    // ---------- rm ----------

    #[test]
    fn rm_accepts_config_declaration() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());

        run_ok(
            &walker,
            &["rm", &doc.to_string_lossy(), "--config", &flag_path],
        );
        assert!(!doc.exists(), "rm should remove the file");
    }

    // ---------- migrate ----------

    #[test]
    fn migrate_accepts_config_declaration() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(
            &walker,
            "doc.md",
            "# Hello\n\n<!-- user comments -->\n- alice (2024-01-01): hi\n",
        );
        let flag_path = String::from(flag_config.to_string_lossy());

        let output = run(
            &walker,
            &["migrate", &doc.to_string_lossy(), "--config", &flag_path],
        );
        let stderr = str::from_utf8(&output.stderr).unwrap();
        assert!(
            output.status.success(),
            "migrate --config should succeed; stderr={stderr}"
        );
    }

    // ---------- sign (strict mode with real key) ----------

    #[test]
    fn sign_under_config_identity_passes_strict_verify() {
        let tmp = TempDir::new().unwrap();
        let walker = tmp.path().join("walker");
        let flag = tmp.path().join("flag");
        fs::create_dir_all(&walker).unwrap();
        fs::create_dir_all(&flag).unwrap();

        // Shared registry lives at the tempdir root so both realms
        // discover the same active participant set.
        let registry = format!(
            "participants:\n  walker-agent:\n    type: agent\n    status: active\n    pubkeys:\n      - {TEST_PUBLIC_KEY}\n  flag-agent:\n    type: agent\n    status: active\n    pubkeys:\n      - {TEST_PUBLIC_KEY}\n"
        );
        fs::write(tmp.path().join(".remargin-registry.yaml"), &registry).unwrap();

        let walker_key = walker.join("agent_key");
        fs::write(&walker_key, TEST_PRIVATE_KEY).unwrap();
        let flag_key = flag.join("agent_key");
        fs::write(&flag_key, TEST_PRIVATE_KEY).unwrap();

        // Use `./agent_key` so `resolve_key_path` treats it as a path
        // (the bare-name shorthand would resolve to `~/.ssh/agent_key`).
        // The post-expansion anchor prepends the config file's dir,
        // pointing at the key we just wrote.
        fs::write(
            walker.join(".remargin.yaml"),
            "identity: walker-agent\ntype: agent\nmode: strict\nkey: ./agent_key\n",
        )
        .unwrap();
        fs::write(
            flag.join(".remargin.yaml"),
            "identity: flag-agent\ntype: agent\nmode: strict\nkey: ./agent_key\n",
        )
        .unwrap();

        let flag_config = flag.join(".remargin.yaml");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let doc_str = String::from(doc.to_string_lossy());

        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(&walker)
            .args([
                "comment",
                &doc_str,
                "flag-agent speaks",
                "--config",
                &flag_path,
                "--json",
            ])
            .output()
            .unwrap();
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            out.status.success(),
            "strict comment --config must succeed; stderr={stderr}"
        );

        // sign --all-mine: with --config, the forgery guard accepts
        // flag-agent's comment as our own. If --config were dropped,
        // the resolved identity would be walker-agent and the
        // flag-agent comment would be classified as foreign.
        let _sign_result = run_ok(
            &walker,
            &[
                "sign",
                &doc_str,
                "--all-mine",
                "--config",
                &flag_path,
                "--json",
            ],
        );

        // Strict-mode verify must pass with --config.
        let verify = run(
            &walker,
            &["verify", &doc_str, "--config", &flag_path, "--json"],
        );
        let verify_stderr = str::from_utf8(&verify.stderr).unwrap();
        assert!(
            verify.status.success(),
            "verify under --config should pass; stderr={verify_stderr}"
        );
    }

    // ---------- mcp (startup-level --config) ----------

    #[test]
    fn mcp_startup_config_sets_default_identity_for_tool_calls() {
        let (_tmp, walker, flag_config) = two_realms();
        let doc = seed_doc(&walker, "doc.md", "# Hello\n\nBody.\n");
        let flag_path = String::from(flag_config.to_string_lossy());
        let doc_str = String::from(doc.to_string_lossy());

        let bin = cargo_bin("remargin");
        let mut child = StdCommand::new(bin)
            .current_dir(&walker)
            .args(["mcp", "--config", &flag_path])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let init = json!({
            "jsonrpc": "2.0", "id": 1_i32, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}}
        });
        let call = json!({
            "jsonrpc": "2.0", "id": 2_i32, "method": "tools/call",
            "params": {
                "name": "comment",
                "arguments": {
                    "file": doc_str,
                    "content": "via mcp without per-tool identity",
                }
            }
        });
        let input = format!(
            "{}\n{}\n",
            serde_json::to_string(&init).unwrap(),
            serde_json::to_string(&call).unwrap()
        );

        let write_result = {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(input.as_bytes())
        };
        write_result.unwrap();
        drop(child.stdin.take());

        let output = child.wait_with_output().unwrap();
        let stderr = str::from_utf8(&output.stderr).unwrap();
        assert!(
            output.status.success(),
            "mcp server exited with failure; stderr={stderr}"
        );

        let body = doc_contents(&doc);
        assert!(
            body.contains("author: flag-agent"),
            "mcp comment (no per-tool identity) should use server's startup --config identity, \
             doc was:\n{body}"
        );
        assert!(
            !body.contains("author: walker-agent"),
            "walker-agent must not leak in, doc was:\n{body}"
        );
    }
}
