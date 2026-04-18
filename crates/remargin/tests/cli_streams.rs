//! Regression tests for rem-1jy: the `elapsed: Xms` footer lives on stderr
//! (never on stdout) in non-JSON mode, and `--quiet` suppresses it entirely.
//!
//! The scenarios use `remargin resolve-mode` because it is a read-only
//! command that needs no sandbox or filesystem fixture, which keeps the
//! test hermetic.

#[cfg(test)]
mod tests {
    use core::str;

    use assert_cmd::Command;

    /// `remargin resolve-mode` (non-JSON) must keep stdout completely free
    /// of the `elapsed:` footer. The footer belongs on stderr.
    #[test]
    fn elapsed_footer_stays_off_stdout() {
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .arg("resolve-mode")
            .output()
            .unwrap();

        assert!(output.status.success(), "command failed: {output:?}");

        let stdout = str::from_utf8(&output.stdout).unwrap();
        let stderr = str::from_utf8(&output.stderr).unwrap();

        assert!(
            !stdout.contains("elapsed:"),
            "elapsed footer leaked onto stdout: {stdout:?}"
        );
        assert!(
            stderr.contains("elapsed:"),
            "elapsed footer missing from stderr: {stderr:?}"
        );
    }

    /// `--json` mode must not print a plaintext `elapsed:` line on either
    /// stream; the value is carried inside the JSON payload instead.
    #[test]
    fn elapsed_footer_absent_in_json_mode() {
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .arg("--json")
            .arg("resolve-mode")
            .output()
            .unwrap();

        assert!(output.status.success(), "command failed: {output:?}");

        let stdout = str::from_utf8(&output.stdout).unwrap();
        let stderr = str::from_utf8(&output.stderr).unwrap();

        assert!(
            !stderr.contains("elapsed:"),
            "plaintext elapsed footer leaked into --json stderr: {stderr:?}"
        );
        assert!(
            stdout.contains("\"elapsed_ms\""),
            "JSON payload missing elapsed_ms key: {stdout:?}"
        );
    }

    /// `--quiet` suppresses the stderr footer so scripted callers can
    /// redirect stderr without capturing the timing line.
    #[test]
    fn quiet_flag_suppresses_elapsed_footer() {
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .arg("--quiet")
            .arg("resolve-mode")
            .output()
            .unwrap();

        assert!(output.status.success(), "command failed: {output:?}");

        let stdout = str::from_utf8(&output.stdout).unwrap();
        let stderr = str::from_utf8(&output.stderr).unwrap();

        assert!(
            !stderr.contains("elapsed:"),
            "--quiet did not suppress elapsed footer on stderr: {stderr:?}"
        );
        assert!(
            !stdout.contains("elapsed:"),
            "stdout unexpectedly contained elapsed footer: {stdout:?}"
        );
    }
}
