//! `remargin purge` integration tests, focused on the directory form.
//!
//! The unit-test layer
//! (`crates/remargin-core/src/operations/purge/tests.rs`) covers the
//! algorithm; this surface check confirms the CLI args plumb through
//! correctly, the JSON shape is documented, and the plan projection
//! round-trips cleanly.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::Path;
    use std::process::Output;

    use assert_cmd::Command;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn assert_status(out: &Output, expected: i32) {
        let actual = out.status.code();
        assert_eq!(
            actual,
            Some(expected),
            "remargin exited with {:?}\nstdout: {}\nstderr: {}",
            actual,
            str::from_utf8(&out.stdout).unwrap(),
            str::from_utf8(&out.stderr).unwrap(),
        );
    }

    fn doc_with_one_comment() -> &'static str {
        "---\ntitle: Sample\n---\n\n# Sample\n\nBody text.\n\n```remargin\n---\nid: aaa111\nauthor: alice\ntype: human\nts: 2026-04-29T10:00:00+00:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n"
    }

    /// `remargin purge --recursive <dir>` purges every `.md` file in
    /// the directory and reports the per-file outcomes in the JSON
    /// payload.
    #[test]
    fn recursive_purge_via_cli() {
        let realm = TempDir::new().unwrap();
        fs::create_dir_all(realm.path().join("notes")).unwrap();
        fs::write(realm.path().join("a.md"), doc_with_one_comment()).unwrap();
        fs::write(realm.path().join("notes/b.md"), doc_with_one_comment()).unwrap();

        let out = run_in(realm.path(), &["purge", "--recursive", ".", "--json"]);
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(
            value["comments_removed"], 2_u64,
            "should report total across files: {value}"
        );
        let purged = value["purged"].as_array().unwrap();
        assert_eq!(purged.len(), 2);
        assert!(value["failed"].as_array().unwrap().is_empty());
        assert!(value["skipped"].as_array().unwrap().is_empty());

        // Both files now comment-free on disk.
        for file in ["a.md", "notes/b.md"] {
            let body = fs::read_to_string(realm.path().join(file)).unwrap();
            assert!(
                !body.contains("```remargin"),
                "{file} should have no remargin block: {body}"
            );
        }
    }

    /// `remargin purge <dir>` (without `--recursive`) is rejected
    /// with a clear "directory" error so the caller is forced to
    /// opt in to the destructive directory form.
    #[test]
    fn dir_target_without_recursive_errors() {
        let realm = TempDir::new().unwrap();
        fs::create_dir_all(realm.path().join("notes")).unwrap();
        fs::write(realm.path().join("notes/a.md"), doc_with_one_comment()).unwrap();

        let out = run_in(realm.path(), &["purge", "notes"]);
        assert_ne!(out.status.code(), Some(0_i32));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("directory") && stderr.contains("--recursive"),
            "expected directory-without-recursive error, got: {stderr}"
        );

        // File untouched.
        let body = fs::read_to_string(realm.path().join("notes/a.md")).unwrap();
        assert!(body.contains("```remargin"));
    }

    /// `remargin purge --recursive <missing>` fails with a clear
    /// non-zero exit so callers can distinguish "empty dir" from
    /// "missing dir".
    #[test]
    fn missing_directory_errors() {
        let realm = TempDir::new().unwrap();

        let out = run_in(realm.path(), &["purge", "--recursive", "missing-dir"]);
        assert_ne!(out.status.code(), Some(0_i32));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("does not exist"),
            "expected missing-dir error, got: {stderr}"
        );
    }

    /// `remargin plan purge --recursive <dir>` reports per-file
    /// projections without writing anything to disk.
    #[test]
    fn plan_recursive_purge_emits_purge_dir_diff() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("a.md"), doc_with_one_comment()).unwrap();
        fs::write(realm.path().join("b.md"), doc_with_one_comment()).unwrap();

        let out = run_in(
            realm.path(),
            &["plan", "purge", "--recursive", ".", "--json"],
        );
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["op"], "purge");
        assert_eq!(value["would_commit"], json!(true));
        assert_eq!(value["noop"], json!(false));

        let diff = &value["purge_dir_diff"];
        assert!(diff.is_object(), "purge_dir_diff missing: {value}");
        let files = diff["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        for file in files {
            assert_eq!(file["outcome"], "would_purge");
            assert_eq!(file["comments_removed"], 1_u64);
        }

        // Plan must not have removed comments from disk.
        for file in ["a.md", "b.md"] {
            let body = fs::read_to_string(realm.path().join(file)).unwrap();
            assert!(
                body.contains("```remargin"),
                "plan must not write {file}: {body}"
            );
        }
    }

    /// Empty / zero-md directory is a successful no-op exit 0.
    #[test]
    fn empty_dir_recursive_purge_succeeds() {
        let realm = TempDir::new().unwrap();

        let out = run_in(realm.path(), &["purge", "--recursive", ".", "--json"]);
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["comments_removed"], 0_u64);
        assert!(value["purged"].as_array().unwrap().is_empty());
    }
}
