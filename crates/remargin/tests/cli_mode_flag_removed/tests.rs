//! Passes `--mode` to the binary and expects clap to refuse it.

use core::str;

use assert_cmd::Command;

/// Clap rejects a global `--mode` at parse time with exit code 2.
#[test]
fn global_mode_flag_is_rejected() {
    let output = Command::cargo_bin("remargin")
        .unwrap()
        .arg("--mode")
        .arg("open")
        .arg("comment")
        .arg("foo.md")
        .arg("hello")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "expected clap parse failure, got success: {output:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(2_i32),
        "clap parse errors exit with code 2; got {:?}",
        output.status.code()
    );

    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("unexpected argument") && stderr.contains("--mode"),
        "expected clap 'unexpected argument --mode' message, got: {stderr:?}"
    );
}

/// The refusal is global, not per-subcommand: `write` rejects the flag too.
#[test]
fn subcommand_mode_flag_is_rejected() {
    let output = Command::cargo_bin("remargin")
        .unwrap()
        .arg("--mode")
        .arg("strict")
        .arg("write")
        .arg("foo.md")
        .arg("body")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "expected clap parse failure, got success: {output:?}"
    );
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("--mode"),
        "expected --mode in error, got: {stderr:?}"
    );
}
