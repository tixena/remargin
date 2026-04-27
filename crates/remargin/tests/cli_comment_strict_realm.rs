//! Cross-mode realm hazard for the singular `remargin comment` path.
//!
//! Companion to `cli_batch_strict_realm.rs` (rem-90tr). Same scenario,
//! same invariant: a caller standing in an open-mode dir who writes a
//! single comment into a strict-mode realm must not leave an unsigned
//! comment in that realm. Either the write escalates to strict (signs)
//! or it refuses with a cross-mode error.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};

    use assert_cmd::Command;
    use remargin_core::parser::parse as parse_doc;
    use tempfile::TempDir;

    const TEST_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";

    const TEST_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin";

    /// Build the cross-mode layout (mirrors `cli_batch_strict_realm`):
    ///
    /// ```text
    /// tmp/
    /// ├── .remargin-registry.yaml      (alice registered with TEST_PUBLIC_KEY)
    /// ├── outer/                        (caller's CWD — open mode)
    /// │   ├── .remargin.yaml            (mode: open)
    /// │   └── alice_key                 (TEST_PRIVATE_KEY)
    /// └── notes/                        (doc lives here — strict mode)
    ///     ├── .remargin.yaml            (mode: strict)
    ///     ├── alice_key                 (TEST_PRIVATE_KEY)
    ///     └── doc.md                    (empty doc, alice will comment into it)
    /// ```
    fn build_cross_mode_layout() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();

        let registry = format!(
            "participants:\n  alice:\n    type: human\n    status: active\n    pubkeys:\n      - {TEST_PUBLIC_KEY}\n"
        );
        fs::write(tmp.path().join(".remargin-registry.yaml"), &registry).unwrap();

        let outer = tmp.path().join("outer");
        fs::create_dir_all(&outer).unwrap();
        fs::write(outer.join("alice_key"), TEST_PRIVATE_KEY).unwrap();
        fs::write(
            outer.join(".remargin.yaml"),
            "identity: alice\ntype: human\nmode: open\nkey: ./alice_key\n",
        )
        .unwrap();

        let notes = tmp.path().join("notes");
        fs::create_dir_all(&notes).unwrap();
        let notes_key = notes.join("alice_key");
        fs::write(&notes_key, TEST_PRIVATE_KEY).unwrap();
        fs::write(
            notes.join(".remargin.yaml"),
            "identity: alice\ntype: human\nmode: strict\nkey: ./alice_key\n",
        )
        .unwrap();

        let doc = notes.join("doc.md");
        fs::write(
            &doc,
            "---\ntitle: Strict realm doc\n---\n\n# Strict realm doc\n\nBody.\n",
        )
        .unwrap();

        (tmp, outer, doc, notes_key)
    }

    fn body(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// Caller in open mode runs `remargin comment` against a doc in a
    /// strict-mode realm. The realm's verify invariant requires every
    /// comment to be signed, so the singular comment path must either
    /// escalate (sign) or refuse — never write an unsigned comment.
    #[test]
    fn comment_into_strict_realm_from_open_caller_does_not_leave_unsigned_comment() {
        let (_tmp, outer, doc, key) = build_cross_mode_layout();
        let doc_str = String::from(doc.to_string_lossy());
        let key_str = String::from(key.to_string_lossy());

        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(&outer)
            .args([
                "comment",
                &doc_str,
                "single via cross-mode comment",
                "--identity",
                "alice",
                "--type",
                "human",
                "--key",
                &key_str,
            ])
            .output()
            .unwrap();

        let stderr = str::from_utf8(&out.stderr).unwrap_or_default();
        let stdout = str::from_utf8(&out.stdout).unwrap_or_default();

        let after = body(&doc);
        let doc_parsed = parse_doc(&after).unwrap();

        if out.status.success() {
            // Acceptable fix path #1: write escalated to strict mode and
            // signed the new comment.
            for cm in doc_parsed.comments() {
                assert!(
                    cm.signature.is_some(),
                    "BUG: comment into a strict-mode realm wrote an \
                     unsigned comment (id={id}). The doc's realm \
                     declares mode: strict, so every comment must be \
                     signed. Caller-mode = open, realm-mode = strict.\n\
                     stderr={stderr}\nstdout={stdout}\ndoc=\n{after}",
                    id = cm.id,
                );
            }
        } else {
            // Acceptable fix path #2: comment refused with an error
            // that names the cross-mode hazard. The doc must remain
            // untouched.
            assert!(
                doc_parsed.comments().is_empty(),
                "BUG: comment refused but already wrote partial state. \
                 stderr={stderr}\ndoc=\n{after}"
            );
            assert!(
                stderr.contains("strict") || stderr.contains("realm") || stderr.contains("mode"),
                "comment refusal must explain the cross-mode hazard. \
                 stderr was:\n{stderr}"
            );
        }
    }
}
