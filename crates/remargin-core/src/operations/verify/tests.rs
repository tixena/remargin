//! Tests for `verify_document` and `commit_with_verify`.
//!
//! The severity matrix is exercised as one test per (status × mode) cell
//! per rem-ef1's acceptance criteria. `RowStatus` / `SignatureStatus`
//! rendering is also exercised.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::Path;

use chrono::DateTime;
use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::registry::Registry;
use crate::config::{Mode, ResolvedConfig};
use crate::crypto;
use crate::operations::batch::{BatchCommentOp, batch_comment};
use crate::operations::verify::{RowStatus, SignatureStatus, commit_with_verify, verify_document};
use crate::operations::{
    CreateCommentParams, ack_comments, create_comment, delete_comments, edit_comment,
};
use crate::parser::{self, AuthorType, Comment, ParsedDocument, Segment};
use crate::writer::InsertPosition;

const SIMPLE_DOC: &str = "\
---
title: Test
---

# Hello

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

fn make_comment(id: &str, author: &str, content: &str) -> Comment {
    let checksum = crypto::compute_checksum(content, &[]);
    Comment {
        ack: Vec::new(),
        attachments: Vec::new(),
        author: String::from(author),
        author_type: AuthorType::Human,
        checksum,
        content: String::from(content),
        id: String::from(id),
        line: 0,
        reactions: BTreeMap::new(),
        remargin_kind: Vec::new(),
        reply_to: None,
        signature: None,
        thread: None,
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339("2026-04-06T14:32:00-04:00").unwrap(),
    }
}

fn doc_with(comments: Vec<Comment>) -> ParsedDocument {
    let mut doc = ParsedDocument {
        segments: Vec::new(),
    };
    doc.segments.push(Segment::Body(String::new()));
    for cm in comments {
        doc.segments.push(Segment::Comment(Box::new(cm)));
    }
    doc
}

fn make_config(mode: Mode, registry: Option<Registry>) -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("alice")),
        ignore: Vec::new(),
        key_path: None,
        mode,
        registry,
        source_path: None,
        unrestricted: false,
    }
}

fn registry_with(yaml: &str) -> Registry {
    serde_yaml::from_str(yaml).unwrap()
}

/// Registry where `alice` is active with a made-up pubkey and `bob` is
/// active with no pubkeys (so any signature from bob cannot match).
fn alice_active_registry() -> Registry {
    registry_with(
        "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAtestalicekey
  bob:
    type: human
    status: active
    pubkeys: []
",
    )
}

// ---------- status.as_str rendering ----------

#[test]
fn signature_status_as_str_matches_cli_vocabulary() {
    assert_eq!(SignatureStatus::Missing.as_str(), "missing");
    assert_eq!(SignatureStatus::Invalid.as_str(), "invalid");
    assert_eq!(SignatureStatus::Valid.as_str(), "valid");
    assert_eq!(SignatureStatus::UnknownAuthor.as_str(), "unknown_author");
}

// ---------- severity: Open mode ----------
// Every status but Invalid (and bad checksum) is neutral in Open.

#[test]
fn open_mode_missing_is_neutral() {
    let doc = doc_with(vec![make_comment("a", "alice", "hello")]);
    let cfg = make_config(Mode::Open, None);
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results.len(), 1);
    assert_eq!(rep.results[0].signature, SignatureStatus::Missing);
    assert!(rep.ok, "Open + missing should be neutral, report ok");
}

#[test]
fn open_mode_unknown_author_is_neutral() {
    // Registry present, author not in registry => UnknownAuthor, but Open
    // mode tolerates unknown authors.
    let doc = doc_with(vec![make_comment("a", "charlie", "hello")]);
    let cfg = make_config(Mode::Open, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::UnknownAuthor);
    assert!(rep.ok, "Open + unknown_author should be neutral");
}

