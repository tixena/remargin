//! End-to-end CLI tests for `--after-heading` (rem-5oqx).
//!
//! Covers the singular `comment` subcommand, the `batch` subcommand,
//! and the clap-level mutual-exclusion guard. Uses `assert_cmd` against
//! the real binary so the wiring through main.rs is exercised end to
//! end (clap → resolver → writer → on-disk markdown).

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Output;

    use assert_cmd::Command;
    use serde_json::Value;
    use tempfile::TempDir;

    const ALICE_CONFIG: &str = "identity: alice\ntype: human\nmode: open\n";

    /// A short multi-section doc with two `## A10.` / `## P11.` style
    /// sub-headings under different `#` parents: matches the path-syntax
    /// disambiguation case in the rem-5oqx test plan.
    const HEADINGS_DOC: &str = "\
---
title: Headings
---

# Activity epic tests

## A10. MCP / CLI parity

Body for A10.

# Permissions epic tests

## P11. MCP / CLI parity

Body for P11.

## P3. deny_ops

Body for P3.
";

    fn setup_vault(doc: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        fs::write(root.join(".remargin.yaml"), ALICE_CONFIG).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/a.md"), doc).unwrap();
        (tmp, root)
    }

    fn run_comment_after_heading(root: &Path, file: &str, body: &str, heading: &str) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(root)
            .arg("comment")
            .arg(file)
            .arg(body)
            .arg("--after-heading")
            .arg(heading)
            .arg("--json")
            .output()
            .unwrap()
    }

    #[test]
    fn singular_comment_after_heading_prefix_match() {
        let (_guard, root) = setup_vault(HEADINGS_DOC);
        let out = run_comment_after_heading(&root, "docs/a.md", "anchored at P3", "P3.");
        assert!(
            out.status.success(),
            "stderr={}",
            str::from_utf8(&out.stderr).unwrap_or("<non-utf8>")
        );
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let id = v["id"].as_str().unwrap();

        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        let lines: Vec<&str> = disk.lines().collect();
        let p3_line = lines
            .iter()
            .position(|l| l.trim_start().starts_with("## P3."))
            .unwrap();
        let id_line = lines
            .iter()
            .position(|l| l.contains(&format!("id: {id}")))
            .unwrap();
        assert!(id_line > p3_line);
    }

    #[test]
    fn singular_comment_after_heading_path_disambiguates() {
        let (_guard, root) = setup_vault(HEADINGS_DOC);
        let out = run_comment_after_heading(
            &root,
            "docs/a.md",
            "anchored at Activity > A10",
            "Activity epic tests > A10.",
        );
        assert!(
            out.status.success(),
            "stderr={}",
            str::from_utf8(&out.stderr).unwrap_or("<non-utf8>")
        );

        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let id = v["id"].as_str().unwrap();
        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        let lines: Vec<&str> = disk.lines().collect();
        let a10 = lines
            .iter()
            .position(|l| l.trim_start().starts_with("## A10."))
            .unwrap();
        let p11 = lines
            .iter()
            .position(|l| l.trim_start().starts_with("## P11."))
            .unwrap();
        let id_line = lines
            .iter()
            .position(|l| l.contains(&format!("id: {id}")))
            .unwrap();
        assert!(id_line > a10);
        assert!(id_line < p11);
    }

    #[test]
    fn singular_comment_after_heading_no_match_errors() {
        let (_guard, root) = setup_vault(HEADINGS_DOC);
        let before = fs::read_to_string(root.join("docs/a.md")).unwrap();
        let out = run_comment_after_heading(&root, "docs/a.md", "no anchor", "Z9. nonexistent");
        assert!(!out.status.success());
        let after = fs::read_to_string(root.join("docs/a.md")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn singular_comment_after_heading_conflicts_with_after_line() {
        let (_guard, root) = setup_vault(HEADINGS_DOC);
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(&root)
            .arg("comment")
            .arg("docs/a.md")
            .arg("body")
            .arg("--after-heading")
            .arg("P3.")
            .arg("--after-line")
            .arg("3")
            .arg("--json")
            .output()
            .unwrap();
        assert!(!out.status.success());
        let stderr = str::from_utf8(&out.stderr).unwrap_or("");
        assert!(
            stderr.contains("cannot be used") || stderr.contains("conflict"),
            "expected clap conflict error, got: {stderr}"
        );
    }

    #[test]
    fn batch_with_after_heading_per_op() {
        let (_guard, root) = setup_vault(HEADINGS_DOC);
        let ops = serde_json::json!([
            { "content": "after A10",
              "after_heading": "Activity epic tests > A10." },
            { "content": "after P3",
              "after_heading": "P3." }
        ]);
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(&root)
            .arg("batch")
            .arg("docs/a.md")
            .arg("--ops")
            .arg(ops.to_string())
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stderr={}",
            str::from_utf8(&out.stderr).unwrap_or("<non-utf8>")
        );
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let ids: Vec<String> = v["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| String::from(e.as_str().unwrap()))
            .collect();
        assert_eq!(ids.len(), 2);

        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        let lines: Vec<&str> = disk.lines().collect();
        let a10 = lines
            .iter()
            .position(|l| l.trim_start().starts_with("## A10."))
            .unwrap();
        let p3 = lines
            .iter()
            .position(|l| l.trim_start().starts_with("## P3."))
            .unwrap();
        let id0 = lines
            .iter()
            .position(|l| l.contains(&format!("id: {}", ids[0])))
            .unwrap();
        let id1 = lines
            .iter()
            .position(|l| l.contains(&format!("id: {}", ids[1])))
            .unwrap();
        assert!(id0 > a10);
        assert!(id1 > p3);
    }

    #[test]
    fn batch_rejects_multi_anchor_op_without_writing() {
        let (_guard, root) = setup_vault(HEADINGS_DOC);
        let before = fs::read_to_string(root.join("docs/a.md")).unwrap();
        let ops = serde_json::json!([
            { "content": "x", "after_heading": "P3.", "after_line": 5_i32 }
        ]);
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(&root)
            .arg("batch")
            .arg("docs/a.md")
            .arg("--ops")
            .arg(ops.to_string())
            .arg("--json")
            .output()
            .unwrap();
        assert!(!out.status.success());
        let after = fs::read_to_string(root.join("docs/a.md")).unwrap();
        assert_eq!(before, after);
    }
}
