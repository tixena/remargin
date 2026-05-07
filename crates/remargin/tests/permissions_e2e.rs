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

    /// E3 (rem-888p): the MCP `restrict` tool is intentionally absent
    /// from the surface. Calling it leaves the realm completely
    /// untouched (no .remargin.yaml, no settings file mutation), and
    /// the response is a CLI-pointing tool error. Replaces the
    /// previous "CLI and MCP restrict produce same state" parity
    /// check, which no longer applies now that the MCP entry is gone.
    #[test]
    fn mcp_restrict_is_inert_and_leaves_realm_untouched() {
        let realm = realm_with_claude();
        write_md(&realm, "src/secret/foo.md", "---\ntitle: t\n---\n\n# Hi\n");
        let mcp_user_settings = user_settings(&realm);

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
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
        let response_str = mcp::process_request(&system, &base, &config, &request_str)
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&response_str).unwrap();
        assert_eq!(
            response["result"]["isError"].as_bool(),
            Some(true),
            "MCP restrict must surface as a tool error (rem-888p)"
        );

        // No realm artifacts may have been created by the rejected
        // dispatch: no .remargin.yaml, no project-scope settings, no
        // user-scope settings file, no sidecar.
        assert!(
            !realm.path().join(".remargin.yaml").exists(),
            "rejected MCP restrict must not create .remargin.yaml"
        );
        assert!(
            !realm.path().join(".claude/settings.local.json").exists(),
            "rejected MCP restrict must not create project settings"
        );
        assert!(
            !mcp_user_settings.exists(),
            "rejected MCP restrict must not create user settings"
        );
        assert!(
            !realm
                .path()
                .join(".claude/.remargin-restrictions.json")
                .exists(),
            "rejected MCP restrict must not create sidecar"
        );
    }

    /// E5 (rem-egp9): restrict two paths, unprotect one — only the
    /// surviving entry remains in `.remargin.yaml`. The `op_guard`
    /// re-resolves on every call against `.remargin.yaml`, so the
    /// surviving restrict still gates writes regardless of what's in
    /// the Claude settings file.
    ///
    /// Note: under the minimised projection, both restricts emit the
    /// identical `Bash(remargin *)` deny. The sidecar carries one
    /// entry per restrict path; removing the `src/secret` entry's
    /// sidecar entry scrubs the projected string from the settings
    /// file even though the `archive` entry's projection nominally
    /// still wants it. This is acceptable: enforcement of `archive` is
    /// load-bearing on `op_guard` reading `.remargin.yaml`, not on
    /// Claude pattern-matching. A subsequent `restrict archive` re-run
    /// (or running `restrict` again on the existing `archive` entry)
    /// would back-fill the rule. The test pins the YAML survival as
    /// the contract; settings-file behaviour is a follow-up concern.
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

        // Sidecar entry for `src/secret` is gone; the `archive` entry
        // remains so a subsequent unprotect of `archive` knows what
        // to scrub.
        let sidecar_body =
            fs::read_to_string(realm.path().join(".claude/.remargin-restrictions.json")).unwrap();
        let sidecar: serde_json::Value = serde_json::from_str(&sidecar_body).unwrap();
        let entries = sidecar["entries"].as_object().unwrap();
        assert!(
            !entries.keys().any(|k| k.contains("src/secret")),
            "src/secret sidecar entry should be removed: {entries:?}",
        );
        assert!(
            entries.keys().any(|k| k.contains("archive")),
            "archive sidecar entry should remain: {entries:?}",
        );
    }

    /// Per-op no-cache: edit `.remargin.yaml` between two write
    /// attempts; the second write succeeds because `op_guard` re-resolves
    /// every call. Post-polarity-flip: target a path OUTSIDE the
    /// allow-list so the first write is refused, then drop the
    /// allow-list and the second write proceeds in open mode.
    #[test]
    fn per_op_no_cache_picks_up_yaml_edits() {
        let realm = realm_with_claude();
        write_md(&realm, "src/public/foo.md", "---\ntitle: t\n---\n\n# Hi\n");
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
                "src/public/foo.md",
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
                "src/public/foo.md",
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

    // ---------------------------------------------------------------
    // rem-egp9 — per-op sandbox consults `trusted_roots`
    // ---------------------------------------------------------------

    // The `trusted_roots`-extends-the-MCP-sandbox scenario was
    // eradicated along with the deny-list polarity. The MCP sandbox is
    // now always anchored at the spawn cwd; reach across boundaries by
    // spawning the MCP server inside the target realm.
    #[ignore = "eradicated: trusted_roots no longer extends the MCP sandbox"]
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive MCP parity check across write/get/metadata/ls in one realm fixture"
    )]
    fn mcp_write_inside_trusted_root_outside_spawn_cwd_succeeds() {
        let realm = realm_with_claude();
        let realm_canonical = RealSystem::new().canonicalize(realm.path()).unwrap();
        // Spawn cwd is `<realm>/spawn`; trusted root is `<realm>/notes`.
        // Both live under `<realm>` so the realm-level
        // `.remargin.yaml` can declare `notes` as a trusted root.
        let spawn = realm_canonical.join("spawn");
        let notes = realm_canonical.join("notes");
        fs::create_dir_all(&spawn).unwrap();
        fs::create_dir_all(&notes).unwrap();
        fs::write(
            realm_canonical.join(".remargin.yaml"),
            format!(
                "permissions:\n  trusted_roots:\n    - {}\n",
                notes.display()
            ),
        )
        .unwrap();

        // Use a non-markdown file so raw writes are accepted.
        let outside_file = notes.join("widening.txt");

        let system = RealSystem::new();
        let base = system.canonicalize(&spawn).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();

        // MCP write to a path that lives outside the spawn cwd
        // (`<realm>/spawn`) but inside a declared trusted root
        // (`<realm>/notes`) succeeds.
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": {
                "name": "write",
                "arguments": {
                    "path": outside_file.to_string_lossy(),
                    "content": "hello\n",
                    "raw": true,
                    "create": true,
                }
            }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        let response = mcp::process_request(&system, &base, &config, &request_str)
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();
        let result = parsed.get("result").unwrap();
        assert!(
            !result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "MCP write to trusted_root outside spawn cwd must succeed: {result:#?}",
        );
        assert!(
            outside_file.exists(),
            "{} should have been written",
            outside_file.display()
        );
        let body = fs::read_to_string(&outside_file).unwrap();
        assert!(body.contains("hello"), "body: {body}");

        // Same path through `get` succeeds.
        let get_req = json!({
            "jsonrpc": "2.0",
            "id": 2_i32,
            "method": "tools/call",
            "params": {
                "name": "get",
                "arguments": { "path": outside_file.to_string_lossy() }
            }
        });
        let get_resp = mcp::process_request(&system, &base, &config, &get_req.to_string())
            .unwrap()
            .unwrap();
        let parsed_get: Value = serde_json::from_str(&get_resp).unwrap();
        let get_result = parsed_get.get("result").unwrap();
        assert!(
            !get_result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "MCP get on trusted_root outside spawn cwd must succeed: {get_result:#?}",
        );

        // metadata succeeds.
        let meta_req = json!({
            "jsonrpc": "2.0",
            "id": 3_i32,
            "method": "tools/call",
            "params": {
                "name": "metadata",
                "arguments": { "path": outside_file.to_string_lossy() }
            }
        });
        let meta_resp = mcp::process_request(&system, &base, &config, &meta_req.to_string())
            .unwrap()
            .unwrap();
        let parsed_meta: Value = serde_json::from_str(&meta_resp).unwrap();
        let meta_result = parsed_meta.get("result").unwrap();
        assert!(
            !meta_result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "MCP metadata on trusted_root outside spawn cwd must succeed: {meta_result:#?}",
        );

        // ls on the trusted root directory succeeds.
        let ls_req = json!({
            "jsonrpc": "2.0",
            "id": 4_i32,
            "method": "tools/call",
            "params": {
                "name": "ls",
                "arguments": { "path": notes.to_string_lossy() }
            }
        });
        let ls_resp = mcp::process_request(&system, &base, &config, &ls_req.to_string())
            .unwrap()
            .unwrap();
        let parsed_ls: Value = serde_json::from_str(&ls_resp).unwrap();
        let ls_result = parsed_ls.get("result").unwrap();
        assert!(
            !ls_result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "MCP ls on trusted_root outside spawn cwd must succeed: {ls_result:#?}",
        );
    }

    // Eradicated alongside `mcp_write_inside_trusted_root_outside_spawn_cwd_succeeds`.
    #[ignore = "eradicated: trusted_roots no longer extends the CLI sandbox"]
    #[test]
    fn cli_write_inside_trusted_root_outside_cwd_succeeds() {
        let realm = realm_with_claude();
        let realm_canonical = RealSystem::new().canonicalize(realm.path()).unwrap();
        let spawn = realm_canonical.join("spawn");
        let notes = realm_canonical.join("notes");
        fs::create_dir_all(&spawn).unwrap();
        fs::create_dir_all(&notes).unwrap();
        fs::write(
            realm_canonical.join(".remargin.yaml"),
            format!(
                "permissions:\n  trusted_roots:\n    - {}\n",
                notes.display()
            ),
        )
        .unwrap();
        let outside_file = notes.join("widening.txt");

        let out = run_in(
            &spawn,
            &[
                "write",
                "--identity",
                "alice",
                "--type",
                "human",
                "--raw",
                "--create",
                "--",
                outside_file.to_str().unwrap(),
                "hello\n",
            ],
        );
        assert_status(&out, 0);
        assert!(outside_file.exists());
    }
}
