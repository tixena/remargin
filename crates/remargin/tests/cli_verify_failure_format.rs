//! When the post-write verify gate trips, the CLI must surface a
//! readable headline + per-failure summary + actionable hint, instead
//! of the raw `verify failed (mode: …)` blob it shipped before. The
//! `--json` surface mirrors the same data as a structured object
//! (`error_kind`, `failures`, `headline`, `hint`, `mode`, `path`) so
//! callers can branch on the `error_kind` without regex-matching a
//! free-form string.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;

    use assert_cmd::Command;
    use serde_json::Value;
    use tempfile::TempDir;

    /// Shape an open-mode workspace with a doc whose comment carries a
    /// stale checksum. Open mode tolerates `Missing` / `UnknownAuthor`,
    /// but a bad checksum is always bad — the verify gate trips on the
    /// next mutating op (here: `ack`) regardless of mode.
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
    fn ack_against_bad_checksum_renders_human_headline_on_stderr() {
        let tmp = build_bad_checksum_workspace();
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["ack", "--file", "doc.md", "abc"])
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "ack should fail when checksum is bad"
        );
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.starts_with("error: verify failed:"),
            "stderr should lead with a plain-English headline, got: {stderr:?}"
        );
        assert!(
            stderr.contains("doc.md"),
            "stderr should name the document, got: {stderr:?}"
        );
        assert!(
            stderr.contains("Try `remargin verify"),
            "stderr should carry the actionable hint, got: {stderr:?}"
        );
        assert!(
            !stderr.contains("\\n"),
            "stderr must render real newlines, not escaped \\n: {stderr:?}"
        );
    }

    #[test]
    fn ack_against_bad_checksum_emits_structured_json_error() {
        let tmp = build_bad_checksum_workspace();
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["ack", "--file", "doc.md", "abc", "--json"])
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "ack should fail when checksum is bad"
        );
        let stderr = str::from_utf8(&out.stderr).unwrap();
        let parsed: Result<Value, _> = serde_json::from_str(stderr.trim());
        assert!(
            parsed.is_ok(),
            "stderr should be JSON; err={:?}; stderr={stderr:?}",
            parsed.as_ref().err()
        );
        let value = parsed.unwrap();
        assert_eq!(value["error_kind"], "verify_failed");
        assert_eq!(value["mode"], "open");
        assert!(
            value["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("doc.md")),
            "path should name the doc, got: {}",
            value["path"]
        );
        let failures = value["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["id"], "abc");
        assert_eq!(failures[0]["checksum_ok"], false);
        assert!(
            value["headline"]
                .as_str()
                .unwrap()
                .starts_with("verify failed:"),
            "headline lead-in is stable, got: {}",
            value["headline"]
        );
        assert!(
            value.get("elapsed_ms").is_some(),
            "envelope keeps elapsed_ms"
        );
    }
}
