//! `claude restrict` runs against temp realms, read back from the files it writes.

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

fn user_settings_arg(realm: &TempDir) -> PathBuf {
    realm.path().join("hermetic-user-settings.json")
}

/// After `claude restrict src/secret`, a write outside that allow-list is refused by `op_guard`.
#[test]
fn restrict_then_write_outside_allow_list_is_refused() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    fs::create_dir_all(realm.path().join("src/public")).unwrap();
    fs::write(realm.path().join("src/public/foo.md"), "x").unwrap();
    let user_settings = user_settings_arg(&realm);

    let restrict = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&restrict, 0);

    let write = run_in(
        realm.path(),
        &[
            "write",
            "src/public/foo.md",
            "blocked content",
            "--raw",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_ne!(write.status.code(), Some(0_i32), "write should be refused");
    let stderr = String::from_utf8_lossy(&write.stderr);
    assert!(
        stderr.contains("outside the allow-list"),
        "expected outside-allow-list refusal, got: {stderr}"
    );
}

/// Restrict writes only the `.remargin.yaml` entry: no settings file, sidecar or gitignore line.
#[test]
fn restrict_writes_only_yaml_no_settings_sidecar_or_gitignore() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&out, 0);

    let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
    assert!(
        yaml.contains("src/secret"),
        "yaml should carry the entry: {yaml}"
    );

    assert!(
        !realm.path().join(".claude/settings.local.json").exists(),
        "no project-scope settings should be written"
    );
    assert!(
        !user_settings.exists(),
        "no user-scope settings should be written"
    );

    assert!(
        !realm
            .path()
            .join(".claude/.remargin-restrictions.json")
            .exists(),
        "no sidecar should be written"
    );
    assert!(
        !realm.path().join(".gitignore").exists(),
        "no gitignore line should be added"
    );
}

/// Wildcard `restrict '*'` allow-lists the whole realm, so a write inside it passes `op_guard`.
#[test]
fn wildcard_restrict_writes_inside_realm_succeed() {
    let realm = realm_with_claude();
    fs::write(realm.path().join("anywhere.md"), "x").unwrap();
    let user_settings = user_settings_arg(&realm);

    let restrict = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "*",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&restrict, 0);

    // The only error is the unrelated `--raw` / markdown collision, which proves the allow-list
    // check passed.
    let write = run_in(
        realm.path(),
        &[
            "write",
            "anywhere.md",
            "blocked",
            "--raw",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    let stderr = String::from_utf8_lossy(&write.stderr);
    assert!(
        !stderr.contains("outside the allow-list") && !stderr.contains("denied by `restrict`"),
        "wildcard restrict should not refuse writes inside the realm; stderr={stderr}"
    );
}

/// `--json` output parses to the `RestrictOutcome` shape.
#[test]
fn restrict_json_output_round_trips() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
            "--json",
        ],
    );
    assert_status(&out, 0);
    let stdout = str::from_utf8(&out.stdout).unwrap();
    let value: Value = serde_json::from_str(stdout).unwrap();
    assert!(value.get("absolute_path").is_some());
    assert!(value.get("anchor").is_some());
    assert!(
        value
            .get("claude_files_touched")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    assert!(
        value
            .get("rules_applied")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    assert_eq!(value["yaml_was_created"], json!(true));
}

/// `tools/list` does not advertise it, and calling `claude_restrict` returns a CLI-pointing error.
#[test]
fn restrict_absent_from_mcp_surface() {
    let realm = realm_with_claude();

    let system = RealSystem::new();
    let base = system.canonicalize(realm.path()).unwrap();
    let config = ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();

    let list_request = json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/list",
        "params": {}
    });
    let list_response_str =
        mcp::process_request(&system, &base, &config, &list_request.to_string())
            .unwrap()
            .unwrap();
    let list_response: Value = serde_json::from_str(&list_response_str).unwrap();
    let tools = list_response["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(
        !names.contains(&"claude_restrict"),
        "claude_restrict must not appear in tools/list, got: {names:?}"
    );

    let call_request = json!({
        "jsonrpc": "2.0",
        "id": 2_i32,
        "method": "tools/call",
        "params": {
            "name": "claude_restrict",
            "arguments": { "path": "src/secret" }
        }
    });
    let call_response_str =
        mcp::process_request(&system, &base, &config, &call_request.to_string())
            .unwrap()
            .unwrap();
    let call_response: Value = serde_json::from_str(&call_response_str).unwrap();
    assert_eq!(
        call_response["result"]["isError"].as_bool(),
        Some(true),
        "claude_restrict dispatch must surface as a tool error"
    );
    let text = call_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        text.contains("not available via MCP"),
        "expected refusal pointing to CLI, got: {text}"
    );
    assert!(text.contains("remargin claude restrict"), "got: {text}");
}

