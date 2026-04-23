//! End-to-end CLI tests for the `remargin_kind` surface (rem-49w0).
//!
//! Exercises the CLI binary to prove:
//!
//! 1. `remargin comment --kind X --kind Y` writes the tags into the
//!    YAML wire format and they round-trip.
//! 2. `remargin comments --kind X` filters the single-file listing.
//! 3. `remargin query --kind X --kind Y` applies the same filter with
//!    OR semantics across a vault.
//! 4. `remargin edit --kind Z` replaces the stored list; omitting
//!    `--kind` preserves it on content-only edits.
//!
//! Tests spin up a real `remargin` binary in a tempdir so they cover
//! the same wiring path users hit.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};

    use assert_cmd::Command;
    use serde_json::Value;
    use tempfile::TempDir;

    const ALICE_CONFIG: &str = "identity: alice\ntype: human\nmode: open\n";

    /// Vault layout used by every test below.
    ///
    /// - `.remargin.yaml` (alice, open mode)
    /// - `docs/a.md` (empty managed doc)
    /// - `docs/b.md` (empty managed doc)
    fn setup_vault() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        fs::write(root.join(".remargin.yaml"), ALICE_CONFIG).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        for name in ["a.md", "b.md"] {
            // Minimal managed-doc frontmatter so `remargin comment`
            // accepts the write.
            let body = "---\ntitle: Test\n---\n\n# Hello\n";
            fs::write(root.join("docs").join(name), body).unwrap();
        }
        (tmp, root)
    }

    fn bin() -> Command {
        Command::cargo_bin("remargin").unwrap()
    }

    fn add_comment(root: &Path, file: &str, content: &str, kinds: &[&str]) -> String {
        let mut cmd = bin();
        cmd.current_dir(root)
            .arg("comment")
            .arg(file)
            .arg(content)
            .arg("--json");
        for k in kinds {
            cmd.arg("--kind").arg(k);
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "remargin comment failed: stderr={}",
            str::from_utf8(&output.stderr).unwrap_or("<non-utf8>")
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        value["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn comment_kind_flag_writes_field_into_yaml() {
        let (_guard, root) = setup_vault();
        let id = add_comment(&root, "docs/a.md", "question body", &["question"]);
        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        assert!(
            disk.contains(&format!("id: {id}")),
            "comment not written: {disk}"
        );
        assert!(
            disk.contains("remargin_kind: [question]"),
            "kind line missing: {disk}"
        );
    }

    #[test]
    fn comments_kind_filter_narrows_single_file_listing() {
        let (_guard, root) = setup_vault();
        let q_id = add_comment(&root, "docs/a.md", "a question", &["question"]);
        let _t_id = add_comment(&root, "docs/a.md", "a todo", &["todo"]);

        let output = bin()
            .current_dir(&root)
            .args(["comments", "docs/a.md", "--kind", "question", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let comments = value["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["id"].as_str().unwrap(), q_id);
    }

    #[test]
    fn query_kind_filter_applies_or_semantics_across_vault() {
        let (_guard, root) = setup_vault();
        add_comment(&root, "docs/a.md", "a question", &["question"]);
        add_comment(&root, "docs/b.md", "another todo", &["todo"]);
        add_comment(&root, "docs/a.md", "unrelated content", &[]);

        let output = bin()
            .current_dir(&root)
            .args([
                "query",
                "docs",
                "--expanded",
                "--kind",
                "question",
                "--kind",
                "todo",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "query failed: {}",
            str::from_utf8(&output.stderr).unwrap_or("<non-utf8>")
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let results = value["results"].as_array().unwrap();
        let mut ids: Vec<&str> = results
            .iter()
            .flat_map(|r| {
                r["comments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["id"].as_str().unwrap())
            })
            .collect();
        ids.sort_unstable();
        assert_eq!(ids.len(), 2, "expected one hit per --kind, got {ids:?}");
    }

    #[test]
    fn edit_without_kind_preserves_stored_list() {
        let (_guard, root) = setup_vault();
        let id = add_comment(&root, "docs/a.md", "initial body", &["question"]);

        let output = bin()
            .current_dir(&root)
            .args(["edit", "docs/a.md", &id, "rewritten body", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());

        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        assert!(disk.contains("rewritten body"), "content not updated");
        assert!(
            disk.contains("remargin_kind: [question]"),
            "kind should have been preserved on content-only edit: {disk}"
        );
    }

    #[test]
    fn edit_with_kind_replaces_the_stored_list() {
        let (_guard, root) = setup_vault();
        let id = add_comment(&root, "docs/a.md", "initial body", &["question"]);

        let output = bin()
            .current_dir(&root)
            .args([
                "edit",
                "docs/a.md",
                &id,
                "rewritten body",
                "--kind",
                "todo",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());

        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        assert!(disk.contains("remargin_kind: [todo]"));
        assert!(!disk.contains("remargin_kind: [question]"));
    }

    #[test]
    fn invalid_kind_is_rejected_before_write() {
        let (_guard, root) = setup_vault();
        let output = bin()
            .current_dir(&root)
            .args([
                "comment",
                "docs/a.md",
                "body",
                "--kind",
                "bad!value",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "invalid kind should fail");
        let stderr = str::from_utf8(&output.stderr).unwrap();
        assert!(
            stderr.contains("remargin_kind") && stderr.contains("invalid character"),
            "expected clear validation error, got: {stderr}"
        );
        // File must not have been mutated (no comment id / no kind line).
        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        assert!(
            !disk.contains("```remargin"),
            "file should be untouched: {disk}"
        );
    }
}
