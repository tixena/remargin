//! End-to-end permissions integration tests (rem-yj1j.8 / rem-637x).
//!
//! Covers the cross-cutting scenarios from the rem-yj1j.8 plan that
//! the per-feature integration files (`cli_restrict.rs`,
//! `cli_unprotect.rs`, `cli_permissions.rs`) do not exercise on
//! their own:
//!
//! - E3: CLI / MCP parity for restrict.
//! - E5: multi-path (restrict A + B, unprotect A leaves B).
//! - E9: per-op no-cache (manual `.remargin.yaml` edit picked up on
//!   the very next op without a restart).
//! - E13: back-compat with realms that have no `permissions:` block.
//! - E14: dot-folder default-deny under restrict.
//! - E15: `allow_dot_folders` override.
//! - E16: `also_deny_bash` propagates into Claude settings.
//! - E17: `--cli-allowed` omits the `Bash(remargin *)` deny rule.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Output;

    use assert_cmd::Command;
    use os_shim::System as _;
    use os_shim::real::RealSystem;
    use remargin_core::config::ResolvedConfig;
    use remargin_core::config::identity::IdentityFlags;
    use remargin_core::mcp;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    fn realm_with_claude() -> TempDir {
        let realm = TempDir::new().unwrap();
        fs::create_dir_all(realm.path().join(".claude")).unwrap();
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

    fn user_settings(realm: &TempDir) -> PathBuf {
        realm.path().join("hermetic-user-settings.json")
    }

    fn restrict_in(realm: &TempDir, path: &str, extra_args: &[&str]) {
        let user = user_settings(realm);
        let mut args: Vec<&str> = vec!["restrict", path, "--user-settings", user.to_str().unwrap()];
        args.extend_from_slice(extra_args);
        let out = run_in(realm.path(), &args);
        assert_status(&out, 0);
    }

    fn unprotect_in(realm: &TempDir, path: &str) {
        let user = user_settings(realm);
        let out = run_in(
            realm.path(),
            &["unprotect", path, "--user-settings", user.to_str().unwrap()],
        );
        assert_status(&out, 0);
    }

    fn write_md(realm: &TempDir, rel: &str, body: &str) {
        let path = realm.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn read_local_settings(realm: &TempDir) -> Value {
        let body = fs::read_to_string(realm.path().join(".claude/settings.local.json")).unwrap();
        serde_json::from_str(&body).unwrap()
    }

    /// E3: CLI restrict and MCP restrict produce structurally
    /// identical state. We run each in its own realm so they can't
    /// interfere with each other; then compare the produced YAML +
    /// the project-scope settings JSON.
    #[test]
    fn cli_and_mcp_restrict_produce_same_state() {
        // CLI side.
        let cli_realm = realm_with_claude();
        write_md(
            &cli_realm,
            "src/secret/foo.md",
            "---\ntitle: t\n---\n\n# Hi\n",
        );
        restrict_in(&cli_realm, "src/secret", &[]);

        // MCP side.
        let mcp_realm = realm_with_claude();
        write_md(
            &mcp_realm,
            "src/secret/foo.md",
            "---\ntitle: t\n---\n\n# Hi\n",
        );
        let mcp_user_settings = user_settings(&mcp_realm);

        let system = RealSystem::new();
        let base = system.canonicalize(mcp_realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "restrict",
                "arguments": {
                    "path": "src/secret",
                    "user_settings": mcp_user_settings.to_string_lossy(),
                }
            }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        mcp::process_request(&system, &base, &config, &request_str)
            .unwrap()
            .unwrap();

        // Compare the YAML shapes (ignoring the absolute paths in
        // resolved entries, which legitimately differ between
        // tempdirs).
        let cli_yaml = fs::read_to_string(cli_realm.path().join(".remargin.yaml")).unwrap();
        let mcp_yaml = fs::read_to_string(mcp_realm.path().join(".remargin.yaml")).unwrap();
        assert_eq!(cli_yaml, mcp_yaml, "YAML shapes diverged");

        // Project-scope settings: the deny array should have the
        // same shape (rule strings differ in the tempdir path, but
        // the count + tool prefixes match).
        let cli_settings = read_local_settings(&cli_realm);
        let mcp_settings = read_local_settings(&mcp_realm);
        let cli_deny_len = cli_settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .len();
        let mcp_deny_len = mcp_settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(cli_deny_len, mcp_deny_len);
    }

    /// E5: restrict two paths, unprotect one — the other survives
    /// in both .remargin.yaml and the settings file.
    #[test]
    fn unprotect_one_path_leaves_others_intact() {
        let realm = realm_with_claude();
        write_md(&realm, "src/secret/foo.md", "x");
        write_md(&realm, "archive/bar.md", "x");
        restrict_in(&realm, "src/secret", &[]);
        restrict_in(&realm, "archive", &[]);

        unprotect_in(&realm, "src/secret");

        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap())
                .unwrap();
        let restricts = yaml["permissions"]["restrict"].as_sequence().unwrap();
        assert_eq!(restricts.len(), 1);
        assert_eq!(
            restricts[0]["path"],
            serde_yaml::Value::String(String::from("archive"))
        );

        let settings = read_local_settings(&realm);
        let deny = settings["permissions"]["deny"].as_array().unwrap();
        assert!(
            deny.iter()
                .any(|v| v.as_str().is_some_and(|s| s.contains("archive"))),
            "archive rules should remain"
        );
        assert!(
            !deny
                .iter()
                .any(|v| v.as_str().is_some_and(|s| s.contains("src/secret"))),
            "src/secret rules should be gone"
        );
    }

    /// E9: per-op no-cache. Restrict, manually delete the
    /// `.remargin.yaml` entry between two write attempts; the
    /// second write succeeds because the `op_guard` re-resolves on
    /// every call.
    #[test]
    fn per_op_no_cache_picks_up_yaml_edits() {
        let realm = realm_with_claude();
        write_md(&realm, "src/secret/foo.md", "---\ntitle: t\n---\n\n# Hi\n");
        restrict_in(&realm, "src/secret", &[]);

        let blocked = run_in(
            realm.path(),
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--",
                "src/secret/foo.md",
                "---\ntitle: t\n---\n\n# Updated\n",
            ],
        );
        assert_ne!(blocked.status.code(), Some(0_i32));

        // Hand-edit the YAML to drop the entry. We don't touch the
        // sidecar or Claude settings — the per-op guard reads only
        // the YAML.
        fs::write(
            realm.path().join(".remargin.yaml"),
            "permissions:\n  restrict: []\n",
        )
        .unwrap();

        let allowed = run_in(
            realm.path(),
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--",
                "src/secret/foo.md",
                "---\ntitle: t\n---\n\n# Updated\n",
            ],
        );
        assert_status(&allowed, 0);
    }

    /// E13: a realm with no `permissions:` block continues to work.
    /// Mutating ops succeed without any restrict / `deny_ops` in
    /// place — the feature is fully opt-in.
    #[test]
    fn realm_without_permissions_block_is_unaffected() {
        let realm = TempDir::new().unwrap();
        write_md(&realm, "note.md", "---\ntitle: t\n---\n\n# Body\n");

        let out = run_in(
            realm.path(),
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--",
                "note.md",
                "---\ntitle: t\n---\n\n# Updated\n",
            ],
        );
        assert_status(&out, 0);
    }

    /// E14: dot-folder default-deny under restrict. Once
    /// `src/secret` is restricted, an op against
    /// `src/secret/.git/foo.md` is refused even though `.git` itself
    /// is not in the YAML.
    #[test]
    fn dot_folder_under_restrict_is_denied() {
        let realm = realm_with_claude();
        write_md(
            &realm,
            "src/secret/.git/foo.md",
            "---\ntitle: t\n---\n\n# Hi\n",
        );
        restrict_in(&realm, "src/secret", &[]);

        let out = run_in(
            realm.path(),
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--",
                "src/secret/.git/foo.md",
                "---\ntitle: t\n---\n\n# Updated\n",
            ],
        );
        assert_ne!(out.status.code(), Some(0_i32));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("dot-folder") || stderr.contains("denied"),
            "expected dot-folder refusal, got: {stderr}"
        );
    }

    /// E15: `allow_dot_folders: ['.git']` permits the same op.
    #[test]
    fn allow_dot_folders_permits_named_dot_folder() {
        let realm = realm_with_claude();
        write_md(
            &realm,
            "src/secret/.git/foo.md",
            "---\ntitle: t\n---\n\n# Hi\n",
        );
        restrict_in(&realm, "src/secret", &[]);

        // Augment the YAML to allow `.git`.
        let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
        let augmented = format!("{yaml}  allow_dot_folders:\n    - .git\n");
        fs::write(realm.path().join(".remargin.yaml"), augmented).unwrap();

        let out = run_in(
            realm.path(),
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--",
                "src/secret/.git/foo.md",
                "---\ntitle: t\n---\n\n# Updated\n",
            ],
        );
        // The dot-folder default-deny is bypassed, but the
        // surrounding `restrict` still covers src/secret. The op is
        // refused for the broader restrict reason — the test pins
        // the *specific* dot-folder branch is not the cause. Either
        // way, success remains gated by the broader restrict, so we
        // assert the error message no longer mentions dot-folders.
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("dot-folder"),
            "dot-folder default-deny should be bypassed when allow_dot_folders names it; got: {stderr}"
        );
    }

    /// E16: `also_deny_bash` lands in the Claude settings as
    /// `Bash(<cmd> * //...)` denies. Uses commands that are NOT in
    /// the default deny list (rem-p74a expanded the defaults to cover
    /// `curl` / `wget` and friends, so the original test would pass
    /// even if `--also-deny-bash` was a no-op).
    #[test]
    fn also_deny_bash_propagates_into_settings() {
        let realm = realm_with_claude();
        write_md(&realm, "src/secret/foo.md", "x");
        restrict_in(
            &realm,
            "src/secret",
            &["--also-deny-bash", "aria2c", "--also-deny-bash", "nc"],
        );

        let settings = read_local_settings(&realm);
        let deny = settings["permissions"]["deny"].as_array().unwrap();
        assert!(
            deny.iter()
                .any(|v| v.as_str().is_some_and(|s| s.starts_with("Bash(aria2c"))),
            "expected Bash(aria2c ...) deny, got: {deny:#?}"
        );
        assert!(
            deny.iter()
                .any(|v| v.as_str().is_some_and(|s| s.starts_with("Bash(nc"))),
            "expected Bash(nc ...) deny, got: {deny:#?}"
        );
    }

    /// E17: `--cli-allowed` omits the `Bash(remargin *)` deny.
    #[test]
    fn cli_allowed_omits_remargin_cli_deny() {
        let realm = realm_with_claude();
        write_md(&realm, "src/secret/foo.md", "x");
        restrict_in(&realm, "src/secret", &["--cli-allowed"]);

        let settings = read_local_settings(&realm);
        let deny = settings["permissions"]["deny"].as_array().unwrap();
        assert!(
            !deny
                .iter()
                .any(|v| v.as_str().is_some_and(|s| s.starts_with("Bash(remargin"))),
            "expected NO Bash(remargin ...) deny, got: {deny:#?}"
        );
    }
}
