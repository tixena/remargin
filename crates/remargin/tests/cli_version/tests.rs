//! Runs `remargin version` and checks the crate version on stderr.

use core::str;

use assert_cmd::Command;

#[test]
fn version_prints_crate_version_to_stderr() {
    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir("/")
        .env("HOME", "/")
        .arg("version")
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.starts_with("remargin "),
        "expected 'remargin <version>' prefix; got: {stderr:?}"
    );
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        stderr.contains(version),
        "stderr {stderr:?} must carry crate version {version}"
    );
}
