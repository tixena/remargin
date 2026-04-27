//! Cross-mode realm hazard: caller's CWD-resolved mode dominates over the
//! doc's realm mode. A caller standing in an open-mode dir who batch-writes
//! into a strict-mode realm produces unsigned comments inside that realm,
//! which subsequently fail `remargin verify` from inside the realm.
//!
//! Reproduces a real scenario from manual testing: an agent's CWD was
//! outside the realm under test (different `~/.remargin.yaml` declared
//! `mode: open`), the doc lived in a strict-mode realm, and `remargin batch`
//! silently wrote 23 unsigned comments. The realm's `verify` invariant
//! broke until `remargin sign --all-mine` was run.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::{Path, PathBuf};

    use assert_cmd::Command;
    use remargin_core::parser::parse as parse_doc;
    use serde_json::json;
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

    /// Build the cross-mode layout:
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
    ///     └── doc.md                    (empty doc, alice will batch into it)
    /// ```
    fn build_cross_mode_layout() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();

        // Shared registry at tempdir root so both realms see the same active set.
        let registry = format!(
            "participants:\n  alice:\n    type: human\n    status: active\n    pubkeys:\n      - {TEST_PUBLIC_KEY}\n"
        );
        fs::write(tmp.path().join(".remargin-registry.yaml"), &registry).unwrap();

        let outer = tmp.path().join("outer");
        fs::create_dir_all(&outer).unwrap();
        fs::write(outer.join("alice_key"), TEST_PRIVATE_KEY).unwrap();
        // Outer caller is in open mode. Identity is irrelevant here because
        // the test passes --identity/--type/--key explicitly — this yaml is
        // what `resolve_mode` will walk up to.
        fs::write(
            outer.join(".remargin.yaml"),
            "identity: alice\ntype: human\nmode: open\nkey: ./alice_key\n",
        )
        .unwrap();

        let notes = tmp.path().join("notes");
        fs::create_dir_all(&notes).unwrap();
        let notes_key = notes.join("alice_key");
        fs::write(&notes_key, TEST_PRIVATE_KEY).unwrap();
        // Notes realm is strict. This is the realm's own declared mode —
        // the doc inside this directory expects all comments to be signed.
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

    /// Failing test (rem-?): caller in open mode batches into a strict-mode
    /// realm. Either the batch must escalate to strict (sign every new
    /// comment with the resolvable key) OR refuse outright. Writing
    /// unsigned comments into a strict-mode realm's doc is wrong because
    /// every subsequent `remargin verify` from inside that realm will fail.
    #[test]
    fn batch_into_strict_realm_from_open_caller_does_not_leave_unsigned_comments() {
        let (_tmp, outer, doc, key) = build_cross_mode_layout();
        let doc_str = String::from(doc.to_string_lossy());
        let key_str = String::from(key.to_string_lossy());

        let ops = json!([
            { "op": "comment", "content": "first via cross-mode batch" },
            { "op": "comment", "content": "second via cross-mode batch" }
        ])
        .to_string();

        // Caller is in `outer` (open mode). They explicitly declare an
        // identity that is registered in the shared registry, and pass a
        // resolvable key path. The doc target lives in a strict-mode realm.
        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(&outer)
            .args([
                "batch",
                &doc_str,
                "--ops",
                &ops,
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
            // Acceptable fix path #1: batch escalated to strict mode and
            // signed every new comment. Assert signatures are present.
            for cm in doc_parsed.comments() {
                assert!(
                    cm.signature.is_some(),
                    "BUG: batch into a strict-mode realm wrote an unsigned \
                     comment (id={id}). The doc's realm declares mode: \
                     strict, so every comment must be signed. \
                     Caller-mode = open, realm-mode = strict.\n\
                     stderr={stderr}\nstdout={stdout}\ndoc=\n{after}",
                    id = cm.id,
                );
            }
        } else {
            // Acceptable fix path #2: batch refused with an error that
            // names the cross-mode hazard. The doc must remain untouched.
            assert!(
                doc_parsed.comments().is_empty(),
                "BUG: batch refused but already wrote partial state. \
                 stderr={stderr}\ndoc=\n{after}"
            );
            assert!(
                stderr.contains("strict") || stderr.contains("realm") || stderr.contains("mode"),
                "batch refusal must explain the cross-mode hazard. \
                 stderr was:\n{stderr}"
            );
        }
    }
}
