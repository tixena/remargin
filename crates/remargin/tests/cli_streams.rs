//! Regression tests for rem-1jy / rem-26w: non-JSON mode emits no
//! `elapsed` footer on any stream; the timing value only survives as
//! the `elapsed_ms` key inside the JSON payload.
//!
//! The scenarios use `remargin resolve-mode` because it is a read-only
//! command that needs no sandbox or filesystem fixture, which keeps the
//! test hermetic.

#[cfg(test)]
mod tests {
    use core::str;

    use assert_cmd::Command;

    /// `remargin resolve-mode` (non-JSON) must not emit an `elapsed:`
    /// line on stdout *or* stderr. stdout stays pure command output and
    /// stderr carries only command diagnostics / errors.
    #[test]
    fn non_json_mode_emits_no_elapsed_footer() {
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
            !stderr.contains("elapsed:"),
            "elapsed footer leaked onto stderr: {stderr:?}"
        );
    }

    /// `--json` mode carries the timing value inside the JSON payload
    /// as `elapsed_ms` and emits no plaintext footer on either stream.
    #[test]
    fn json_mode_carries_elapsed_in_payload_only() {
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .arg("resolve-mode")
            .arg("--json")
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
}
