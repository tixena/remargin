//! Tests for `verify_document` and `commit_with_verify`.
//!
//! The severity matrix is exercised as one test per (status × mode) cell
//! 's acceptance criteria. `RowStatus` / `SignatureStatus`
//! rendering is also exercised.

extern crate alloc;

use std::path::{Path, PathBuf};

use chrono::DateTime;
use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::registry::Registry;
use crate::config::{Mode, ResolvedConfig};
use crate::crypto;
use crate::operations::batch::{BatchCommentOp, batch_comment};
use crate::operations::verify::{
    Anomaly, AnomalyKind, RecipientStatus, RowStatus, SignatureStatus, SubsetGateFailure,
    VerifyFailure, anomalies_for_doc, commit_with_verify, verify_and_refresh, verify_document,
};
use crate::operations::{
    CreateCommentParams, ack_comments, create_comment, delete_comments, edit_comment, react,
};
use crate::parser::{self, AuthorType, Comment, ParsedDocument, Segment};
use crate::reactions::Reactions;
use crate::writer::InsertPosition;

/// Document carrying a directed comment with a partial ack and
/// frontmatter that would be wrong under the (correct, post-fix)
/// `is_pending` rule. Models a doc written by a buggy older version of
/// remargin that thought any ack closed a directed comment.
const STALE_FRONTMATTER_DOC: &str = "\
---
title: Test
remargin_pending: 0
remargin_pending_for: []
---

# Hello

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
to: [eduardo]
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
ack:
  - agent@2026-04-06T13:00:00-04:00
