//! End-to-end tests for `remargin get --binary` (rem-cdr).
//!
//! Verifies the three binary-mode output shapes described in the task:
//! - Default (no `--out`, no `--json`): raw bytes to stdout.
//! - `--json`: base64 payload alongside `mime`, `size_bytes`, `path`.
//! - `--out <path>`: bytes written to the target file; stdout gets a summary.
//!
//! Also covers the `.md` rejection symmetry with `write --binary`.

#[cfg(test)]
mod tests {
    use assert_cmd::Command;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use std::fs;
    use tempfile::TempDir;

    const FAKE_PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0xde, 0xad, 0xbe, 0xef,
    ];

    fn write_fixture(dir: &TempDir, name: &str, bytes: &[u8]) {
        fs::write(dir.path().join(name), bytes).unwrap();
    }

    #[test]
    fn binary_get_raw_bytes_to_stdout() {
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "pic.png", FAKE_PNG);

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["get", "--binary", "pic.png"])
            .output()
            .unwrap();

        assert!(output.status.success(), "command failed: {output:?}");
        assert_eq!(output.stdout, FAKE_PNG);
    }

    #[test]
    fn binary_get_json_returns_base64() {
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "pic.png", FAKE_PNG);

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["get", "--binary", "pic.png", "--json"])
            .output()
            .unwrap();

        assert!(output.status.success(), "command failed: {output:?}");

        let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(payload["binary"], true);
        assert_eq!(payload["mime"], "image/png");
        let encoded = payload["content"].as_str().unwrap();
        let decoded = BASE64_STANDARD.decode(encoded).unwrap();
        assert_eq!(decoded, FAKE_PNG);
    }

    #[test]
    fn binary_get_with_out_writes_file() {
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "pic.png", FAKE_PNG);
        let out = tmp.path().join("copy.png");

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args([
                "get",
                "--binary",
                "--out",
                out.to_str().unwrap(),
                "pic.png",
                "--json",
            ])
            .output()
            .unwrap();

        assert!(output.status.success(), "command failed: {output:?}");

        // The file on disk is byte-identical to the source.
        let written = fs::read(&out).unwrap();
        assert_eq!(written, FAKE_PNG);

        // Stdout summary in --json mode carries the metadata, NOT the bytes.
        let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(payload["mime"], "image/png");
        assert!(payload.get("content").is_none());
    }

    #[test]
    fn binary_get_rejects_markdown() {
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "doc.md", b"# hi\n");

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["get", "--binary", "doc.md"])
            .output()
            .unwrap();

        assert!(!output.status.success(), "expected failure: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot fetch .md as binary"),
            "unexpected stderr: {stderr}"
        );
    }

    #[test]
    fn out_without_binary_errors() {
        let tmp = TempDir::new().unwrap();
        write_fixture(&tmp, "doc.md", b"# hi\n");

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["get", "--out", "copy.md", "doc.md"])
            .output()
            .unwrap();

        assert!(!output.status.success(), "expected failure: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--out requires --binary"),
            "unexpected stderr: {stderr}"
        );
    }
}