#[test]
fn open_mode_invalid_is_bad() {
    // Alice is registered with a real-shaped key; an `ed25519:` payload
    // that cannot be decoded as a valid sshsig resolves to Invalid.
    let mut cm = make_comment("a", "alice", "hello");
    cm.signature = Some(String::from("ed25519:garbage-not-a-valid-signature"));
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Open, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::Invalid);
    assert!(!rep.ok, "Invalid is always bad, even in Open");
}

#[test]
fn open_mode_bad_checksum_is_bad() {
    let mut cm = make_comment("a", "alice", "hello");
    cm.checksum = String::from("sha256:deadbeef");
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Open, None);
    let rep = verify_document(&doc, &cfg);
    assert!(!rep.results[0].checksum_ok);
    assert!(!rep.ok, "Bad checksum is always bad, even in Open");
}

// ---------- severity: Registered mode ----------

#[test]
fn registered_mode_missing_is_neutral() {
    let doc = doc_with(vec![make_comment("a", "alice", "hello")]);
    let cfg = make_config(Mode::Registered, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::Missing);
    assert!(rep.ok, "Registered + missing is neutral");
}

#[test]
fn registered_mode_unknown_author_is_bad() {
    let doc = doc_with(vec![make_comment("a", "charlie", "hello")]);
    let cfg = make_config(Mode::Registered, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::UnknownAuthor);
    assert!(!rep.ok, "Registered + unknown_author is bad");
}

#[test]
fn registered_mode_invalid_is_bad() {
    let mut cm = make_comment("a", "alice", "hello");
    cm.signature = Some(String::from("ed25519:garbage"));
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Registered, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::Invalid);
    assert!(!rep.ok);
}

// ---------- severity: Strict mode ----------

#[test]
fn strict_mode_missing_for_registered_active_is_bad() {
    let doc = doc_with(vec![make_comment("a", "alice", "hello")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::Missing);
    assert!(!rep.ok, "Strict + missing for registered active is bad");
}

#[test]
fn strict_mode_missing_for_unknown_author_is_bad_via_unknown_author() {
    // Author isn't in registry at all → resolves to UnknownAuthor (always
    // bad in Strict), not Missing. The bad-ness comes from UnknownAuthor.
    let doc = doc_with(vec![make_comment("a", "charlie", "hello")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::UnknownAuthor);
    assert!(!rep.ok);
}

#[test]
fn strict_mode_unknown_author_is_bad() {
    let doc = doc_with(vec![make_comment("a", "charlie", "hello")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::UnknownAuthor);
    assert!(!rep.ok);
}

#[test]
fn strict_mode_invalid_is_bad() {
    let mut cm = make_comment("a", "alice", "hello");
    cm.signature = Some(String::from("ed25519:garbage"));
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results[0].signature, SignatureStatus::Invalid);
    assert!(!rep.ok);
}

// ---------- aggregation ----------

#[test]
fn empty_document_is_ok_in_every_mode() {
    let doc = doc_with(vec![]);
    for mode in [Mode::Open, Mode::Registered, Mode::Strict] {
        let cfg = make_config(mode.clone(), Some(alice_active_registry()));
        let rep = verify_document(&doc, &cfg);
        assert!(rep.results.is_empty());
        assert!(rep.ok, "empty doc must pass in {mode:?}");
    }
}

#[test]
fn one_bad_row_marks_whole_report_bad() {
    let mut bad = make_comment("a", "alice", "hello");
    bad.checksum = String::from("sha256:deadbeef");
    let good = make_comment("b", "alice", "hello");
    let doc = doc_with(vec![bad, good]);
    let cfg = make_config(Mode::Open, None);
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results.len(), 2);
    assert!(!rep.results[0].checksum_ok);
    assert!(rep.results[1].checksum_ok);
    assert!(!rep.ok, "one bad row poisons the aggregate");
}

// ---------- commit_with_verify gate ----------

#[test]
fn commit_with_verify_invokes_writer_when_ok() {
    let doc = doc_with(vec![make_comment("a", "alice", "hello")]);
    let cfg = make_config(Mode::Open, None);

    let mut called = false;
    let result = commit_with_verify(&doc, &cfg, |_| {
        called = true;
        Ok(())
    });

    result.unwrap();
    assert!(called, "writer must be called when report is ok");
}

#[test]
fn commit_with_verify_blocks_writer_on_bad_checksum() {
    let mut bad = make_comment("a", "alice", "hello");
    bad.checksum = String::from("sha256:deadbeef");
    let doc = doc_with(vec![bad]);
    let cfg = make_config(Mode::Open, None);

    let mut called = false;
    let result = commit_with_verify(&doc, &cfg, |_| {
        called = true;
        Ok(())
    });

    assert!(result.is_err());
    assert!(!called, "writer must not run when verify fails");
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("verify failed"),
        "diagnostic must name verify, got: {msg}",
    );
}