/// Runs `claude restrict src/secret` with the given `--also-deny-bash` argv and returns the
/// `permissions.trusted_roots[0].also_deny_bash` list parsed back from `.remargin.yaml`.
fn also_deny_bash_for(extra_args: &[&str]) -> Vec<String> {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);
    let mut args: Vec<&str> = vec![
        "claude",
        "restrict",
        "src/secret",
        "--user-settings",
        user_settings.to_str().unwrap(),
    ];
    args.extend_from_slice(extra_args);
    let out = run_in(realm.path(), &args);
    assert_status(&out, 0);

    let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    value["permissions"]["trusted_roots"][0]["also_deny_bash"]
        .as_sequence()
        .map(|s| s.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

/// Repeated `--also-deny-bash` flags emit each token.
#[test]
fn also_deny_bash_repeated_flags() {
    let tokens = also_deny_bash_for(&["--also-deny-bash", "curl", "--also-deny-bash", "wget"]);
    assert_eq!(tokens, vec!["curl".to_owned(), "wget".to_owned()]);
}

/// Comma-separated values split the same way repeated flags do.
#[test]
fn also_deny_bash_comma_separated() {
    let tokens = also_deny_bash_for(&["--also-deny-bash", "curl,wget"]);
    assert_eq!(tokens, vec!["curl".to_owned(), "wget".to_owned()]);
}

/// Mixed comma-separated values and repeated flags concatenate in argv order.
#[test]
fn also_deny_bash_mixed_csv_and_repeated() {
    let tokens = also_deny_bash_for(&["--also-deny-bash", "curl,wget", "--also-deny-bash", "sed"]);
    assert_eq!(
        tokens,
        vec!["curl".to_owned(), "wget".to_owned(), "sed".to_owned()],
    );
}

/// With the flag absent the yaml has no `also_deny_bash` key, or an empty list.
#[test]
fn also_deny_bash_absent_omits_or_empties_field() {
    let tokens = also_deny_bash_for(&[]);
    assert!(
        tokens.is_empty(),
        "expected no extra deny tokens, got: {tokens:?}"
    );
}

#[test]
fn also_deny_bash_single_value() {
    let tokens = also_deny_bash_for(&["--also-deny-bash", "curl"]);
    assert_eq!(tokens, vec!["curl".to_owned()]);
}

/// No `cd` / `pushd` deny rules are projected: the `PreToolUse` hook closes that bypass.
#[test]
fn cd_pushd_denies_not_projected() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&out, 0);

    assert!(
        !realm.path().join(".claude/settings.local.json").exists(),
        "no project-scope settings should be written"
    );
    assert!(
        !user_settings.exists(),
        "no user-scope settings should be written"
    );
}

/// Restrict then unrestrict leaves no settings artifacts: nothing was projected.
#[test]
fn restrict_unprotect_round_trip_leaves_no_settings() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let restrict = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&restrict, 0);

    let unprotect = run_in(
        realm.path(),
        &[
            "claude",
            "unrestrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&unprotect, 0);

    assert!(
        !realm.path().join(".claude/settings.local.json").exists(),
        "no project-scope settings should exist after the round trip"
    );
    assert!(
        !user_settings.exists(),
        "no user-scope settings should exist after the round trip"
    );
}

/// `plan restrict` projects no settings changes, so `settings_files` is empty.
#[test]
fn plan_restrict_projects_no_settings() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let out = run_in(
        realm.path(),
        &[
            "plan",
            "claude",
            "restrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
            "--json",
        ],
    );
    assert_status(&out, 0);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let settings_files = report["config_diff"]["settings_files"].as_array().unwrap();
    assert!(
        settings_files.is_empty(),
        "plan restrict should project no settings files, got {settings_files:?}"
    );
}

/// Re-running restrict leaves `.remargin.yaml` byte-stable and creates no settings file.
#[test]
fn cli_restrict_is_idempotent() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let user_settings = user_settings_arg(&realm);

    let args = [
        "claude",
        "restrict",
        "src/secret",
        "--user-settings",
        user_settings.to_str().unwrap(),
    ];
    let first = run_in(realm.path(), &args);
    assert_status(&first, 0);
    let yaml_path = realm.path().join(".remargin.yaml");
    let first_yaml = fs::read_to_string(&yaml_path).unwrap();

    let second = run_in(realm.path(), &args);
    assert_status(&second, 0);
    let second_yaml = fs::read_to_string(&yaml_path).unwrap();

    assert_eq!(first_yaml, second_yaml, "idempotent re-run must match");
    assert!(
        !realm.path().join(".claude/settings.local.json").exists(),
        "restrict must not create a settings file"
    );
}
