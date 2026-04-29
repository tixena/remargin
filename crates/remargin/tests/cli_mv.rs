//! `remargin mv` integration tests (rem-0j2x / T44).
//!
//! Exercises the CLI subcommand against real-filesystem temp dirs.
//! The unit-test layer (`crates/remargin-core/src/operations/mv/tests.rs`)
//! covers the algorithm; this surface check confirms the CLI args
//! plumb through to it correctly, the JSON shape is documented, and
//! the plan projection round-trips cleanly.

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

    /// Same-directory rename produces the expected on-disk state and
    /// reports a non-zero `bytes_moved` in JSON mode.
    #[test]
    fn renames_within_same_dir_via_cli() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("a.md"), b"hello\n").unwrap();

        let out = run_in(realm.path(), &["mv", "a.md", "b.md", "--json"]);
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["bytes_moved"], 6_u64);
        assert_eq!(value["overwritten"], json!(false));
        assert_eq!(value["fallback_copy"], json!(false));
        assert_eq!(value["noop_same_path"], json!(false));

        assert!(!realm.path().join("a.md").exists());
        let body = fs::read_to_string(realm.path().join("b.md")).unwrap();
        assert_eq!(body, "hello\n");
    }

    /// Cross-directory move keeps the bytes intact.
    #[test]
    fn moves_across_directories_via_cli() {
        let realm = TempDir::new().unwrap();
        fs::create_dir_all(realm.path().join("notes")).unwrap();
        fs::create_dir_all(realm.path().join("archive")).unwrap();
        fs::write(realm.path().join("notes/foo.md"), b"x").unwrap();

        let out = run_in(
            realm.path(),
            &["mv", "notes/foo.md", "archive/foo.md", "--json"],
        );
        assert_status(&out, 0);

        assert!(!realm.path().join("notes/foo.md").exists());
        assert!(realm.path().join("archive/foo.md").exists());
    }

    /// `--force` overwrites an existing destination; the destination
    /// content matches the source after the call.
    #[test]
    fn force_overwrites_destination_via_cli() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("a.md"), b"new\n").unwrap();
        fs::write(realm.path().join("b.md"), b"old\n").unwrap();

        let no_force = run_in(realm.path(), &["mv", "a.md", "b.md"]);
        assert_ne!(no_force.status.code(), Some(0_i32));
        let stderr = String::from_utf8_lossy(&no_force.stderr);
        assert!(
            stderr.contains("destination exists"),
            "expected destination-exists refusal, got: {stderr}"
        );

        // Source still in place after the refusal.
        assert!(realm.path().join("a.md").exists());

        let with_force = run_in(realm.path(), &["mv", "a.md", "b.md", "--force", "--json"]);
        assert_status(&with_force, 0);

        let value: Value =
            serde_json::from_str(str::from_utf8(&with_force.stdout).unwrap()).unwrap();
        assert_eq!(value["overwritten"], json!(true));
        assert_eq!(
            fs::read_to_string(realm.path().join("b.md")).unwrap(),
            "new\n"
        );
    }

    /// Same-path no-op reports `noop_same_path` and leaves the file
    /// alone.
    #[test]
    fn same_path_is_noop_via_cli() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("a.md"), b"unchanged").unwrap();

        let out = run_in(realm.path(), &["mv", "a.md", "a.md", "--json"]);
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["noop_same_path"], json!(true));
        assert_eq!(
            fs::read_to_string(realm.path().join("a.md")).unwrap(),
            "unchanged"
        );
    }

    /// Idempotent re-run: when the source is missing AND the
    /// destination already exists, the op succeeds with `bytes_moved
    /// == 0`. Lets retried `mv` calls settle cleanly.
    #[test]
    fn idempotent_when_already_settled_via_cli() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("b.md"), b"already moved").unwrap();

        let out = run_in(realm.path(), &["mv", "a.md", "b.md", "--json"]);
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["bytes_moved"], 0_u64);
        assert_eq!(value["overwritten"], json!(false));
        assert_eq!(value["noop_same_path"], json!(false));
    }

    /// Refuses moves whose source is not visible (sandbox escape).
    #[test]
    fn refuses_path_escape_via_cli() {
        let realm = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("escape.md"), b"x").unwrap();

        let out = run_in(
            realm.path(),
            &[
                "mv",
                outside.path().join("escape.md").to_str().unwrap(),
                "b.md",
            ],
        );
        assert_ne!(out.status.code(), Some(0_i32));
    }

    /// Comments + frontmatter survive a CLI-driven rename: the moved
    /// document parses cleanly with the same comment id and content
    /// checksum it had at the source.
    #[test]
    fn preserves_comments_and_frontmatter_across_rename() {
        // Hand-rolled fixture with a single signed-shape comment block.
        // The exact checksum value here is whatever the parser would
        // report; we capture it from the source file before the move
        // and assert byte-equality after.
        let realm = TempDir::new().unwrap();
        let source = "---\ntitle: Sample\n---\n\n# Sample\n\nBody text.\n\n```remargin\n---\nid: aaa111\nauthor: alice\ntype: human\nts: 2026-04-29T10:00:00+00:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n";
        fs::write(realm.path().join("src.md"), source).unwrap();

        let out = run_in(realm.path(), &["mv", "src.md", "dst.md"]);
        assert_status(&out, 0);

        let after = fs::read_to_string(realm.path().join("dst.md")).unwrap();
        assert_eq!(
            after, source,
            "comments + frontmatter must survive byte-for-byte"
        );
    }

    /// `remargin plan mv` projects the move without touching the
    /// filesystem and emits the documented `mv_diff` shape with
    /// `would_commit = true`.
    #[test]
    fn plan_mv_emits_mv_diff() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("a.md"), b"plan me").unwrap();

        let out = run_in(realm.path(), &["plan", "mv", "a.md", "b.md", "--json"]);
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["op"], "mv");
        assert_eq!(value["would_commit"], json!(true));
        assert_eq!(value["noop"], json!(false));
        let mv_diff = &value["mv_diff"];
        assert!(mv_diff.is_object(), "mv_diff missing: {value}");
        assert_eq!(mv_diff["dst_exists"], json!(false));
        assert_eq!(mv_diff["src_exists"], json!(true));
        assert_eq!(mv_diff["noop_same_path"], json!(false));
        assert_eq!(mv_diff["idempotent_already_settled"], json!(false));

        // Plan must NOT have moved the file.
        assert!(realm.path().join("a.md").exists());
        assert!(!realm.path().join("b.md").exists());
    }

    /// `remargin plan mv` against an existing destination without
    /// `--force` flips `would_commit = false` and surfaces the
    /// destination-exists message in `reject_reason`.
    #[test]
    fn plan_mv_rejects_existing_destination_without_force() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("a.md"), b"src").unwrap();
        fs::write(realm.path().join("b.md"), b"dst").unwrap();

        let out = run_in(realm.path(), &["plan", "mv", "a.md", "b.md", "--json"]);
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["would_commit"], json!(false));
        assert!(
            value["reject_reason"]
                .as_str()
                .is_some_and(|s| s.contains("destination exists")),
            "missing destination-exists reject reason: {value}"
        );
    }

    /// `remargin plan mv --force` against an existing destination
    /// flips `would_commit = true` because the projection now mirrors
    /// the live `--force` behaviour.
    #[test]
    fn plan_mv_force_clears_existing_destination_rejection() {
        let realm = TempDir::new().unwrap();
        fs::write(realm.path().join("a.md"), b"src").unwrap();
        fs::write(realm.path().join("b.md"), b"dst").unwrap();

        let out = run_in(
            realm.path(),
            &["plan", "mv", "a.md", "b.md", "--force", "--json"],
        );
        assert_status(&out, 0);

        let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(value["would_commit"], json!(true));
        assert_eq!(value["mv_diff"]["dst_exists"], json!(true));
    }

    /// `remargin restrict` emits the new source-side `mv` deny
    /// patterns into the project-scope settings file (rem-0j2x / T44).
    #[test]
    fn restrict_emits_source_side_mv_denies() {
        let realm = TempDir::new().unwrap();
        fs::create_dir_all(realm.path().join(".claude")).unwrap();
        fs::create_dir_all(realm.path().join("src/secret")).unwrap();
        let user_settings = realm.path().join("hermetic-user-settings.json");

        let out = run_in(
            realm.path(),
            &[
                "restrict",
                "src/secret",
                "--user-settings",
                user_settings.to_str().unwrap(),
            ],
        );
        assert_status(&out, 0);

        let project_scope = realm.path().join(".claude/settings.local.json");
        let body = fs::read_to_string(&project_scope).unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        let deny: Vec<String> = value["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| String::from(v.as_str().unwrap()))
            .collect();

        // Existing destination-side rule.
        assert!(
            deny.iter()
                .any(|r| r.starts_with("Bash(mv * ") && r.ends_with("/**)"))
        );
        // New source-side rules (rem-0j2x).
        assert!(
            deny.iter()
                .any(|r| r.contains("Bash(mv ") && r.ends_with("/**)") && !r.contains("* ")),
            "expected `Bash(mv <path>/**)` rule, got: {deny:#?}"
        );
        assert!(
            deny.iter()
                .any(|r| r.contains("Bash(mv ") && r.contains("/** *)")),
            "expected source-side `Bash(mv <path>/** *)` rule, got: {deny:#?}"
        );
        assert!(
            deny.iter().any(|r| {
                let opens = r.matches("/**").count();
                opens >= 2 && r.starts_with("Bash(mv ")
            }),
            "expected both-sides `Bash(mv <path>/** <path>/**)` rule, got: {deny:#?}"
        );
    }
}