#[test]
fn commit_with_verify_blocks_in_registered_for_unknown_author() {
    let doc = doc_with(vec![make_comment("a", "charlie", "hello")]);
    let cfg = make_config(Mode::Registered, Some(alice_active_registry()));

    let mut called = false;
    let result = commit_with_verify(&doc, &cfg, |_| {
        called = true;
        Ok(())
    });

    assert!(result.is_err());
    assert!(!called);
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("registered"),
        "diagnostic should name mode, got: {msg}"
    );
    assert!(
        msg.contains("unknown_author"),
        "diagnostic should list the status, got: {msg}"
    );
}

// ---------- row rendering sanity ----------

#[test]
fn row_status_struct_preserves_comment_id() {
    let doc = doc_with(vec![make_comment("myid123", "alice", "hello")]);
    let cfg = make_config(Mode::Open, None);
    let rep = verify_document(&doc, &cfg);
    let row: &RowStatus = &rep.results[0];
    assert_eq!(row.id, "myid123");
}

// ---------- reality check: parse + verify round-trip ----------

#[test]
fn parse_then_verify_plain_open_mode() {
    let doc = parser::parse(SIMPLE_DOC).unwrap();
    let cfg = make_config(Mode::Open, None);
    let rep = verify_document(&doc, &cfg);
    assert_eq!(rep.results.len(), 1);
    assert!(
        rep.results[0].checksum_ok,
        "checksum in SIMPLE_DOC must round-trip"
    );
    assert_eq!(rep.results[0].signature, SignatureStatus::Missing);
    assert!(rep.ok);
}

// ---------- op-level gate: file stays byte-identical when gate trips ----------
//
// The spec: a failing mutation must leave the on-disk file byte-identical
// to before the call. Each of these tests mutates a file under a config
// where the gate will trip and asserts the file contents are unchanged.

/// A minimal valid document with a real-checksum comment authored by
/// `alice` (who is present in `alice_active_registry`). Used to assert
/// that fresh ops work against this baseline document.
fn alice_doc_content() -> String {
    // sha256 of "alice's note" (exact content below).
    let content = "alice's note";
    let cksum = crypto::compute_checksum(content, &[]);
    format!(
        "\
---
title: Test
---

# Hello

```remargin
---
id: alc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: {cksum}
---
{content}
```
",
    )
}

fn registered_cfg_with_alice() -> ResolvedConfig {
    let mut cfg = make_config(Mode::Registered, Some(alice_active_registry()));
    cfg.identity = Some(String::from("alice"));
    cfg
}

fn open_cfg_as(author: &str) -> ResolvedConfig {
    let mut cfg = make_config(Mode::Open, None);
    cfg.identity = Some(String::from(author));
    cfg
}

/// Helper: put the document on a mock filesystem.
fn mock_with_doc(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/d/a.md"), content.as_bytes())
        .unwrap()
}

#[test]
fn comment_op_open_mode_unknown_author_succeeds_and_writes() {
    // Open mode + fresh unregistered identity should be accepted by the
    // gate (unknown_author is neutral in Open).
    let system = mock_with_doc(&alice_doc_content());
    let cfg = open_cfg_as("charlie");

    let pos = InsertPosition::Append;
    let new_id = create_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "reply",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(content.contains(&new_id), "new id must be present on disk");
    assert!(
        content.contains("charlie"),
        "new comment's author must be written"
    );
}

