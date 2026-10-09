//! `claude unrestrict` runs against temp realms, including one seeded with projected rules.

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

fn run_restrict(realm: &TempDir, path: &str) {
    let user_settings = user_settings_arg(realm);
    let out = run_in(
        realm.path(),
        &[
            "claude",
            "restrict",
            path,
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&out, 0);
}

/// Unrestrict removes the `.remargin.yaml` entry; no settings file or sidecar ever existed.
#[test]
fn restrict_then_unprotect_clears_state() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    run_restrict(&realm, "src/secret");

    let project_scope = realm.path().join(".claude/settings.local.json");
    assert!(
        !project_scope.exists(),
        "no settings file should be projected"
    );

    let out = run_in(realm.path(), &["claude", "unrestrict", "src/secret"]);
    assert_status(&out, 0);

    let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap_or_default();
    assert!(
        !yaml.contains("src/secret"),
        "yaml still references the removed entry:\n{yaml}"
    );

    assert!(
        !realm
            .path()
            .join(".claude/.remargin-restrictions.json")
            .exists(),
        "no sidecar should exist for a hook-only realm"
    );
}

/// After unrestrict, writes outside the former allow-list succeed: the guard re-resolves per call.
#[test]
fn layer_1_stops_enforcing_after_unprotect() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    fs::create_dir_all(realm.path().join("src/public")).unwrap();
    fs::write(
        realm.path().join("src/public/foo.md"),
        "---\ntitle: test\n---\n\n# Hi\n",
    )
    .unwrap();
    run_restrict(&realm, "src/secret");

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
            "---\ntitle: test\n---\n\n# Updated\n",
        ],
    );
    assert_ne!(blocked.status.code(), Some(0_i32));
    let blocked_stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        blocked_stderr.contains("outside the allow-list"),
        "expected outside-allow-list refusal, got: {blocked_stderr}"
    );

    let unprotect = run_in(realm.path(), &["claude", "unrestrict", "src/secret"]);
    assert_status(&unprotect, 0);

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
            "---\ntitle: test\n---\n\n# Updated\n",
        ],
    );
    assert_status(&allowed, 0);
    let body = fs::read_to_string(realm.path().join("src/public/foo.md")).unwrap();
    assert!(body.contains("# Updated"));
}

/// The wildcard cycle prunes the empty restrict array and the then-empty permissions block.
#[test]
fn wildcard_restrict_and_unprotect_cycle() {
    let realm = realm_with_claude();
    fs::write(realm.path().join("anywhere.md"), "x").unwrap();
    run_restrict(&realm, "*");

    let unprotect = run_in(realm.path(), &["claude", "unrestrict", "*"]);
    assert_status(&unprotect, 0);

    let body = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
    assert!(
        !body.contains("permissions:") && !body.contains("restrict:"),
        "wildcard unprotect should compact .remargin.yaml: {body}",
    );
}

/// `--json` output parses to the `UnprotectOutcome` shape.
#[test]
fn unprotect_json_output_round_trips() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    run_restrict(&realm, "src/secret");

    let out = run_in(
        realm.path(),
        &["claude", "unrestrict", "src/secret", "--json"],
    );
    assert_status(&out, 0);
    let stdout = str::from_utf8(&out.stdout).unwrap();
    let value: Value = serde_json::from_str(stdout).unwrap();
    assert!(value.get("absolute_path").is_some());
    assert!(value.get("anchor").is_some());
    assert_eq!(value["yaml_entry_removed"], json!(true));
    assert!(value.get("warnings").and_then(Value::as_array).is_some());
}

/// `tools/list` does not advertise it, and calling `claude_unrestrict` returns a CLI-pointing error.
#[test]
fn unprotect_absent_from_mcp_surface() {
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
        !names.contains(&"claude_unrestrict"),
        "claude_unrestrict must not appear in tools/list, got: {names:?}"
    );

    let call_request = json!({
        "jsonrpc": "2.0",
        "id": 2_i32,
        "method": "tools/call",
        "params": {
            "name": "claude_unrestrict",
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
        "claude_unrestrict dispatch must surface as a tool error"
    );
    let text = call_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        text.contains("not available via MCP"),
        "expected refusal pointing to CLI, got: {text}"
    );
    assert!(text.contains("remargin claude unrestrict"), "got: {text}");
}

/// A tracked rule hand-deleted from both settings files earns one warning per file.
#[test]
fn unprotect_warns_per_settings_file_when_both_have_hand_deleted_rules() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    let realm_local = realm.path().join(".claude/settings.local.json");
    let user_scope = user_settings_arg(&realm);

    // `restrict` projects nothing, so the sidecar-tracked state is seeded by hand.
    let canonical_realm = fs::canonicalize(realm.path()).unwrap();
    let target_key = format!("{}/src/secret", canonical_realm.display());
    let tracked_rule = format!("Edit({target_key}/**)");
    fs::write(
        realm.path().join(".remargin.yaml"),
        "permissions:\n  trusted_roots:\n    - path: src/secret\n",
    )
    .unwrap();
    let empty_deny = json!({ "permissions": { "deny": [] } });
    fs::write(&realm_local, empty_deny.to_string()).unwrap();
    fs::write(&user_scope, empty_deny.to_string()).unwrap();
    let sidecar_seed = json!({
        "version": 1_u32,
        "entries": {
            target_key: {
                "added_at": "legacy",
                "added_to_files": [realm_local.to_string_lossy(), user_scope.to_string_lossy()],
                "allow": [],
                "deny": [tracked_rule],
            }
        }
    });
    fs::write(
        realm.path().join(".claude/.remargin-restrictions.json"),
        sidecar_seed.to_string(),
    )
    .unwrap();

    let out = run_in(
        realm.path(),
        &[
            "claude",
            "unrestrict",
            "src/secret",
            "--user-settings",
            user_scope.to_str().unwrap(),
        ],
    );
    assert_status(&out, 0);
    let stderr = str::from_utf8(&out.stderr).unwrap();

    // `revert_rules` owns the wording; matching on `not present in` survives small changes to it.
    let warning_count = stderr.matches("not present in").count();
    assert_eq!(
        warning_count, 2,
        "expected one not-present warning per settings file, got {warning_count}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains(".claude/settings.local.json"),
        "stderr should name the realm-local file: {stderr}"
    );
    assert!(
        stderr.contains("hermetic-user-settings.json"),
        "stderr should name the user-scope file: {stderr}"
    );

    let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
    assert!(
        !yaml.contains("src/secret"),
        "restrict entry should have been removed from yaml: {yaml}"
    );

    let sidecar_body =
        fs::read_to_string(realm.path().join(".claude/.remargin-restrictions.json")).unwrap();
    let sidecar: Value = serde_json::from_str(&sidecar_body).unwrap();
    assert!(
        sidecar["entries"].as_object().unwrap().is_empty(),
        "sidecar entries should be empty after unprotect: {sidecar}"
    );
}

