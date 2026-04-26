//! Regression test for rem-1are: `remargin comment <file> "- bullet"`
//! must accept content that starts with a hyphen.
//!
//! Clap rejects positional values starting with `-` by default. The
//! `comment` subcommand annotates `content` with `allow_hyphen_values`
//! so markdown bullets (and any other dash-led prose) round-trip.

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

    fn setup_vault() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        fs::write(root.join(".remargin.yaml"), ALICE_CONFIG).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        let body = "---\ntitle: Test\n---\n\n# Hello\n";
        fs::write(root.join("docs/a.md"), body).unwrap();
        (tmp, root)
    }

    fn run_comment(root: &Path, file: &str, content: &str) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(root)
            .arg("comment")
            .arg(file)
            .arg(content)
            .arg("--json")
            .output()
            .unwrap()
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "test: a missing JSON id should fail loudly with context"
    )]
    fn comment_accepts_content_starting_with_hyphen() {
        let (_guard, root) = setup_vault();
        let body = "- About comment.created\n  - is_reply and reply_to are redundant";
        let output = run_comment(&root, "docs/a.md", body);
        assert!(
            output.status.success(),
            "remargin comment with leading hyphen failed: stderr={}",
            str::from_utf8(&output.stderr).unwrap_or("<non-utf8>")
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let id = value["id"].as_str().expect("comment id missing from JSON");
        let disk = fs::read_to_string(root.join("docs/a.md")).unwrap();
        assert!(
            disk.contains(&format!("id: {id}")),
            "comment not written: {disk}"
        );
        assert!(
            disk.contains("About comment.created"),
            "comment body not persisted: {disk}"
        );
    }

    #[test]
    fn comment_accepts_content_that_is_just_a_hyphen_bullet() {
        let (_guard, root) = setup_vault();
        let output = run_comment(&root, "docs/a.md", "- lone bullet");
        assert!(
            output.status.success(),
            "remargin comment with bare bullet failed: stderr={}",
            str::from_utf8(&output.stderr).unwrap_or("<non-utf8>")
        );
    }
}