#[test]
fn comment_op_registered_mode_unregistered_author_file_byte_identical() {
    // Post-xc8x the primary gate is at resolve time (see
    // `config::tests::resolve_bails_when_revoked_identity_in_strict_mode`
    // for the resolver-level test). This test covers the belt-and-braces
    // case: if a caller somehow hands `create_comment` a hand-built
    // config whose identity is not in the registry, the post-write
    // verify gate still catches the bad artifact and the file stays
    // byte-identical.
    let before = alice_doc_content();
    let system = mock_with_doc(&before);
    let mut bad_cfg = registered_cfg_with_alice();
    // Force-swap identity to a non-registered author.
    bad_cfg.identity = Some(String::from("charlie"));

    let pos = InsertPosition::Append;
    let result = create_comment(
        &system,
        Path::new("/d/a.md"),
        &bad_cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "sneaky",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    );

    result.unwrap_err();
    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(
        after, before,
        "file must be byte-identical after blocked op"
    );
}

#[test]
fn ack_op_open_mode_identity_not_in_registry_succeeds() {
    // `ack_comments` with an unregistered identity in Open mode: the gate
    // tolerates unknown_author rows, and the ack write lands.
    let system = mock_with_doc(&alice_doc_content());
    let cfg = open_cfg_as("charlie");

    ack_comments(&system, Path::new("/d/a.md"), &cfg, &["alc"], false).unwrap();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        after.contains("charlie@"),
        "open-mode ack from unregistered author must land: {after}"
    );
}

#[test]
fn ack_op_gate_blocks_when_existing_comment_has_bad_checksum() {
    // A document that already has a comment with a bad checksum is an
    // integrity incident. Any subsequent mutation must be blocked and
    // leave the file byte-identical (the gate catches it on the way out).
    //
    // This is the "bad checksum on disk" regression guard from rem-ef1's
    // acceptance list.
    let corrupted = "\
---
title: Test
---

# Hello

```remargin
---
id: alc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:deadbeef
---
alice's note
```
";
    let system = mock_with_doc(corrupted);
    let cfg = open_cfg_as("alice");

    let result = ack_comments(&system, Path::new("/d/a.md"), &cfg, &["alc"], false);
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("verify failed"),
        "error should mention verify: {msg}"
    );
    assert!(
        msg.contains("checksum=FAIL"),
        "error should call out the bad row: {msg}"
    );

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(
        after, corrupted,
        "file must be byte-identical after gate trip"
    );
}

// ---------- rem-dyz: strict mode fails fast at creation time ----------
//
// Creation-time fail-fast is paired with the post-write verify gate. The
// verify gate catches unsigned artifacts on the NEXT mutation (too late,
// because nine orphans have already been written). These tests exercise
// the pre-write fail-fast path: strict + registered active + no key →
// the op bails before touching disk.

/// Strict-mode config with `alice` registered active, no key path set.
/// The identity field is blank by default; each test sets it to the
/// relevant author.
fn strict_cfg_with_alice_no_key() -> ResolvedConfig {
    let mut cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    cfg.identity = Some(String::from("alice"));
    cfg.key_path = None;
    cfg
}

#[test]
fn create_comment_strict_registered_active_no_key_file_byte_identical() {
    // The headline rem-dyz scenario: strict + registered active + no key
    // configured must never corrupt disk.
    //
    // Post-xc8x the primary gate is at resolve time (the paired test
    // `config::tests::resolve_bails_when_strict_identity_has_no_key`
    // asserts the resolver error surface). Here we exercise the
    // belt-and-braces path: a hand-built invalid config reaches
    // `create_comment`, the post-write verify gate catches the unsigned
    // artifact, and the file stays byte-identical.
    let before = alice_doc_content();
    let system = mock_with_doc(&before);
    let cfg = strict_cfg_with_alice_no_key();

    let pos = InsertPosition::Append;
    let result = create_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "unsigned attempt",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    );

    result.unwrap_err();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(
        after, before,
        "file must be byte-identical when the verify gate trips",
    );
    assert!(
        !after.contains("unsigned attempt"),
        "rejected content must never reach disk"
    );
}

