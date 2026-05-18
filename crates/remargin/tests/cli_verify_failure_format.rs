//! Subset-gate refusal rendering at the CLI surface. Under the subset
//! gate, mutating ops that don't introduce new anomalies (e.g. `ack`
//! against a file with a pre-existing bad checksum) must succeed.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;

    use assert_cmd::Command;
    use tempfile::TempDir;

    /// Open-mode workspace whose single comment has a stale checksum.
    /// Under the subset gate, ops that don't introduce new anomalies
    /// (like `ack`) must pass — the pre-existing bad checksum is in P,
    /// so it's also in Q, so Q ⊆ P.
    fn build_bad_checksum_workspace() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(".remargin.yaml"),
            "identity: alice\ntype: human\nmode: open\n",
        )
        .unwrap();
        let doc = "\
---
title: Doc
---

# Doc

Body.

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:0000000000000000000000000000000000000000000000000000000000000000
---
hello
```
";
        fs::write(tmp.path().join("doc.md"), doc).unwrap();
        tmp
    }

    #[test]
    fn ack_against_bad_checksum_succeeds_under_subset_gate() {
        // ack on a file with a pre-existing bad checksum must succeed —
        // the anomaly is in P, and ack doesn't add to Q.
        let tmp = build_bad_checksum_workspace();
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["ack", "--file", "doc.md", "abc"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "ack should succeed when no new anomaly is introduced; stderr: {}",
            str::from_utf8(&out.stderr).unwrap()
        );
    }
}