---
hello
```
";

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

const ALICE_ACTIVE_REGISTRY_YAML: &str = "\
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
";

const RECIPIENT_VERIFY_REGISTRY: &str = "\
participants:
  alice:
    type: human
    status: active
    pubkeys: []
  bob:
    type: human
    status: revoked
    pubkeys: []
  eduardo-burgos:
    type: human
    status: active
    pubkeys: []
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
        edited_at: None,
        id: String::from(id),
        line: 0,
        reactions: Reactions::new(),
        remargin_kind: None,
        reply_to: None,
        signature: None,
        thread: None,
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339("2026-04-06T14:32:00-04:00").unwrap(),
    }
}

fn doc_with(comments: Vec<Comment>) -> ParsedDocument {
    let mut doc = ParsedDocument::from_segments(Vec::new());
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
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn registry_with(yaml: &str) -> Registry {
    serde_yaml::from_str(yaml).unwrap()
}

/// Registry where `alice` is active with a made-up pubkey and `bob` is
/// active with no pubkeys (so any signature from bob cannot match).
fn alice_active_registry() -> Registry {
    registry_with(ALICE_ACTIVE_REGISTRY_YAML)
}

// WHY: commit_with_verify now derives mode and registry from the doc's
// realm. Tests that hand it a (mode, registry) pair must also stage a
// matching realm at /d/ so the realm walk doesn't replace either.
fn realm_at_d(mode: &Mode, registry_yaml: Option<&str>) -> MockSystem {
    let yaml = format!("mode: {}\n", mode.as_str());
    let mut sys = MockSystem::new()
        .with_file(Path::new("/d/.remargin.yaml"), yaml.as_bytes())
        .unwrap();
    if let Some(reg) = registry_yaml {
        sys = sys
            .with_file(Path::new("/d/.remargin-registry.yaml"), reg.as_bytes())
            .unwrap();
    }
    sys
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
    let system = MockSystem::new();

    let mut called = false;
    let result = commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_| {
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
    let system = MockSystem::new();

    let mut called = false;
    let result = commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_| {
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
    let system = realm_at_d(&Mode::Registered, Some(ALICE_ACTIVE_REGISTRY_YAML));

    let mut called = false;
    let result = commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_| {
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

// ---------- VerifyFailure: typed-error rendering ----------

#[test]
fn verify_failure_headline_singular() {
    let doc = doc_with(vec![make_comment("abc", "charlie", "hi")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let vf = VerifyFailure::from_document(&doc, &cfg, Path::new("/d/a.md"));
    assert_eq!(
        vf.headline(),
        "verify failed: 1 unsigned or invalid comment in /d/a.md"
    );
}

#[test]
fn verify_failure_headline_plural() {
    let doc = doc_with(vec![
        make_comment("a1", "charlie", "x"),
        make_comment("a2", "dave", "y"),
        make_comment("a3", "eve", "z"),
    ]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let vf = VerifyFailure::from_document(&doc, &cfg, Path::new("/d/multi.md"));
    assert_eq!(
        vf.headline(),
        "verify failed: 3 unsigned or invalid comments in /d/multi.md"
    );
}

#[test]
fn verify_failure_summary_groups_by_status() {
    let doc = doc_with(vec![
        make_comment("a1", "charlie", "x"),
        make_comment("a2", "dave", "y"),
    ]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let vf = VerifyFailure::from_document(&doc, &cfg, Path::new("/d/a.md"));
    let lines = vf.summary_lines();
    assert_eq!(lines.len(), 1, "single status group, got: {lines:?}");
    assert!(
        lines[0].contains("a1, a2") && lines[0].contains("unknown_author"),
        "summary should list ids and status, got: {}",
        lines[0]
    );
}

#[test]
fn verify_failure_summary_truncates_after_five_ids() {
    // Strict + alice-registered-no-key path: alice with Missing
    // signature is bad in Strict.
    let doc = doc_with(vec![
        make_comment("id01", "alice", "a"),
        make_comment("id02", "alice", "b"),
        make_comment("id03", "alice", "c"),
        make_comment("id04", "alice", "d"),
        make_comment("id05", "alice", "e"),
        make_comment("id06", "alice", "f"),
        make_comment("id07", "alice", "g"),
    ]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let vf = VerifyFailure::from_document(&doc, &cfg, Path::new("/d/a.md"));
    let lines = vf.summary_lines();
    assert!(
        lines[0].contains("(and 2 more)"),
        "summary first line should call out the truncated tail, got: {}",
        lines[0]
    );
}

#[test]
fn verify_failure_to_json_shape_is_stable() {
    let doc = doc_with(vec![make_comment("abc", "charlie", "hi")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let vf = VerifyFailure::from_document(&doc, &cfg, Path::new("/d/a.md"));
    let value = vf.to_json();
    assert_eq!(value["error_kind"], "verify_failed");
    assert_eq!(value["mode"], "strict");
    assert_eq!(value["path"], "/d/a.md");
    let failures = value["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["id"], "abc");
    assert_eq!(failures[0]["signature"], "unknown_author");
    assert_eq!(failures[0]["checksum_ok"], true);
    assert!(
        value["headline"]
            .as_str()
            .unwrap()
            .starts_with("verify failed:")
    );
    assert!(value["hint"].as_str().unwrap().contains("remargin verify"));
}

#[test]
fn verify_failure_human_text_has_three_blocks() {
    let doc = doc_with(vec![make_comment("abc", "charlie", "hi")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let vf = VerifyFailure::from_document(&doc, &cfg, Path::new("/d/a.md"));
    let text = vf.human_text();
    assert!(text.starts_with("verify failed:"), "headline first: {text}");
    assert!(text.contains("\n\n- "), "summary block follows: {text}");
    assert!(text.contains("Try `remargin verify"), "hint last: {text}");
}