#[test]
fn create_comment_strict_unregistered_author_file_byte_identical() {
    // Strict + unregistered author via a hand-built config (bypassing
    // the resolver). The resolver-level rejection is the primary gate
    // (see `config::tests::resolve_bails_when_revoked_identity_in_strict_mode`);
    // this test confirms that even if an invalid config reaches the op,
    // the verify gate still refuses to write a bad artifact.
    let before = alice_doc_content();
    let system = mock_with_doc(&before);
    let mut cfg = strict_cfg_with_alice_no_key();
    cfg.identity = Some(String::from("charlie"));

    let pos = InsertPosition::Append;
    let result = create_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "uninvited",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    );

    result.unwrap_err();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(after, before, "file must be byte-identical");
}

#[test]
fn create_comment_open_mode_no_key_still_writes_unsigned() {
    // Open mode is the explicit non-strict regression guard. No key
    // configured, registered or not — the op must land, unsigned.
    let system = mock_with_doc(&alice_doc_content());
    let cfg = open_cfg_as("alice");

    let pos = InsertPosition::Append;
    let new_id = create_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: false,
            content: "open mode reply",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let content = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(content.contains(&new_id));
    assert!(
        content.contains("open mode reply"),
        "open-mode unsigned write must land"
    );
}

#[test]
fn edit_comment_strict_registered_active_no_key_fails_fast() {
    // edit_comment is a signed-artifact-producing op: it re-signs on edit
    // when the identity requires signing. Same fail-fast rule applies.
    let before = alice_doc_content();
    let system = mock_with_doc(&before);
    let cfg = strict_cfg_with_alice_no_key();

    let result = edit_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        "alc",
        "new content that must not land",
        None,
    );

    result.unwrap_err();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(
        after, before,
        "file must be byte-identical when the verify gate trips on an edit",
    );
    assert!(
        !after.contains("new content that must not land"),
        "rejected content must never reach disk"
    );
}

#[test]
fn batch_comment_strict_registered_active_no_key_file_byte_identical() {
    // Pre-xc8x the op had its own fail-fast. After xc8x the resolver
    // rejects this combination at construction; if a hand-built config
    // sneaks past, the post-write verify gate catches the unsigned
    // batch before any byte reaches disk.
    let before = alice_doc_content();
    let system = mock_with_doc(&before);
    let cfg = strict_cfg_with_alice_no_key();

    let ops = vec![
        BatchCommentOp::new(String::from("batch op 1")),
        BatchCommentOp::new(String::from("batch op 2")),
    ];

    let result = batch_comment(&system, Path::new("/d/a.md"), &cfg, &ops);
    result.unwrap_err();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(
        after, before,
        "file must be byte-identical when the verify gate trips on a batch",
    );
}

#[test]
fn delete_op_gate_blocks_over_corrupted_doc() {
    // Even an op whose purpose is to remove a comment cannot land if the
    // resulting in-memory doc still has a bad checksum on a surviving
    // comment.
    let corrupted = "\
---
title: Test
---

# Hello

```remargin
---
id: alc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb
---
alice's note
```

```remargin
---
id: bad
author: alice
type: human
ts: 2026-04-06T13:00:00-04:00
checksum: sha256:deadbeef
---
surviving corrupt row
```
";
    let system = mock_with_doc(corrupted);
    let cfg = open_cfg_as("alice");

    // Try to delete `alc` (the good one). The surviving `bad` row still
    // fails verify, so the gate blocks.
    let result = delete_comments(&system, Path::new("/d/a.md"), &cfg, &["alc"]);
    let _err: anyhow::Error = result.unwrap_err();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(
        after, corrupted,
        "file must be byte-identical after blocked delete"
    );
}
