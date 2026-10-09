//! `remargin registry show` in text and `--json` modes, and without a registry.

use core::str;
use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

const REGISTRY: &str = "\
participants:
  alice:
    type: human
    status: active
    display_name: Alice
    pubkeys:
      - 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPlaceholder'
  bot:
    type: agent
    status: active
";

#[test]
fn registry_show_text_mode_lists_participants() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".remargin-registry.yaml"), REGISTRY).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("registry")
        .arg("show")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(stdout.contains("alice"), "stdout missing alice: {stdout:?}");
    assert!(stdout.contains("bot"), "stdout missing bot: {stdout:?}");
}

#[test]
fn registry_show_json_mode_emits_participants_array() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".remargin-registry.yaml"), REGISTRY).unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("registry")
        .arg("--json")
        .arg("show")
        .output()
        .unwrap();
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = str::from_utf8(&output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout).unwrap();
    let participants = json["participants"].as_array().unwrap();
    assert_eq!(participants.len(), 2, "expected 2 participants: {json}");
}

#[test]
fn registry_show_without_registry_errors() {
    let tmp = TempDir::new().unwrap();
    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .arg("registry")
        .arg("show")
        .output()
        .unwrap();
    assert!(!output.status.success(), "expected failure: {output:?}");
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("no registry"),
        "expected 'no registry' diagnostic: {stderr:?}"
    );
}