#[test]
fn commit_with_verify_returns_typed_subset_gate_failure() {
    // Fresh file (no on-disk state → P = ∅). The in-memory doc has a
    // charlie comment, which under strict+alice-registry is
    // unknown_author. Q = {(abc, SignatureUnknownAuthor)}. Q ⊄ P →
    // SubsetGateFailure.
    let doc = doc_with(vec![make_comment("abc", "charlie", "hi")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let system = realm_at_d(&Mode::Strict, Some(ALICE_ACTIVE_REGISTRY_YAML));
    let result = commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_| Ok(()));
    let err = result.unwrap_err();
    let sg = err.downcast_ref::<SubsetGateFailure>().unwrap();
    assert_eq!(sg.path, Path::new("/d/a.md"));
    assert_eq!(sg.introduced.len(), 1);
    assert_eq!(sg.introduced[0].id, "abc");
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

// WHY: file's realm is the source of truth for mode AND registry. Tests
// that rely on strict-mode op gating need /d/ to declare strict
// explicitly AND carry the registry the realm's gate consults.
fn mock_with_doc_in_strict_realm(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/d/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/d/.remargin-registry.yaml"),
            ALICE_ACTIVE_REGISTRY_YAML.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/d/a.md"), content.as_bytes())
        .unwrap()
}

fn mock_with_doc_in_registered_realm(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/d/.remargin.yaml"), b"mode: registered\n")
        .unwrap()
        .with_file(
            Path::new("/d/.remargin-registry.yaml"),
            ALICE_ACTIVE_REGISTRY_YAML.as_bytes(),
        )
        .unwrap()
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
            auto_ack: Some(false),
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
    let system = mock_with_doc_in_registered_realm(&before);
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
            auto_ack: Some(false),
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
fn ack_op_passes_when_existing_comment_has_bad_checksum() {
    // Under the subset gate, a pre-existing bad checksum (in P) does
    // not block a mutating op that doesn't introduce new anomalies.
    // ack only adds to the comment's ack list — Q has the same
    // checksum_invalid anomaly P does, so Q ⊆ P. Allowed.
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

    ack_comments(&system, Path::new("/d/a.md"), &cfg, &["alc"], false).unwrap();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        after.contains("alice@"),
        "open-mode ack should land even with pre-existing bad checksum: {after}"
    );
}

// ----------: strict mode fails fast at creation time ----------
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
    // The headline scenario: strict + registered active + no key
    // configured must never corrupt disk.
    //
    // Post-xc8x the primary gate is at resolve time (the paired test
    // `config::tests::resolve_bails_when_strict_identity_has_no_key`
    // asserts the resolver error surface). Here we exercise the
    // belt-and-braces path: a hand-built invalid config reaches
    // `create_comment`, the post-write verify gate catches the unsigned
    // artifact, and the file stays byte-identical.
    let before = alice_doc_content();
    let system = mock_with_doc_in_strict_realm(&before);
    let cfg = strict_cfg_with_alice_no_key();

    let pos = InsertPosition::Append;
    let result = create_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: Some(false),
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
    let system = mock_with_doc_in_strict_realm(&before);
    let mut cfg = strict_cfg_with_alice_no_key();
    cfg.identity = Some(String::from("charlie"));

    let pos = InsertPosition::Append;
    let result = create_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: Some(false),
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
            auto_ack: Some(false),
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
fn edit_comment_strict_registered_active_no_key_passes_under_subset_gate() {
    // alice_doc_content has an unsigned alice comment. Under strict +
    // alice-registered-active, P = {(alc, signature_missing)}. The
    // edit recomputes the checksum to match the new content but
    // produces no new signature (no key). Q still = {(alc,
    // signature_missing)}. Q ⊆ P → allowed. The edit lands.
    let before = alice_doc_content();
    let system = mock_with_doc_in_strict_realm(&before);
    let cfg = strict_cfg_with_alice_no_key();

    edit_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        "alc",
        "new content that lands under subset gate",
        None,
    )
    .unwrap();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        after.contains("new content that lands under subset gate"),
        "edit must land under subset gate when no new anomalies are introduced"
    );
}

#[test]
fn batch_comment_strict_registered_active_no_key_file_byte_identical() {
    // Pre-xc8x the op had its own fail-fast. After xc8x the resolver
    // rejects this combination at construction; if a hand-built config
    // sneaks past, the post-write verify gate catches the unsigned
    // batch before any byte reaches disk.
    let before = alice_doc_content();
    let system = mock_with_doc_in_strict_realm(&before);
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
fn delete_op_passes_over_corrupted_doc_under_subset_gate() {
    // P contains (bad, checksum_invalid). Deleting `alc` leaves only
    // the still-corrupted `bad` row → Q = {(bad, checksum_invalid)}.
    // Q ⊆ P → allowed. Pre-existing anomalies are not magnified into
    // a block; the op proceeds.
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

    delete_comments(&system, Path::new("/d/a.md"), &cfg, &["alc"]).unwrap();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        !after.contains("alice's note"),
        "delete must remove alc; file: {after}"
    );
    assert!(
        after.contains("surviving corrupt row"),
        "delete must leave bad row in place; file: {after}"
    );
}

// ===========================================================================
// verify_and_refresh: self-healing frontmatter on a stale file, no-op on a
// fresh file.
// ===========================================================================

#[test]
fn verify_and_refresh_rewrites_stale_frontmatter() {
    let system = mock_with_doc(STALE_FRONTMATTER_DOC);
    let cfg = open_cfg_as("alice");

    let report = verify_and_refresh(&system, Path::new("/d/a.md"), &cfg).unwrap();
    assert!(report.ok, "open mode + good checksum must report ok");

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        after.contains("remargin_pending: 1"),
        "stale frontmatter must self-heal under verify; got:\n{after}"
    );
    assert!(
        after.contains("eduardo"),
        "remargin_pending_for must list the unacked addressee; got:\n{after}"
    );
}

