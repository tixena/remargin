//! `remargin permissions show / check` CLI + MCP integration tests
//! (rem-yj1j.7 / T28 / rem-8y1h).
//!
//! Covers spec scenarios 20-22 of T28's testing plan:
//!
//! - Scenario 20: `permissions show` and `permissions check`
//!   surface the parent-walked `.remargin.yaml` permissions correctly
//!   (text + JSON, restricted exit 0).
//! - Scenario 21: when no rules cover a path, `check` exits 1 and
//!   `show` lists the empty surface.
//! - Scenario 22: MCP `permissions_show` and `permissions_check`
//!   parity with CLI `--json` output.
//!
//! T26 (`restrict`) and T27 (`unprotect`) are not yet wired, so these
//! tests stage a `.remargin.yaml` directly. When the CLI restrict/
//! unprotect commands land, they replace the hand-written fixture with
//! a CLI invocation but the assertions stay the same.

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
    use remargin_core::mcp;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    // ---- rem-k7e5: schema mirrors for `permissions show --json` ----
    //
    // The mirrors below are `#[serde(deny_unknown_fields)]` so any
    // new field on the corresponding Rust type fails the build until
    // the schema doc on `permissions/inspect.rs` is updated.

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AllowDotFoldersSchema {
        names: Vec<String>,
        source_file: String,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct DenyOpsSchema {
        ops: Vec<String>,
        path: String,
        source_file: String,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TrustedRootSchema {
        absolute_path: Option<String>,
        also_deny_bash: Vec<String>,
        cli_allowed: bool,
        path_text: String,
        realm_root: Option<String>,
        source_file: String,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ShowSchema {
        allow_dot_folders: Vec<AllowDotFoldersSchema>,
        deny_ops: Vec<DenyOpsSchema>,
        elapsed_ms: u64,
        trusted_roots: Vec<TrustedRootSchema>,
    }

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn stdout_of(out: &Output) -> &str {
        str::from_utf8(&out.stdout).unwrap()
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

    fn write_realm_yaml(realm: &Path, body: &str) {
        fs::write(realm.join(".remargin.yaml"), body).unwrap();
    }

    /// Scenario 20a: `permissions show --json` over a hand-rolled
    /// realm reports the declared `restrict` and `deny_ops` entries.
    #[test]
    fn show_json_lists_declared_entries() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n  deny_ops:\n    - path: archive\n      ops: [purge]\n",
        );
        // Materialise the targets so `restrict_covers` matches paths
        // canonicalised through the real filesystem.
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::create_dir_all(realm.path().join("archive")).unwrap();

        let out = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&out, 0);
        let body: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        let trusted_roots = body.get("trusted_roots").and_then(Value::as_array).unwrap();
        assert_eq!(trusted_roots.len(), 1);
        let deny_ops = body.get("deny_ops").and_then(Value::as_array).unwrap();
        assert_eq!(deny_ops.len(), 1);
        let ops = deny_ops[0].get("ops").and_then(Value::as_array).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].as_str().unwrap(), "purge");
    }

    /// `permissions check` exits 0 when the path is OUTSIDE the
    /// allow-list declared by `restrict` — the path is restricted
    /// (the gitignore-style "matched = success" code, post-polarity-flip).
    #[test]
    fn check_exits_zero_for_restricted_path() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/public")).unwrap();
        fs::write(realm.path().join("src/public/foo.md"), "x").unwrap();

        let out = run_in(realm.path(), &["permissions", "check", "src/public/foo.md"]);
        assert_status(&out, 0);
    }

    /// Scenario 21a: with no `.remargin.yaml`, `permissions show`
    /// returns empty collections under `--json`.
    #[test]
    fn show_json_empty_when_no_config() {
        let realm = TempDir::new().unwrap();
        let out = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&out, 0);
        let body: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        assert!(
            body.get("trusted_roots")
                .and_then(Value::as_array)
                .unwrap()
                .is_empty()
        );
        assert!(
            body.get("deny_ops")
                .and_then(Value::as_array)
                .unwrap()
                .is_empty()
        );
    }

    /// `permissions check` exits 1 when the path IS inside the
    /// allow-list — sanctioned, not restricted, gitignore-style
    /// "not matched".
    #[test]
    fn check_exits_one_when_unrestricted() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::write(realm.path().join("src/secret/foo.md"), "x").unwrap();

        let out = run_in(realm.path(), &["permissions", "check", "src/secret/foo.md"]);
        assert_status(&out, 1);
    }

    /// `--why` populates the matching-rule section in JSON output when
    /// the target is OUTSIDE the allow-list (post-polarity-flip).
    #[test]
    fn check_why_populates_matching_rule() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/public")).unwrap();
        fs::write(realm.path().join("src/public/foo.md"), "x").unwrap();

        let out = run_in(
            realm.path(),
            &[
                "permissions",
                "check",
                "src/public/foo.md",
                "--why",
                "--json",
            ],
        );
        assert_status(&out, 0);
        let body: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        assert_eq!(body.get("restricted").unwrap(), &Value::Bool(true));
        let rule = body.get("matching_rule").unwrap();
        assert_eq!(rule.get("kind").unwrap().as_str().unwrap(), "trusted_roots");
    }

    // --- Scenario 22: MCP parity ------------------------------------------

    fn mcp_call(base: &Path, config: &ResolvedConfig, tool_name: &str, arguments: &Value) -> Value {
        let system = RealSystem::new();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1_i32,
            "method": "tools/call",
            "params": { "name": tool_name, "arguments": arguments }
        });
        let request_str = serde_json::to_string(&request).unwrap();
        let response = mcp::process_request(&system, base, config, &request_str)
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();
        parsed.get("result").unwrap().clone()
    }

    fn mcp_payload(result: &Value) -> Value {
        let content = result.get("content").and_then(Value::as_array).unwrap();
        let text = content[0].get("text").and_then(Value::as_str).unwrap();
        let mut parsed: Value = serde_json::from_str(text).unwrap();
        strip_elapsed_ms(&mut parsed);
        parsed
    }

    /// Drop the `elapsed_ms` field injected by both surfaces so the
    /// comparison focuses on the structured payload. CLI / MCP run in
    /// different processes and rarely report the same elapsed time, but
    /// every other field is identical by construction.
    fn strip_elapsed_ms(value: &mut Value) {
        if let Value::Object(map) = value {
            map.remove("elapsed_ms");
        }
    }

    fn parse_cli_json(out: &Output) -> Value {
        let mut value: Value = serde_json::from_str(stdout_of(out)).unwrap();
        strip_elapsed_ms(&mut value);
        value
    }

    /// `permissions_show` MCP tool returns the same JSON shape the CLI
    /// emits under `--json`.
    #[test]
    fn mcp_permissions_show_matches_cli_json() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n  deny_ops:\n    - path: archive\n      ops: [purge]\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::create_dir_all(realm.path().join("archive")).unwrap();

        // CLI side.
        let cli = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&cli, 0);
        let cli_body = parse_cli_json(&cli);

        // MCP side.
        let system = RealSystem::new();
        // Canonicalise the temp path so the parent-walk inside the MCP
        // resolver matches the CLI's canonicalised cwd.
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();
        let result = mcp_call(&base, &config, "permissions_show", &json!({}));
        let mcp_body = mcp_payload(&result);

        assert_eq!(cli_body, mcp_body, "CLI and MCP show output diverged");
    }

    /// `permissions_check` MCP tool agrees with CLI `--json` for both
    /// restricted (= outside allow-list) and unrestricted (= inside) targets.
    #[test]
    fn mcp_permissions_check_matches_cli_json() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::write(realm.path().join("src/secret/foo.md"), "x").unwrap();
        fs::create_dir_all(realm.path().join("src/public")).unwrap();
        fs::write(realm.path().join("src/public/foo.md"), "x").unwrap();

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();

        // Outside allow-list → restricted.
        let cli_hit = run_in(
            realm.path(),
            &[
                "permissions",
                "check",
                "src/public/foo.md",
                "--json",
                "--why",
            ],
        );
        assert_status(&cli_hit, 0);
        let cli_hit_body = parse_cli_json(&cli_hit);
        let mcp_hit = mcp_call(
            &base,
            &config,
            "permissions_check",
            &json!({ "path": "src/public/foo.md", "why": true }),
        );
        let mcp_hit_body = mcp_payload(&mcp_hit);
        assert_eq!(cli_hit_body, mcp_hit_body);

        // Inside allow-list → not restricted.
        let cli_miss = run_in(
            realm.path(),
            &["permissions", "check", "src/secret/foo.md", "--json"],
        );
        assert_status(&cli_miss, 1);
        let cli_miss_body = parse_cli_json(&cli_miss);
        let mcp_miss = mcp_call(
            &base,
            &config,
            "permissions_check",
            &json!({ "path": "src/secret/foo.md" }),
        );
        let mcp_miss_body = mcp_payload(&mcp_miss);
        assert_eq!(cli_miss_body, mcp_miss_body);
    }

    /// rem-k7e5: pin the canonical `permissions show --json` schema
    /// against the doc in `permissions/inspect.rs`. Strict-mode
    /// deserialise into [`ShowSchema`] aborts the test if any new
    /// undocumented field appears in the output. Per-entry semantics
    /// are pinned in companion assertions below.
    #[test]
    fn permissions_show_json_shape_is_canonical() {
        let realm = canonical_schema_realm();
        let out = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&out, 0);
        let stdout = stdout_of(&out);

        let parse: Result<ShowSchema, _> = serde_json::from_str(stdout);
        assert!(
            parse.is_ok(),
            "permissions show --json drifted from documented schema: {:?}\nbody: {stdout}",
            parse.err()
        );
        let parsed = parse.unwrap();

        // Read every documented field — pins the doc semantics and
        // also keeps the strict `dead_code` lint quiet without
        // per-struct `#[allow]`s (banned by clippy::restriction).
        assert!(
            parsed.elapsed_ms < 60_000,
            "elapsed_ms unrealistically large"
        );
        assert_eq!(parsed.allow_dot_folders.len(), 1);
        let dot = &parsed.allow_dot_folders[0];
        assert_eq!(dot.names, vec![String::from(".obsidian")]);
        assert!(!dot.source_file.is_empty());
        assert_eq!(parsed.deny_ops.len(), 1);
        let deny = &parsed.deny_ops[0];
        assert_eq!(deny.ops, vec![String::from("purge")]);
        assert!(!deny.path.is_empty());
        assert!(!deny.source_file.is_empty());

        assert_trusted_root_wildcard_invariant(&parsed.trusted_roots);

        // Belt-and-suspenders: also flag an undocumented top-level
        // key by inspecting the raw Value, not just the typed mirror.
        let body: Value = serde_json::from_str(stdout).unwrap();
        let documented = [
            "allow_dot_folders",
            "deny_ops",
            "elapsed_ms",
            "trusted_roots",
        ];
        for key in body.as_object().unwrap().keys() {
            assert!(
                documented.contains(&key.as_str()),
                "undocumented top-level key {key:?} in permissions show --json output"
            );
        }
    }

    /// Build the canonical schema-coverage realm: a wildcard
    /// restrict (so `realm_root` is non-null), an absolute-path
    /// restrict (so `realm_root` is null), a `deny_ops` with `ops`,
    /// and an `allow_dot_folders` entry.
    fn canonical_schema_realm() -> TempDir {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  \
             trusted_roots:\n    - path: src/secret\n    - path: '*'\n  \
             deny_ops:\n    - path: archive\n      ops: [purge]\n  \
             allow_dot_folders:\n    - .obsidian\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        fs::create_dir_all(realm.path().join("archive")).unwrap();
        realm
    }

    /// Pin the schema-doc claim that `realm_root` is non-null only
    /// for wildcard `path: '*'` entries, and `absolute_path` is
    /// non-null otherwise.
    fn assert_trusted_root_wildcard_invariant(restrict: &[TrustedRootSchema]) {
        let mut saw_wildcard = false;
        let mut saw_absolute = false;
        for entry in restrict {
            assert!(!entry.source_file.is_empty());
            // `also_deny_bash` and `cli_allowed` must round-trip;
            // touching them keeps the strict-mirror types honest.
            let _: &Vec<String> = &entry.also_deny_bash;
            let _: bool = entry.cli_allowed;
            if entry.path_text == "*" {
                assert!(
                    entry.realm_root.is_some(),
                    "wildcard restrict missing realm_root, path_text={:?}",
                    entry.path_text
                );
                saw_wildcard = true;
            } else {
                assert!(
                    entry.realm_root.is_none(),
                    "non-wildcard restrict has unexpected realm_root, path_text={:?}",
                    entry.path_text
                );
                assert!(entry.absolute_path.is_some());
                saw_absolute = true;
            }
        }
        assert!(saw_wildcard, "missing wildcard restrict entry");
        assert!(saw_absolute, "missing absolute-path restrict entry");
    }

    /// rem-k7e5: empty config still respects the canonical schema —
    /// every documented top-level key is present (with empty
    /// arrays) plus `elapsed_ms`.
    #[test]
    fn permissions_show_json_empty_shape_is_canonical() {
        let realm = TempDir::new().unwrap();
        let out = run_in(realm.path(), &["permissions", "show", "--json"]);
        assert_status(&out, 0);
        let body: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        let map = body.as_object().unwrap();
        for key in [
            "allow_dot_folders",
            "deny_ops",
            "elapsed_ms",
            "trusted_roots",
        ] {
            assert!(
                map.contains_key(key),
                "empty payload missing key {key}: {body}"
            );
        }
        for array_key in ["allow_dot_folders", "deny_ops", "trusted_roots"] {
            assert!(
                map.get(array_key)
                    .and_then(Value::as_array)
                    .unwrap()
                    .is_empty(),
                "{array_key} should be empty"
            );
        }
        assert!(map.get("elapsed_ms").unwrap().is_u64());
    }

    /// `permissions show` text output names the realm and the
    /// restricted entry. Smoke test for the human-readable formatter.
    #[test]
    fn show_text_output_includes_restrict_entry() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();

        let out = run_in(realm.path(), &["permissions", "show"]);
        assert_status(&out, 0);
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("trusted_roots:"),
            "expected 'trusted_roots:' header in:\n{stderr}",
        );
        assert!(
            stderr.contains("src/secret"),
            "expected restricted path in:\n{stderr}",
        );
    }

    /// `PathBuf` coverage: relative paths beginning with `./` canonicalise
    /// through the CLI surface. Uses an OUTSIDE-the-allow-list target so
    /// the CLI exits 0 (= restricted, allow-list flipped).
    #[test]
    fn check_dot_slash_path_canonicalises() {
        let realm = TempDir::new().unwrap();
        write_realm_yaml(
            realm.path(),
            "permissions:\n  trusted_roots:\n    - path: src/secret\n",
        );
        fs::create_dir_all(realm.path().join("src/public")).unwrap();
        fs::write(realm.path().join("src/public/foo.md"), "x").unwrap();

        let out = run_in(
            realm.path(),
            &["permissions", "check", "./src/public/foo.md"],
        );
        assert_status(&out, 0);
    }

    // --- rem-w6m1: McpSandbox boundary -------------------------------------

    fn extract_tool_text(result: &Value) -> String {
        let content = result.get("content").and_then(Value::as_array).unwrap();
        content[0]
            .get("text")
            .and_then(Value::as_str)
            .unwrap()
            .to_owned()
    }

    fn is_tool_error(result: &Value) -> bool {
        result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Sandbox bootstrap: with no `trusted_roots`, the spawn cwd is the
    /// only root. Reading a file under it succeeds.
    #[test]
    fn mcp_sandbox_allows_path_under_spawn_cwd() {
        let realm = TempDir::new().unwrap();
        let inner = realm.path().join("note.md");
        fs::write(&inner, b"# hi\n").unwrap();
        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();
        let result = mcp_call(&base, &config, "get", &json!({ "path": "note.md" }));
        assert!(!is_tool_error(&result), "{result:#?}");
        let text = extract_tool_text(&result);
        assert!(text.contains("# hi"), "{text}");
    }

    /// Sandbox enforcement: a path that escapes the cwd's
    /// (canonicalised) root surfaces as a tool-level error containing
    /// the documented `path escapes MCP sandbox` marker.
    #[test]
    fn mcp_sandbox_rejects_path_outside_root() {
        let realm = TempDir::new().unwrap();
        let outsider = TempDir::new().unwrap();
        let outsider_file = outsider.path().join("foo.md");
        fs::write(&outsider_file, b"# leak\n").unwrap();

        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let outsider_canonical = system.canonicalize(&outsider_file).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();
        let result = mcp_call(
            &base,
            &config,
            "get",
            &json!({ "path": outsider_canonical.to_string_lossy() }),
        );
        assert!(is_tool_error(&result), "{result:#?}");
        let text = extract_tool_text(&result);
        assert!(
            text.contains("path escapes MCP sandbox"),
            "expected sandbox-escape error, got: {text}"
        );
    }

    /// `permissions_show` is in the no-path tool list — it works even
    /// when the spawn cwd is the sandbox's only root.
    #[test]
    fn mcp_sandbox_lets_permissions_show_through() {
        let realm = TempDir::new().unwrap();
        let system = RealSystem::new();
        let base = system.canonicalize(realm.path()).unwrap();
        let config =
            ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();
        let result = mcp_call(&base, &config, "permissions_show", &json!({}));
        assert!(!is_tool_error(&result), "{result:#?}");
    }

    // The two `trusted_roots`-shaped scenarios that lived here
    // (recursive-realm-respect and no-transitive-trust) tested the old
    // deny-list-with-carve-out polarity. Post-eradication, the relevant
    // semantics are pinned by op_guard's allow-list scenarios in
    // `remargin-core/src/permissions/op_guard/tests.rs`.
}
