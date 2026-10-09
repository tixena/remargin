//! Runs `remargin keygen` and reads the keypair back from disk.

use core::str;
use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn keygen_writes_private_and_public_keypair() {
    let tmp = TempDir::new().unwrap();
    let priv_path = tmp.path().join("id_remargin");

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .env("HOME", tmp.path())
        .arg("keygen")
        .arg(&priv_path)
        .output()
        .unwrap();

    assert!(output.status.success(), "keygen failed: {output:?}");

    let pub_path = priv_path.with_extension("pub");
    assert!(priv_path.exists(), "private key not written");
    assert!(pub_path.exists(), "public key not written");

    let priv_bytes = fs::read_to_string(&priv_path).unwrap();
    assert!(
        priv_bytes.contains("BEGIN OPENSSH PRIVATE KEY"),
        "private key must be OpenSSH-formatted: {priv_bytes:?}"
    );
    let pub_bytes = fs::read_to_string(&pub_path).unwrap();
    assert!(
        pub_bytes.starts_with("ssh-ed25519 "),
        "public key must be ed25519 OpenSSH: {pub_bytes:?}"
    );

    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("Private key:") && stderr.contains("Public key:"),
        "stderr missing path summary: {stderr:?}"
    );
}
