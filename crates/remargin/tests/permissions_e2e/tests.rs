//! Cross-feature permissions runs: restrict, unrestrict and guarded writes on one temp realm.

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
    let mut args: Vec<&str> = vec![
        "claude",
        "restrict",
        path,
        "--user-settings",
        user.to_str().unwrap(),
    ];
    args.extend_from_slice(extra_args);
    let out = run_in(realm.path(), &args);
    assert_status(&out, 0);
}

fn unprotect_in(realm: &TempDir, path: &str) {
    let user = user_settings(realm);
    let out = run_in(
        realm.path(),
        &[
            "claude",
            "unrestrict",
            path,
            "--user-settings",
            user.to_str().unwrap(),
        ],
    );
    assert_status(&out, 0);
}

fn write_md(realm: &TempDir, rel: &str, body: &str) {
    let path = realm.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Calling the absent MCP `restrict` tool is a CLI-pointing error that leaves the realm untouched.
#[test]
fn mcp_restrict_is_inert_and_leaves_realm_untouched() {
    let realm = realm_with_claude();
    write_md(&realm, "src/secret/foo.md", "---\ntitle: t\n---\n\n# Hi\n");
    let mcp_user_settings = user_settings(&realm);

    let system = RealSystem::new();
    let base = system.canonicalize(realm.path()).unwrap();
    let config = ResolvedConfig::resolve(&system, &base, &IdentityFlags::default(), None).unwrap();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": {
            "name": "claude_restrict",
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
        "MCP claude_restrict must surface as a tool error"
    );

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

/// With two paths restricted, unrestricting one leaves only the other in `.remargin.yaml`.
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
    let restricts = yaml["permissions"]["trusted_roots"].as_sequence().unwrap();
    assert_eq!(restricts.len(), 1);
    assert_eq!(
        restricts[0]["path"],
        serde_yaml::Value::String(String::from("archive"))
    );

    assert!(
        !realm
            .path()
            .join(".claude/.remargin-restrictions.json")
            .exists(),
        "no sidecar should exist for a hook-only realm"
    );
}

/// `op_guard` re-resolves per call: a write refused before a `.remargin.yaml` edit passes after it.
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

    // The key is dropped entirely: an empty list means locked, and open mode needs it absent.
    fs::write(realm.path().join(".remargin.yaml"), "permissions: {}\n").unwrap();

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

/// With no `permissions:` block, mutating ops succeed: the feature is opt-in.
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

/// Under a restricted `src/secret`, `src/secret/.git/foo.md` is refused with `.git` unlisted.
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

/// `allow_dot_folders: ['.git']` lifts the dot-folder refusal; the broader restrict still applies.
#[test]
fn allow_dot_folders_permits_named_dot_folder() {
    let realm = realm_with_claude();
    write_md(
        &realm,
        "src/secret/.git/foo.md",
        "---\ntitle: t\n---\n\n# Hi\n",
    );
    restrict_in(&realm, "src/secret", &[]);

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
    // The op is still refused by the surrounding restrict; the assertion is that the refusal does
    // not mention dot-folders.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("dot-folder"),
        "dot-folder default-deny should be bypassed when allow_dot_folders names it; got: {stderr}"
    );
}

/// `also_deny_bash` lands on the `.remargin.yaml` entry and no settings file is projected.
#[test]
fn also_deny_bash_lands_on_yaml_entry() {
    let realm = realm_with_claude();
    write_md(&realm, "src/secret/foo.md", "x");
    restrict_in(
        &realm,
        "src/secret",
        &["--also-deny-bash", "aria2c", "--also-deny-bash", "nc"],
    );

    let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let extras: Vec<String> = value["permissions"]["trusted_roots"][0]["also_deny_bash"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(extras.contains(&"aria2c".to_owned()), "got: {extras:?}");
    assert!(extras.contains(&"nc".to_owned()), "got: {extras:?}");

    assert!(
        !realm.path().join(".claude/settings.local.json").exists(),
        "no settings file should be written"
    );
}

/// `--cli-allowed` lands on the `.remargin.yaml` entry; the `PreToolUse` hook reads it from there.
#[test]
fn cli_allowed_persists_on_yaml_entry() {
    let realm = realm_with_claude();
    write_md(&realm, "src/secret/foo.md", "x");
    restrict_in(&realm, "src/secret", &["--cli-allowed"]);

    let yaml = fs::read_to_string(realm.path().join(".remargin.yaml")).unwrap();
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        value["permissions"]["trusted_roots"][0]["cli_allowed"],
        serde_yaml::Value::Bool(true)
    );
    assert!(
        !realm.path().join(".claude/settings.local.json").exists(),
        "no settings file should be written"
    );
}