/// A second `unrestrict` warns and exits 0.
#[test]
fn cli_unprotect_is_idempotent() {
    let realm = realm_with_claude();
    fs::create_dir_all(realm.path().join("src/secret")).unwrap();
    run_restrict(&realm, "src/secret");
    run_in(realm.path(), &["claude", "unrestrict", "src/secret"]);
    let second = run_in(realm.path(), &["claude", "unrestrict", "src/secret"]);
    assert_status(&second, 0);
    let stderr = str::from_utf8(&second.stderr).unwrap();
    assert!(
        stderr.contains("not currently restricted"),
        "expected idempotent warn, got: {stderr}"
    );
}

/// `--strict` against an unrestricted path exits non-zero and leaves the yaml untouched.
#[test]
fn cli_unprotect_strict_unrestricted_path_fails() {
    let realm = realm_with_claude();
    let yaml_path = realm.path().join(".remargin.yaml");
    let out = run_in(
        realm.path(),
        &["claude", "unrestrict", "src/secret", "--strict"],
    );
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(
        stderr.contains("not currently restricted") && stderr.contains("--strict"),
        "expected strict refusal with --strict in message, got: {stderr}",
    );
    assert!(!yaml_path.exists(), "no .remargin.yaml should be created");
}

/// Unrestrict scrubs every deny rule an older `restrict` projected, through the matching sidecar.
#[test]
fn unrestrict_scrubs_rules_projected_by_an_older_restrict() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_unprotect");
    let legacy_settings_body =
        fs::read_to_string(fixture_dir.join("legacy-settings.json")).unwrap();
    let sidecar_body = fs::read_to_string(fixture_dir.join("sidecar.json")).unwrap();

    // The fixture sidecar names `/realm/...` paths, so it is repointed at the temp realm.
    let realm = realm_with_claude();
    let realm_path = realm.path();
    let project_scope = realm_path.join(".claude/settings.local.json");
    fs::write(&project_scope, &legacy_settings_body).unwrap();

    let mut sidecar_value: Value = serde_json::from_str(&sidecar_body).unwrap();
    let entries = sidecar_value["entries"].as_object_mut().unwrap();
    let entry = entries.values_mut().next().unwrap();
    entry["added_to_files"] = json!([project_scope.to_string_lossy()]);
    let sidecar_path = realm_path.join(".claude/.remargin-restrictions.json");
    fs::write(
        &sidecar_path,
        serde_json::to_string_pretty(&sidecar_value).unwrap(),
    )
    .unwrap();

    let yaml = "permissions:\n  trusted_roots:\n    - path: src/secret\n";
    fs::write(realm_path.join(".remargin.yaml"), yaml).unwrap();
    let new_key = realm_path.join("src/secret").to_string_lossy().to_string();
    let mut sidecar_value_v2: Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    let entries_v2 = sidecar_value_v2["entries"].as_object_mut().unwrap();
    let prev_value = entries_v2.remove("/realm/src/secret").unwrap();
    entries_v2.insert(new_key, prev_value);
    fs::write(
        &sidecar_path,
        serde_json::to_string_pretty(&sidecar_value_v2).unwrap(),
    )
    .unwrap();

    let pre_settings: Value =
        serde_json::from_str(&fs::read_to_string(&project_scope).unwrap()).unwrap();
    let pre_deny = pre_settings["permissions"]["deny"].as_array().unwrap();
    assert!(
        pre_deny.len() >= 80,
        "fixture should carry the ~80 legacy rules, got {}",
        pre_deny.len()
    );

    let user_settings = realm_path.join("hermetic-user-settings.json");
    // `unrestrict` needs the user settings file to exist; the sidecar names only the project file.
    fs::write(&user_settings, "{}").unwrap();
    let out = run_in(
        realm_path,
        &[
            "claude",
            "unrestrict",
            "src/secret",
            "--user-settings",
            user_settings.to_str().unwrap(),
        ],
    );
    assert_status(&out, 0);

    let post_settings: Value =
        serde_json::from_str(&fs::read_to_string(&project_scope).unwrap()).unwrap();
    let post_deny = post_settings["permissions"]["deny"].as_array().unwrap();
    assert!(
        post_deny.is_empty(),
        "every legacy rule should be scrubbed; remaining: {post_deny:#?}"
    );

    let post_sidecar: Value =
        serde_json::from_str(&fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    let post_entries = post_sidecar["entries"].as_object().unwrap();
    assert!(
        post_entries.is_empty(),
        "sidecar entries should be empty after unprotect, got: {post_entries:?}"
    );
}