#[test]
fn verify_and_refresh_is_a_no_op_when_frontmatter_is_already_current() {
    let system = mock_with_doc(STALE_FRONTMATTER_DOC);
    let cfg = open_cfg_as("alice");

    verify_and_refresh(&system, Path::new("/d/a.md"), &cfg).unwrap();
    let after_first = system.read_to_string(Path::new("/d/a.md")).unwrap();

    verify_and_refresh(&system, Path::new("/d/a.md"), &cfg).unwrap();
    let after_second = system.read_to_string(Path::new("/d/a.md")).unwrap();

    assert_eq!(
        after_first, after_second,
        "verify on a fresh file must not write; bytes diverged"
    );
}

// ---------- realm-mode-bypass exploits ----------
//
// Each test below mutates a file whose realm declares mode: strict, using
// a hand-built caller config that says mode: Open. The realm's mode is
// the source of truth; the caller's mode is irrelevant. The op must
// refuse to write under the realm's strict gate.
//
// Regression coverage: until commit_with_verify did the realm walk,
// these ops silently ran under the caller's mode and the gate did not
// fire on the realm's rules.

#[test]
fn realm_walk_passes_through_yamls_without_a_mode_field() {
    // An intermediate .remargin.yaml that only declares system_prompt
    // (no `mode:`) must NOT short-circuit the realm walk. The default
    // mode is Open; if the walk stops at this file, a strict realm
    // root one level higher is silently ignored and ops run as Open.
    let private_key = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";
    let registry_yaml = "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin
";
    let prompt_only_yaml = "\
system_prompt:
  name: Inner prompt
  prompt: |
    body
";
    let doc = "\
---
title: T
---

# H
";
    let system = MockSystem::new()
        .with_file(Path::new("/vault/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/vault/.remargin-registry.yaml"),
            registry_yaml.as_bytes(),
        )
        .unwrap()
        .with_file(
            Path::new("/vault/sub/.remargin.yaml"),
            prompt_only_yaml.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/vault/sub/doc.md"), doc.as_bytes())
        .unwrap()
        .with_file(Path::new("/keys/alice"), private_key.as_bytes())
        .unwrap();

    let mut cfg = make_config(Mode::Open, None);
    cfg.identity = Some(String::from("alice"));
    cfg.key_path = Some(PathBuf::from("/keys/alice"));

    let pos = InsertPosition::Append;
    let new_id = create_comment(
        &system,
        Path::new("/vault/sub/doc.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: Some(false),
            content: "hello",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let written = parser::parse_file(&system, Path::new("/vault/sub/doc.md")).unwrap();
    let new_cm = written.find_comment(&new_id).unwrap();
    assert!(
        new_cm.signature.is_some(),
        "an intermediate prompt-only yaml must not hijack the realm walk; \
         the strict declaration one level up still applies, so the new \
         comment must be signed"
    );
}

#[test]
fn create_comment_signs_when_realm_is_strict_even_if_caller_mode_is_open() {
    let private_key = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";
    let registry_yaml = "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin
";
    let doc = "\
---
title: T
---

# H
";
    let system = MockSystem::new()
        .with_file(Path::new("/d/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/d/.remargin-registry.yaml"),
            registry_yaml.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/d/a.md"), doc.as_bytes())
        .unwrap()
        .with_file(Path::new("/keys/alice"), private_key.as_bytes())
        .unwrap();

    // Caller mirrors what handle_comment hands to create_comment: mode
    // and registry inherited from the MCP startup walk (Open here),
    // identity + key_path resolved from the --config target.
    let mut cfg = make_config(Mode::Open, None);
    cfg.identity = Some(String::from("alice"));
    cfg.key_path = Some(PathBuf::from("/keys/alice"));

    let pos = InsertPosition::Append;
    let new_id = create_comment(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: Some(false),
            content: "hello",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let written = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();
    let new_cm = written.find_comment(&new_id).unwrap();
    assert!(
        new_cm.signature.is_some(),
        "realm is strict + alice is registered-active + a key is configured; \
         create_comment must sign before write, but signature is None"
    );
}

#[test]
fn create_comment_signs_when_realm_yaml_is_several_dirs_above_doc() {
    // Real-world shape: vault declares strict at the root; the doc lives
    // several levels deep; the registry sits one level above the vault.
    // The walks must traverse all those intermediate empty directories
    // to find both the realm config and the registry.
    let private_key = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";
    let registry_yaml = "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin
";
    let doc = "\
---
title: T
---

# H
";
    let system = MockSystem::new()
        .with_file(
            Path::new("/home/u/.remargin-registry.yaml"),
            registry_yaml.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/home/u/vault/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(Path::new("/home/u/vault/a/b/c/doc.md"), doc.as_bytes())
        .unwrap()
        .with_file(Path::new("/keys/alice"), private_key.as_bytes())
        .unwrap();

    let mut cfg = make_config(Mode::Open, None);
    cfg.identity = Some(String::from("alice"));
    cfg.key_path = Some(PathBuf::from("/keys/alice"));

    let pos = InsertPosition::Append;
    let new_id = create_comment(
        &system,
        Path::new("/home/u/vault/a/b/c/doc.md"),
        &cfg,
        &CreateCommentParams {
            attachments: &[],
            auto_ack: Some(false),
            content: "hello",
            position: &pos,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        },
    )
    .unwrap();

    let written = parser::parse_file(&system, Path::new("/home/u/vault/a/b/c/doc.md")).unwrap();
    let new_cm = written.find_comment(&new_id).unwrap();
    assert!(
        new_cm.signature.is_some(),
        "realm is strict 3 dirs up + registry is 4 dirs up + alice is \
         registered-active + a key is configured; create_comment must \
         sign before write"
    );
}

#[test]
fn ack_allowed_under_realm_strict_when_no_new_anomalies() {
    // Realm declares strict + alice registered active; the on-disk doc
    // carries an unsigned alice comment (in P). ack only touches the
    // ack list — Q has the same anomaly. Q ⊆ P → allowed.
    let before = alice_doc_content();
    let system = mock_with_doc_in_strict_realm(&before);
    let mut cfg = make_config(Mode::Open, Some(alice_active_registry()));
    cfg.identity = Some(String::from("alice"));

    ack_comments(&system, Path::new("/d/a.md"), &cfg, &["alc"], false).unwrap();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        after.contains("alice@"),
        "ack must land under subset gate: {after}"
    );
}

#[test]
fn react_allowed_under_realm_strict_when_no_new_anomalies() {
    let before = alice_doc_content();
    let system = mock_with_doc_in_strict_realm(&before);
    let mut cfg = make_config(Mode::Open, Some(alice_active_registry()));
    cfg.identity = Some(String::from("alice"));

    react(&system, Path::new("/d/a.md"), &cfg, "alc", "+1", false).unwrap();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        after.contains("+1"),
        "reaction must land under subset gate: {after}"
    );
}

#[test]
fn delete_allowed_under_realm_strict_when_no_new_anomalies() {
    // Two unsigned alice comments → P has two signature_missing entries.
    // Delete one; the surviving one's anomaly was already in P. Q ⊆ P
    // → allowed.
    let content1 = "alice's first note";
    let content2 = "alice's second note";
    let cksum1 = crypto::compute_checksum(content1, &[]);
    let cksum2 = crypto::compute_checksum(content2, &[]);
    let before = format!(
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
checksum: {cksum1}
---
{content1}
```

```remargin
---
id: alc2
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: {cksum2}
---
{content2}
```
",
    );
    let system = mock_with_doc_in_strict_realm(&before);
    let mut cfg = make_config(Mode::Open, Some(alice_active_registry()));
    cfg.identity = Some(String::from("alice"));

    delete_comments(&system, Path::new("/d/a.md"), &cfg, &["alc"]).unwrap();

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert!(
        !after.contains(content1),
        "deleted comment must be gone: {after}"
    );
    assert!(
        after.contains(content2),
        "surviving comment must remain: {after}"
    );
}

#[test]
fn commit_with_verify_passes_when_in_memory_doc_matches_disk() {
    // Identity transformation: parse the on-disk file, re-pass the
    // parsed doc back to commit_with_verify. P == Q (same anomaly
    // set), so the subset gate allows it regardless of what the
    // realm's mode says.
    let doc_content = alice_doc_content();
    let system = mock_with_doc_in_strict_realm(&doc_content);
    let mut cfg = make_config(Mode::Open, Some(alice_active_registry()));
    cfg.identity = Some(String::from("alice"));

    let doc = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();

    let result = commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_d| Ok(()));

    assert!(result.is_ok(), "Q == P → subset gate allows: {result:?}");
}

#[test]
fn escalate_mode_for_doc_keeps_caller_registry_today() {
    // The caller's registry knows alice and treats her as active.
    // The realm registry is a different one — say, empty.
    // After escalation, the resulting config's mode IS the realm's, but
    // the registry is still the caller's. So `requires_signature(alice)`
    // returns true based on the caller's registry, not the realm's.
    //
    // Per the rule (file's realm is the source of truth), the registry
    // should also come from the realm's anchor.
    let system = MockSystem::new()
        .with_file(Path::new("/realm/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/realm/.remargin-registry.yaml"),
            b"participants: {}\n",
        )
        .unwrap()
        .with_file(Path::new("/realm/file.md"), b"# doc\n")
        .unwrap();

    let mut caller = make_config(Mode::Open, Some(alice_active_registry()));
    caller.identity = Some(String::from("alice"));

    let resolved = caller
        .escalate_mode_for_doc(&system, Path::new("/realm/file.md"))
        .unwrap();

    assert!(
        !resolved.requires_signature("alice"),
        "the realm's registry is empty, so alice cannot be \
         registered-active for THIS realm; but the helper kept the \
         caller's registry, which has her as active"
    );
}

// ===========================================================================
// Subset gate (Q ⊆ P) scenarios. The gate refuses iff the in-memory
// post-mutation anomaly set introduces an entry not present in the
// on-disk pre-mutation set.
// ===========================================================================

#[test]
fn anomalies_open_mode_returns_empty_for_clean_doc() {
    let doc = doc_with(vec![make_comment("a", "alice", "hello")]);
    let cfg = make_config(Mode::Open, None);
    assert!(anomalies_for_doc(&doc, &cfg).is_empty());
}

#[test]
fn anomalies_reports_bad_checksum_under_open_mode() {
    let mut cm = make_comment("a", "alice", "hello");
    cm.checksum = String::from("sha256:deadbeef");
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Open, None);
    let set = anomalies_for_doc(&doc, &cfg);
    assert_eq!(set.len(), 1);
    assert!(set.contains(&Anomaly {
        id: String::from("a"),
        kind: AnomalyKind::ChecksumInvalid,
    }));
}

#[test]
fn anomalies_reports_missing_signature_under_strict_for_registered_active() {
    let doc = doc_with(vec![make_comment("a", "alice", "hello")]);
    let cfg = make_config(Mode::Strict, Some(alice_active_registry()));
    let set = anomalies_for_doc(&doc, &cfg);
    assert!(set.contains(&Anomaly {
        id: String::from("a"),
        kind: AnomalyKind::SignatureMissing,
    }));
}

#[test]
fn subset_gate_allows_op_when_q_equals_p() {
    // P == Q. Identity transformation: rewrite the same bytes.
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
    let doc = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();

    commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_| Ok(())).unwrap();
}

#[test]
fn subset_gate_refuses_op_that_introduces_new_anomaly() {
    // Pre-clean doc → P = ∅. Synthesise an in-memory doc with a bad
    // checksum on a NEW comment. Q = {(b, checksum_invalid)}. Q ⊄ P
    // → refuse with SubsetGateFailure.
    let before = "\
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
";
    let system = mock_with_doc(before);
    let cfg = open_cfg_as("alice");

    // Hand-craft an in-memory doc that introduces a new bad-checksum
    // comment.
    let mut new = make_comment("b", "alice", "new comment");
    new.checksum = String::from("sha256:deadbeef");
    let doc = doc_with(vec![
        // Re-parse the existing alice comment so its checksum stays good.
        parser::parse_file(&system, Path::new("/d/a.md"))
            .unwrap()
            .find_comment("alc")
            .unwrap()
            .clone(),
        new,
    ]);

    let err =
        commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_| Ok(())).unwrap_err();
    let sg = err.downcast_ref::<SubsetGateFailure>().unwrap();
    assert_eq!(sg.introduced.len(), 1);
    assert_eq!(sg.introduced[0].id, "b");
    assert_eq!(sg.introduced[0].kind, AnomalyKind::ChecksumInvalid);
}

#[test]
fn subset_gate_allows_repair_op_that_reduces_anomaly_set() {
    // P contains a bad checksum. The op produces a clean doc (Q = ∅).
    // Q ⊆ P → allowed.
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

    // Hand a clean in-memory doc to the gate (the checksum now matches
    // the content).
    let doc = doc_with(vec![make_comment("alc", "alice", "alice's note")]);
    commit_with_verify(&system, &doc, &cfg, Path::new("/d/a.md"), |_| Ok(())).unwrap();
}

#[test]
fn subset_gate_allows_op_when_pre_file_missing_and_post_is_clean() {
    // Fresh file (P = ∅). Clean Q. Q ⊆ P (both empty). Allowed.
    let system = MockSystem::new();
    let cfg = open_cfg_as("alice");
    let doc = doc_with(vec![make_comment("alc", "alice", "alice's note")]);
    commit_with_verify(&system, &doc, &cfg, Path::new("/d/new.md"), |_| Ok(())).unwrap();
}

// Recipient status in verify.

#[test]
fn anomaly_kind_pair_distinguishes_checksum_from_signature() {
    // Same id, different kind = different anomaly identity.
    let mut cm_bad_checksum = make_comment("a", "alice", "hello");
    cm_bad_checksum.checksum = String::from("sha256:deadbeef");
    let doc_bad_checksum = doc_with(vec![cm_bad_checksum]);
    let cfg = make_config(Mode::Open, None);
    let p = anomalies_for_doc(&doc_bad_checksum, &cfg);
    assert!(p.contains(&Anomaly {
        id: String::from("a"),
        kind: AnomalyKind::ChecksumInvalid,
    }));
    assert!(!p.contains(&Anomaly {
        id: String::from("a"),
        kind: AnomalyKind::SignatureMissing,
    }));
}

fn make_comment_with_to(id: &str, author: &str, content: &str, to: Vec<String>) -> Comment {
    let mut cm = make_comment(id, author, content);
    cm.to = to;
    cm
}

fn recipient_verify_registry() -> Registry {
    registry_with(RECIPIENT_VERIFY_REGISTRY)
}

/// Scenario 18: unknown recipient in strict → row bad, report.ok = false.
#[test]
fn verify_unknown_recipient_strict_row_bad() {
    let reg = recipient_verify_registry();
    let cm = make_comment_with_to("a", "alice", "hi", vec![String::from("eduardo_burgos")]);
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Strict, Some(reg));
    let report = verify_document(&doc, &cfg);
    assert!(
        !report.ok,
        "unknown recipient in strict should make ok=false"
    );
    assert_eq!(
        report.results[0].recipients,
        RecipientStatus::Unknown(vec!["eduardo_burgos".to_owned()]),
        "row recipients should be Unknown with the bad name"
    );
}

/// Scenario 19: unknown recipient in registered → row bad.
#[test]
fn verify_unknown_recipient_registered_row_bad() {
    let reg = recipient_verify_registry();
    let cm = make_comment_with_to("a", "alice", "hi", vec![String::from("eduardo_burgos")]);
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Registered, Some(reg));
    let report = verify_document(&doc, &cfg);
    assert!(
        !report.ok,
        "unknown recipient in registered should make ok=false"
    );
    assert_eq!(
        report.results[0].recipients,
        RecipientStatus::Unknown(vec!["eduardo_burgos".to_owned()])
    );
}

/// Scenario 20: unknown recipient in open → row neutral (ok=true).
#[test]
fn verify_unknown_recipient_open_row_neutral() {
    let reg = recipient_verify_registry();
    let cm = make_comment_with_to("a", "alice", "hi", vec![String::from("eduardo_burgos")]);
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Open, Some(reg));
    let report = verify_document(&doc, &cfg);
    // Open mode: unknown recipient is neutral (not bad unless checksum/sig fails).
    // alice has no keys so signature is Missing, and Missing is neutral in Open.
    assert!(report.ok, "open mode: unknown recipient is neutral");
    assert_eq!(
        report.results[0].recipients,
        RecipientStatus::Unknown(vec!["eduardo_burgos".to_owned()]),
        "recipient status is still Unknown even in open mode"
    );
}

/// Scenario 21: revoked recipient in strict → row bad.
#[test]
fn verify_revoked_recipient_strict_row_bad() {
    let reg = recipient_verify_registry();
    let cm = make_comment_with_to("a", "alice", "hi", vec![String::from("bob")]);
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Strict, Some(reg));
    let report = verify_document(&doc, &cfg);
    assert!(
        !report.ok,
        "revoked recipient in strict should make ok=false"
    );
    assert_eq!(
        report.results[0].recipients,
        RecipientStatus::Unknown(vec!["bob".to_owned()])
    );
}

/// Scenario 22: empty to: in strict → row ok.
#[test]
fn verify_empty_to_strict_row_ok() {
    let reg = recipient_verify_registry();
    let cm = make_comment("a", "alice", "hi"); // to: Vec::new() by default
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Strict, Some(reg));
    let report = verify_document(&doc, &cfg);
    // Broadcast — no recipients to check, recipients = Ok.
    assert_eq!(report.results[0].recipients, RecipientStatus::Ok);
}

/// Scenario 23: `commit_with_verify` allows write that fixes a bad recipient
/// (Q ⊂ P: removes the bad recipient anomaly from the pre-state).
#[test]
fn commit_with_verify_repair_removes_bad_recipient_succeeds() {
    let reg_yaml = RECIPIENT_VERIFY_REGISTRY;
    let pre_doc = doc_with(vec![make_comment_with_to(
        "a",
        "alice",
        "hi",
        vec![String::from("eduardo_burgos")],
    )]);
    let pre_md = pre_doc.to_markdown().unwrap();

    let system = MockSystem::new()
        .with_file(Path::new("/d/a.md"), pre_md.as_bytes())
        .unwrap()
        .with_file(Path::new("/d/.remargin.yaml"), b"mode: registered\n")
        .unwrap()
        .with_file(Path::new("/d/.remargin-registry.yaml"), reg_yaml.as_bytes())
        .unwrap();

    let cfg = make_config(Mode::Registered, Some(registry_with(reg_yaml)));

    // Post-state: same comment but with a valid recipient.
    let post_doc = doc_with(vec![make_comment_with_to(
        "a",
        "alice",
        "hi",
        vec![String::from("eduardo-burgos")],
    )]);

    // Repair: P has RecipientUnknown for "a"; Q does not → Q ⊆ P.
    commit_with_verify(&system, &post_doc, &cfg, Path::new("/d/a.md"), |_| Ok(())).unwrap();
}

/// Scenario 24: `commit_with_verify` blocks write that introduces a bad recipient.
#[test]
fn commit_with_verify_introducing_bad_recipient_blocked() {
    let reg_yaml = RECIPIENT_VERIFY_REGISTRY;
    let system = MockSystem::new()
        .with_file(Path::new("/d/a.md"), b"---\ntitle: Test\n---\n\n# Hello\n")
        .unwrap()
        .with_file(Path::new("/d/.remargin.yaml"), b"mode: registered\n")
        .unwrap()
        .with_file(Path::new("/d/.remargin-registry.yaml"), reg_yaml.as_bytes())
        .unwrap();

    let cfg = make_config(Mode::Registered, Some(registry_with(reg_yaml)));

    // Post-state: comment with unknown recipient — introduces RecipientUnknown.
    let post_doc = doc_with(vec![make_comment_with_to(
        "a",
        "alice",
        "hi",
        vec![String::from("eduardo_burgos")],
    )]);

    let err =
        commit_with_verify(&system, &post_doc, &cfg, Path::new("/d/a.md"), |_| Ok(())).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("recipient_unknown") || msg.contains("anomaly"),
        "gate should refuse write introducing bad recipient: {msg}"
    );
}

/// `RecipientStatus::as_str` renders correctly.
#[test]
fn recipient_status_as_str() {
    assert_eq!(RecipientStatus::Ok.as_str(), "ok");
    assert_eq!(RecipientStatus::Unknown(vec![]).as_str(), "unknown");
}

/// `AnomalyKind::RecipientUnknown` is present in the anomaly set for a doc
/// with an unknown recipient in registered mode.
#[test]
fn anomaly_kind_recipient_unknown_in_anomaly_set() {
    let reg = recipient_verify_registry();
    let cm = make_comment_with_to("a", "alice", "hi", vec![String::from("nobody")]);
    let doc = doc_with(vec![cm]);
    let cfg = make_config(Mode::Registered, Some(reg));
    let anomalies = anomalies_for_doc(&doc, &cfg);
    assert!(
        anomalies.contains(&Anomaly {
            id: String::from("a"),
            kind: AnomalyKind::RecipientUnknown,
        }),
        "anomaly set should include RecipientUnknown: {anomalies:?}"
    );
}
