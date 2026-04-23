//! End-to-end CLI tests for the new `--pending-for-me` /
//! `--pending-broadcast` flags and the `--pending` broadcast-inclusion
//! bug fix (rem-4j91).
//!
//! These tests exercise the binary (not just core) to prove the CLI
//! adapter wires the flags through and picks up the caller's identity
//! from `.remargin.yaml`.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};

    use assert_cmd::Command;
    use serde_json::Value;
    use tempfile::TempDir;

    const ALICE_CONFIG: &str = "identity: alice\ntype: human\nmode: open\n";
    const BOB_CONFIG: &str = "identity: bob\ntype: human\nmode: open\n";

    /// Doc with a fresh broadcast comment (no `to`, no acks).
    const BROADCAST_DOC: &str = "\
---
title: Broadcast
---

```remargin
---
id: bcast1
author: bot
type: agent
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:bc1
---
Fresh broadcast, no acks.
```
";

    /// Doc with a directed comment to alice (unacked) AND a broadcast
    /// (unacked).
    const MIXED_DOC: &str = "\
---
title: Mixed
---

```remargin
---
id: bcast2
author: bot
type: agent
ts: 2026-04-06T09:00:00-04:00
checksum: sha256:bc2
---
Broadcast, no acks.
```

```remargin
---
id: dir_alice
author: bob
type: human
ts: 2026-04-06T10:00:00-04:00
to: [alice]
checksum: sha256:da
---
Directed to alice, no ack.
```

```remargin
---
id: dir_bob
author: alice
type: human
ts: 2026-04-06T10:30:00-04:00
to: [bob]
checksum: sha256:db
---
Directed to bob, no ack.
```
";

    fn run_json(cwd: &Path, args: &[&str]) -> Value {
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "remargin {args:?} failed\nstderr: {}\nstdout: {}",
            str::from_utf8(&out.stderr).unwrap(),
            str::from_utf8(&out.stdout).unwrap(),
        );
        let stdout = str::from_utf8(&out.stdout).unwrap();
        serde_json::from_str(stdout).unwrap()
    }

    fn seed(path: &Path, name: &str, body: &str) -> PathBuf {
        let p = path.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    fn setup_realm(config: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".remargin.yaml"), config).unwrap();
        let path = tmp.path().to_path_buf();
        (tmp, path)
    }

    #[test]
    fn pending_flag_includes_broadcast_rem_4j91() {
        // Before rem-4j91: --pending silently excluded broadcasts.
        // After: a fresh broadcast (empty `to`, no acks) must surface.
        let (_tmp, cwd) = setup_realm(ALICE_CONFIG);
        seed(&cwd, "broadcast.md", BROADCAST_DOC);

        let result = run_json(&cwd, &["query", ".", "--pending", "--json"]);
        let results = result["results"].as_array().unwrap();
        assert_eq!(
            results.len(),
            1,
            "--pending must surface the unacked broadcast"
        );
        assert_eq!(results[0]["pending_count"].as_u64().unwrap(), 1);
    }

    #[test]
    fn pending_for_me_surfaces_directed_to_caller() {
        let (_tmp, cwd) = setup_realm(ALICE_CONFIG);
        seed(&cwd, "mixed.md", MIXED_DOC);

        let result = run_json(&cwd, &["query", ".", "--pending-for-me", "--json"]);
        let results = result["results"].as_array().unwrap();
        assert_eq!(results.len(), 1, "expected one matching file");
        let comments = results[0]["comments"].as_array().unwrap();
        let ids: Vec<&str> = comments.iter().map(|c| c["id"].as_str().unwrap()).collect();
        // Alice is named in dir_alice.to but NOT dir_bob.to; broadcast
        // does not count for --pending-for-me.
        assert_eq!(ids, vec!["dir_alice"]);
    }

    #[test]
    fn pending_for_me_as_bob_surfaces_only_bobs_comment() {
        let (_tmp, cwd) = setup_realm(BOB_CONFIG);
        seed(&cwd, "mixed.md", MIXED_DOC);

        let result = run_json(&cwd, &["query", ".", "--pending-for-me", "--json"]);
        let results = result["results"].as_array().unwrap();
        let ids: Vec<&str> = results[0]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["dir_bob"]);
    }

    #[test]
    fn pending_broadcast_surfaces_only_broadcasts() {
        let (_tmp, cwd) = setup_realm(ALICE_CONFIG);
        seed(&cwd, "mixed.md", MIXED_DOC);

        let result = run_json(&cwd, &["query", ".", "--pending-broadcast", "--json"]);
        let results = result["results"].as_array().unwrap();
        let ids: Vec<&str> = results[0]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["bcast2"]);
    }

    #[test]
    fn pending_for_me_and_broadcast_compose_as_union() {
        let (_tmp, cwd) = setup_realm(ALICE_CONFIG);
        seed(&cwd, "mixed.md", MIXED_DOC);

        let result = run_json(
            &cwd,
            &[
                "query",
                ".",
                "--pending-for-me",
                "--pending-broadcast",
                "--json",
            ],
        );
        let results = result["results"].as_array().unwrap();
        let mut ids: Vec<&str> = results[0]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["bcast2", "dir_alice"]);
    }

    #[test]
    fn pending_for_me_errors_without_identity() {
        // A blank config dir has no identity. --pending-for-me should
        // return a clear error rather than silently dropping the flag.
        let tmp = TempDir::new().unwrap();
        // Intentionally no .remargin.yaml.
        seed(tmp.path(), "mixed.md", MIXED_DOC);

        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["query", ".", "--pending-for-me", "--json"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "must error when identity missing");
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("pending_for_me") || stderr.contains("identity"),
            "stderr should mention missing identity: {stderr}"
        );
    }
}
