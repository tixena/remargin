//! rem-wws: the `--mode` CLI flag is deleted. Mode is a property of the
//! directory tree (resolved by walking upward for `.remargin.yaml`) and
//! is not caller-overridable. Passing `--mode` must produce a clap-level
//! "unexpected argument" error, not silent acceptance that would let an
//! agent weaken enforcement on a strict vault.

#[cfg(test)]
mod tests {
    use core::str;

    use assert_cmd::Command;

    /// `remargin --mode open comment foo.md "..."` → clap rejects
    /// `--mode` at parse time with exit code 2.
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

    /// Same flag on another subcommand (write) must also reject — the
    /// ban is global, not per-subcommand.
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

    // The `resolve-mode` subcommand's behavior is covered in-process by
    // `resolve_mode_*` tests in `remargin-core/src/config/tests.rs`
    // against a MockSystem. The CLI smoke variant was deleted because
    // it walked up to the real `~/.remargin.yaml`.
}
