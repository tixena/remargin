//! `remargin plan {ack,batch,comment,react,sign,sandbox-add,sandbox-remove}` smoke tests.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;

    use assert_cmd::Command;
    use tempfile::TempDir;

    const FIXTURE_DOC: &str = "---
title: Plan fixture
---

# Heading

```remargin
---
id: aaa
author: parity-bot
type: agent
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:3e5121224e71bb75be3d2a2ac568d2117b6cd3aa10a54f7abc9b19cdb1976b2e
---
Seed comment.
```
";

    fn seed_tmp() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("doc.md"), FIXTURE_DOC).unwrap();
        fs::write(
            tmp.path().join(".remargin.yaml"),
            "mode: open\nidentity: parity-bot\ntype: agent\n",
        )
        .unwrap();
        tmp
    }

    fn plan(tmp: &TempDir, args: &[&str]) -> serde_json::Value {
        let mut full: Vec<&str> = args.to_vec();
        full.push("--json");
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .args(&full)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "plan failed for args {full:?}: stdout={:?} stderr={:?}",
            str::from_utf8(&output.stdout).unwrap_or(""),
            str::from_utf8(&output.stderr).unwrap_or("")
        );
        let stdout = str::from_utf8(&output.stdout).unwrap();
        serde_json::from_str(stdout).unwrap()
    }

    #[test]
    fn plan_ack_emits_ack_op_label() {
        let tmp = seed_tmp();
        let report = plan(&tmp, &["plan", "ack", "doc.md", "aaa"]);
        assert_eq!(report["op"], "ack");
    }

    #[test]
    fn plan_sign_without_key_reports_no_signing_key() {
        let tmp = seed_tmp();
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .args(["plan", "sign", "doc.md", "--all-mine", "--json"])
            .output()
            .unwrap();
        let stderr = str::from_utf8(&output.stderr).unwrap();
        assert!(
            stderr.contains("no signing key resolved"),
            "expected key-resolution diagnostic; got: {stderr:?}"
        );
    }

    #[test]
    fn plan_react_emits_react_op_label() {
        let tmp = seed_tmp();
        let report = plan(&tmp, &["plan", "react", "doc.md", "aaa", "thumbsup"]);
        assert_eq!(report["op"], "react");
    }

    #[test]
    fn plan_sandbox_add_emits_sandbox_add_op_label() {
        let tmp = seed_tmp();
        let report = plan(&tmp, &["plan", "sandbox-add", "doc.md"]);
        assert_eq!(report["op"], "sandbox-add");
    }

    #[test]
    fn plan_sandbox_remove_emits_sandbox_remove_op_label() {
        let tmp = seed_tmp();
        let report = plan(&tmp, &["plan", "sandbox-remove", "doc.md"]);
        assert_eq!(report["op"], "sandbox-remove");
    }

    #[test]
    fn plan_batch_with_ops_file_emits_batch_op_label() {
        let tmp = seed_tmp();
        let ops_json = r#"[{"content":"first body"},{"content":"second body"}]"#;
        let ops_path = tmp.path().join("ops.json");
        fs::write(&ops_path, ops_json).unwrap();
        let ops_str = ops_path.to_str().unwrap();
        let report = plan(&tmp, &["plan", "batch", "doc.md", ops_str]);
        assert_eq!(report["op"], "batch");
    }

    #[test]
    fn plan_comment_emits_comment_op_label() {
        let tmp = seed_tmp();
        let report = plan(&tmp, &["plan", "comment", "doc.md", "Hello body for plan."]);
        assert_eq!(report["op"], "comment");
    }
}
