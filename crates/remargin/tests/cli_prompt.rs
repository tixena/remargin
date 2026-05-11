//! `remargin prompt resolve` integration tests.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;

    use assert_cmd::Command;
    use tempfile::TempDir;

    fn write_yaml(tmp: &TempDir, rel: &str, body: &str) {
        let path = tmp.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn resolve_text_mode_names_prompt_and_source() {
        let tmp = TempDir::new().unwrap();
        write_yaml(
            &tmp,
            ".remargin.yaml",
            "system_prompt:\n  name: SWE reviewer\n  prompt: review this carefully\n",
        );
        fs::write(tmp.path().join("doc.md"), "# Hi\n").unwrap();

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .arg("prompt")
            .arg("resolve")
            .arg("doc.md")
            .output()
            .unwrap();
        assert!(output.status.success(), "command failed: {output:?}");
        let stderr = str::from_utf8(&output.stderr).unwrap();
        assert!(stderr.contains("SWE reviewer"), "stderr: {stderr}");
        assert!(stderr.contains(".remargin.yaml"), "stderr: {stderr}");
        assert!(stderr.contains("Default: no"), "stderr: {stderr}");
    }

    #[test]
    fn resolve_json_mode_carries_elapsed_ms_and_fields() {
        let tmp = TempDir::new().unwrap();
        write_yaml(
            &tmp,
            ".remargin.yaml",
            "system_prompt:\n  name: docs\n  prompt: do the thing\n",
        );
        fs::write(tmp.path().join("doc.md"), "# Hi\n").unwrap();

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .arg("prompt")
            .arg("resolve")
            .arg("doc.md")
            .arg("--json")
            .output()
            .unwrap();
        assert!(output.status.success(), "command failed: {output:?}");
        let stdout = str::from_utf8(&output.stdout).unwrap();
        let json: serde_json::Value = serde_json::from_str(stdout).unwrap();
        assert_eq!(json["name"], "docs");
        assert_eq!(json["prompt"], "do the thing");
        assert_eq!(json["is_default"], false);
        assert!(json["elapsed_ms"].is_number(), "missing elapsed_ms");
        assert!(json["source"].is_string());
    }

    #[test]
    fn resolve_in_unconfigured_tree_returns_default() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("doc.md"), "# Hi\n").unwrap();

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .arg("prompt")
            .arg("resolve")
            .arg("doc.md")
            .output()
            .unwrap();
        assert!(output.status.success(), "command failed: {output:?}");
        let stderr = str::from_utf8(&output.stderr).unwrap();
        assert!(stderr.contains("walk exhausted"), "stderr: {stderr}");
        assert!(stderr.contains("default"), "stderr: {stderr}");
    }

    #[test]
    fn resolve_in_strict_realm_without_key_still_works() {
        let tmp = TempDir::new().unwrap();
        write_yaml(
            &tmp,
            ".remargin.yaml",
            "mode: strict\nsystem_prompt:\n  name: strict-realm\n  prompt: stay strict\n",
        );
        fs::write(tmp.path().join("doc.md"), "# Hi\n").unwrap();

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .arg("prompt")
            .arg("resolve")
            .arg("doc.md")
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "strict realm resolve should not require a key: {output:?}"
        );
        let stdout = str::from_utf8(&output.stdout).unwrap();
        let json: serde_json::Value = serde_json::from_str(stdout).unwrap();
        assert_eq!(json["name"], "strict-realm");
    }

    #[test]
    fn resolve_nonexistent_file_uses_parent_walk() {
        let tmp = TempDir::new().unwrap();
        write_yaml(
            &tmp,
            ".remargin.yaml",
            "system_prompt:\n  name: parent\n  prompt: body\n",
        );

        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .arg("prompt")
            .arg("resolve")
            .arg("missing.md")
            .arg("--json")
            .output()
            .unwrap();
        assert!(output.status.success(), "command failed: {output:?}");
        let stdout = str::from_utf8(&output.stdout).unwrap();
        let json: serde_json::Value = serde_json::from_str(stdout).unwrap();
        assert_eq!(json["name"], "parent");
        assert_eq!(json["is_default"], false);
    }
}
