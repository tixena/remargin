//! Tests for the `sign_comments` operation (rem-1ec).
//!
//! Coverage matrix:
//! - happy path: unsigned comment authored by caller gets signed and
//!   the signature verifies byte-identically against the registry pubkey
//! - idempotency: running sign twice yields zero signed / every id under
//!   skipped on the second pass
//! - forgery guard: `--ids` entry authored by a different participant is
//!   a hard error before any byte hits disk
//! - not-found: `--ids` entry that does not exist errors out
//! - already-signed under `--ids`: reported as skipped, not signed again
//! - `--all-mine` excludes non-owned and already-signed silently
//! - no-key: sign with a config that has no resolvable key is a hard
//!   error regardless of mode (stricter than create/edit's fail-fast —
//!   sign without a key has nothing to do)
//!
//! Dry-run coverage: the per-op `--dry-run` flag was removed in rem-0ry
//! in favour of `remargin plan sign`. The projection test lives in
//! `operations/tests.rs::project_sign_*`.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::registry::Registry;
use crate::config::{Mode, ResolvedConfig};
use crate::crypto::{self, compute_signature};
use crate::operations::sign::{SignOptions, SignSelection, sign_comments};
use crate::parser::{self, AuthorType, Comment, Segment};

// ---- Test key pair -----------------------------------------------------
//
// Matched ed25519 key pair copied from `crate::crypto::tests`. Using the
// exact same pair keeps sign's round-trip behaviour identical to what
// verify_signature tests already pin down — a signature produced by
// TEST_PRIVATE_KEY verifies against TEST_PUBLIC_KEY, which is what the
// registry's `pubkeys` list holds.

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

// ---- Fixtures ---------------------------------------------------------

fn registry_with_alice_pubkey() -> Registry {
    let yaml = format!(
        "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - {TEST_PUBLIC_KEY}
  bob:
    type: human
    status: active
    pubkeys:
      - {TEST_PUBLIC_KEY}
",
    );
    serde_yaml::from_str(&yaml).unwrap()
}

fn make_config(mode: Mode, identity: &str, key_path: Option<&str>) -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from(identity)),
        ignore: Vec::new(),
        key_path: key_path.map(PathBuf::from),
        mode,
        registry: Some(registry_with_alice_pubkey()),
        source_path: None,
        unrestricted: false,
    }
}

/// Two-comment document: one authored by alice, one by bob, both
/// unsigned. Checksums are computed so the verify gate stays neutral on
/// them (modulo the signature status, which is what sign changes).
fn two_author_doc() -> String {
    let alice_content = "alice's note";
    let bob_content = "bob's note";
    let alice_cksum = crypto::compute_checksum(alice_content, &[]);
    let bob_cksum = crypto::compute_checksum(bob_content, &[]);
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
checksum: {alice_cksum}
---
{alice_content}
```

```remargin
---
id: bob
author: bob
type: human
ts: 2026-04-06T13:00:00-04:00
checksum: {bob_cksum}
---
{bob_content}
```
",
    )
}

/// Single-comment doc, already signed with the matching test key.
/// Useful for idempotency / skip-already-signed cases.
fn pre_signed_doc(system_for_signing: &MockSystem) -> String {
    let content = "alice's note";
    let cksum = crypto::compute_checksum(content, &[]);

    let comment = Comment {
        ack: Vec::new(),
        attachments: Vec::new(),
        author: String::from("alice"),
        author_type: AuthorType::Human,
        checksum: cksum.clone(),
        content: String::from(content),
        id: String::from("alc"),
        line: 0,
        reactions: BTreeMap::new(),
        remargin_kind: None,
        reply_to: None,
        signature: None,
        thread: None,
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339("2026-04-06T12:00:00-04:00").unwrap(),
    };
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), system_for_signing).unwrap();

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
signature: {sig}
---
{content}
```
",
    )
}

fn mock_with(content: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/keys/ed25519"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap()
        .with_file(Path::new("/d/a.md"), content.as_bytes())
        .unwrap()
}

// ---- Happy path --------------------------------------------------------

#[test]
fn sign_all_mine_writes_signature_for_owned_unsigned_comments() {
    let system = mock_with(&two_author_doc());
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::AllMine,
        SignOptions::default(),
    )
    .unwrap();

    assert_eq!(result.signed.len(), 1, "should sign alice's comment only");
    assert_eq!(result.signed[0].id, "alc");
    assert_eq!(
        result.skipped.len(),
        0,
        "all-mine never reports non-owned ids as skipped"
    );

    // Post-state: the on-disk doc must now carry a signature on alc and
    // still none on bob.
    let doc = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();
    let alc = doc.find_comment("alc").unwrap();
    let bob = doc.find_comment("bob").unwrap();
    assert!(
        alc.signature.is_some(),
        "alice's comment must be signed after sign --all-mine"
    );
    assert!(
        bob.signature.is_none(),
        "bob's comment must remain unsigned"
    );
}

#[test]
fn sign_ids_signs_listed_comments() {
    let system = mock_with(&two_author_doc());
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("alc")]),
        SignOptions::default(),
    )
    .unwrap();

    assert_eq!(result.signed.len(), 1);
    assert_eq!(result.signed[0].id, "alc");

    let doc = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();
    assert!(doc.find_comment("alc").unwrap().signature.is_some());
}

// ---- Forgery guard ----------------------------------------------------

#[test]
fn sign_ids_foreign_author_is_hard_error() {
    let before = two_author_doc();
    let system = mock_with(&before);
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    // alice tries to sign bob's comment — cryptographic forgery. Must
    // bail before any byte hits disk.
    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("bob")]),
        SignOptions::default(),
    );

    let err = result.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("forgery guard"),
        "must mention the forgery guard, got: {msg}"
    );
    assert!(
        msg.contains("bob") && msg.contains("alice"),
        "must name both the comment's author and the caller, got: {msg}"
    );

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(after, before, "forgery-guard refusal must not touch disk");
}

#[test]
fn sign_ids_missing_id_is_hard_error() {
    let before = two_author_doc();
    let system = mock_with(&before);
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("ghost")]),
        SignOptions::default(),
    );

    let err = result.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not found"),
        "missing id must produce a clear diagnosis, got: {msg}"
    );

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(after, before);
}

// ---- Already-signed behaviour ----------------------------------------

#[test]
fn sign_ids_already_signed_reported_as_skipped() {
    // Pre-build a pre-signed doc (using the same key pair) so the
    // caller lists an id that is already signed under `--ids`. The op
    // must NOT re-sign; it must report it as skipped with the canonical
    // reason string.
    let pre_system = MockSystem::new()
        .with_file(Path::new("/keys/ed25519"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap();
    let doc_text = pre_signed_doc(&pre_system);
    let system = mock_with(&doc_text);
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("alc")]),
        SignOptions::default(),
    )
    .unwrap();

    assert_eq!(
        result.signed.len(),
        0,
        "already-signed must not be re-signed"
    );
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].id, "alc");
    assert_eq!(result.skipped[0].reason, "already_signed");
}

#[test]
fn sign_all_mine_is_idempotent() {
    let system = mock_with(&two_author_doc());
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    // First run: signs alice's one comment.
    let r1 = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::AllMine,
        SignOptions::default(),
    )
    .unwrap();
    assert_eq!(r1.signed.len(), 1);

    // Second run: alice has no unsigned comments left. --all-mine is
    // a filter, so already-signed ids are silently excluded.
    let r2 = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::AllMine,
        SignOptions::default(),
    )
    .unwrap();
    assert_eq!(r2.signed.len(), 0, "second pass must sign nothing");
    assert_eq!(
        r2.skipped.len(),
        0,
        "--all-mine must not report non-candidates as skipped"
    );
}

// ---- No key -----------------------------------------------------------

#[test]
fn sign_without_resolvable_key_is_hard_error() {
    // Unlike create / edit (which route through `resolve_signing_key`
    // and get `Ok(None)` in non-strict modes), `sign` has a stricter
    // pre-condition: without a key there is literally nothing to do.
    // The op must bail with an actionable message regardless of mode.
    let before = two_author_doc();
    let system = mock_with(&before);
    let mut cfg = make_config(Mode::Registered, "alice", None);
    cfg.key_path = None;

    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::AllMine,
        SignOptions::default(),
    );

    let err = result.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("no signing key") || msg.contains("no signing key resolved"),
        "error must mention the missing key, got: {msg}"
    );

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(after, before, "no-key refusal must not touch disk");
}

// ---- Parser sanity: signed comment round-trips through verify ----------

#[test]
fn signed_comment_survives_reparse_with_signature() {
    // Sanity guard: after sign + write + re-parse, the `signature:` field
    // is still attached to the comment (not dropped on serialize). This
    // is what makes the idempotency and skip-already-signed tests above
    // meaningful.
    let system = mock_with(&two_author_doc());
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::AllMine,
        SignOptions::default(),
    )
    .unwrap();

    let doc = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();
    let signed_count = doc
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Comment(cm) if cm.signature.is_some()))
        .count();
    assert_eq!(signed_count, 1, "exactly one signed comment expected");
}

// ---- repair_checksum (rem-x072) --------------------------------------

/// Substitute every occurrence of `needle` inside a `remargin` fence in
/// `doc` with `replacement`. Simulates an out-of-band edit (e.g. a text
/// editor, rsync merge, or hand-patched diff) that modifies a comment's
/// stored `content` without touching the `checksum` field — exactly the
/// shape that `--repair-checksum` is designed to re-vouch for.
fn edit_fence_content_in_place(doc: &str, needle: &str, replacement: &str) -> String {
    doc.replace(needle, replacement)
}

#[test]
fn sign_refuses_tampered_content_without_repair_checksum() {
    // Baseline for the repair_checksum tests below: a stale checksum
    // trips the verify gate, and sign without --repair-checksum refuses
    // to write. Keeping the safety rail is the default behavior; the
    // repair flag is an explicit opt-in.
    let system = mock_with(&two_author_doc());
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    // Out-of-band edit to alice's comment body.
    let before = system.read_to_string(Path::new("/d/a.md")).unwrap();
    let tampered = edit_fence_content_in_place(&before, "alice's note", "alice's NOTE (edited)");
    assert_ne!(before, tampered, "tamper must actually change bytes");
    system
        .write(Path::new("/d/a.md"), tampered.as_bytes())
        .unwrap();

    let err = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("alc")]),
        SignOptions::default(),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("checksum") || msg.to_lowercase().contains("verify"),
        "stale checksum must surface in the refusal, got: {msg}"
    );

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(after, tampered, "verify-gate refusal must not touch disk");
}

#[test]
fn sign_with_repair_checksum_rewrites_stale_checksum_and_signs() {
    // Full scenario:
    //   1. Place a comment (two_author_doc seeds alice's "alice's note"
    //      with a matching checksum).
    //   2. Something alters the comment (simulated here by a direct
    //      byte substitution on the on-disk file, as an editor or
    //      merge tool would).
    //   3. Sign the comment with --repair-checksum.
    //   4. The stored checksum is recomputed from the new content,
    //      the signature is attached, and verify passes.
    let system = mock_with(&two_author_doc());
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    // Step 2: out-of-band edit to alice's comment body. Bob's comment
    // is untouched and must remain untouched post-sign.
    let before = system.read_to_string(Path::new("/d/a.md")).unwrap();
    let tampered = edit_fence_content_in_place(&before, "alice's note", "alice's NOTE (edited)");
    system
        .write(Path::new("/d/a.md"), tampered.as_bytes())
        .unwrap();

    // Capture the stale checksum for the assertion on the repair entry.
    let pre_sign = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();
    let stale_cksum = pre_sign.find_comment("alc").unwrap().checksum.clone();
    let fresh_cksum = crypto::compute_checksum("alice's NOTE (edited)", &[]);
    assert_ne!(
        stale_cksum, fresh_cksum,
        "sanity: tampered content must diverge from the stored checksum"
    );

    // Step 3: sign with repair_checksum = true.
    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("alc")]),
        SignOptions {
            repair_checksum: true,
        },
    )
    .unwrap();

    // Result assertions.
    assert_eq!(result.signed.len(), 1, "alice's comment should be signed");
    assert_eq!(result.signed[0].id, "alc");
    assert_eq!(
        result.repaired.len(),
        1,
        "exactly one comment's checksum must be reported as repaired"
    );
    assert_eq!(result.repaired[0].id, "alc");
    assert_eq!(result.repaired[0].old_checksum, stale_cksum);
    assert_eq!(result.repaired[0].new_checksum, fresh_cksum);

    // Step 4: on-disk post-state.
    let after = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();
    let alc = after.find_comment("alc").unwrap();
    assert_eq!(
        alc.content, "alice's NOTE (edited)",
        "the tampered content must survive the repair"
    );
    assert_eq!(alc.checksum, fresh_cksum, "checksum must now match content");
    assert!(
        alc.signature.is_some(),
        "repaired comment must carry a fresh signature"
    );

    // Bob's comment is untouched by the repair.
    let bob = after.find_comment("bob").unwrap();
    assert!(
        bob.signature.is_none(),
        "bob's comment must remain unsigned (not in target set)"
    );
}

#[test]
fn sign_with_repair_checksum_on_already_valid_checksum_reports_no_repair() {
    // When the stored checksum already matches the current content the
    // repair path is a no-op — the signature is still written, but the
    // `repaired` list stays empty. Regression guard: we should not
    // spuriously mark every signed comment as "repaired".
    let system = mock_with(&two_author_doc());
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("alc")]),
        SignOptions {
            repair_checksum: true,
        },
    )
    .unwrap();

    assert_eq!(result.signed.len(), 1);
    assert!(
        result.repaired.is_empty(),
        "no-op repair must not produce a repaired entry, got: {:?}",
        result.repaired
    );
}

#[test]
fn sign_with_repair_checksum_overwrites_stale_signature_on_tampered_comment() {
    // The real-world shape (the scenario that drove rem-x072): a
    // comment was signed at creation, then edited out-of-band. The
    // stored signature is now invalid (covers pre-edit content) and
    // the stored checksum is stale. Default sign skips the comment
    // with reason="already_signed" because `signature.is_some()`;
    // under --repair-checksum the op is supposed to re-vouch, which
    // means overwriting both fields.
    let pre_system = MockSystem::new()
        .with_file(Path::new("/keys/ed25519"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap();
    let pre_signed = pre_signed_doc(&pre_system);
    let system = mock_with(&pre_signed);
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    // Tamper with the content after signing. Now checksum is stale
    // and signature no longer matches the current content.
    let tampered =
        edit_fence_content_in_place(&pre_signed, "alice's note", "alice's NOTE (edited)");
    system
        .write(Path::new("/d/a.md"), tampered.as_bytes())
        .unwrap();

    // Sanity: baseline sign (no repair flag) classifies it as
    // already-signed and skips — exactly the behavior the user hit in
    // production.
    let baseline = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("alc")]),
        SignOptions::default(),
    );
    // Default sign bails in the verify gate because the stale
    // checksum trips `row_is_bad`; the skip list never gets emitted.
    assert!(
        baseline.is_err(),
        "default sign (no repair) must refuse a doc with stale checksum/signature"
    );

    // With --repair-checksum: overwrite both fields, write, verify
    // passes.
    let result = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("alc")]),
        SignOptions {
            repair_checksum: true,
        },
    )
    .unwrap();

    assert_eq!(
        result.signed.len(),
        1,
        "already-signed comment must be re-signed under --repair-checksum"
    );
    assert_eq!(result.signed[0].id, "alc");
    assert_eq!(
        result.skipped.len(),
        0,
        "skip list must be empty with --repair-checksum: the flag supersedes the already_signed skip rule"
    );
    assert_eq!(result.repaired.len(), 1, "checksum repair must be reported");

    let after = parser::parse_file(&system, Path::new("/d/a.md")).unwrap();
    let alc = after.find_comment("alc").unwrap();
    assert_eq!(alc.content, "alice's NOTE (edited)");
    assert_eq!(
        alc.checksum,
        crypto::compute_checksum("alice's NOTE (edited)", &[])
    );
    assert!(alc.signature.is_some());
    // The new signature must verify against the registry pubkey
    // (round-trip guard — re-signing with the matched key produces a
    // valid signature over the current content).
    let sig_ok = crypto::verify_signature(alc, TEST_PUBLIC_KEY).unwrap();
    assert!(
        sig_ok,
        "re-signed signature must verify over the new content"
    );
}

#[test]
fn sign_forgery_guard_blocks_repair_on_foreign_comment() {
    // repair_checksum does not bypass the forgery guard. alice cannot
    // repair (or sign) bob's comment — the op bails before any byte
    // hits disk, even with the repair flag on.
    let before = two_author_doc();
    let system = mock_with(&before);
    let cfg = make_config(Mode::Registered, "alice", Some("/keys/ed25519"));

    // Out-of-band edit to bob's comment; alice tries to repair+sign it.
    let tampered = edit_fence_content_in_place(&before, "bob's note", "bob's NOTE (edited)");
    system
        .write(Path::new("/d/a.md"), tampered.as_bytes())
        .unwrap();

    let err = sign_comments(
        &system,
        Path::new("/d/a.md"),
        &cfg,
        &SignSelection::Ids(vec![String::from("bob")]),
        SignOptions {
            repair_checksum: true,
        },
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("forgery guard"),
        "forgery guard must still fire with --repair-checksum, got: {msg}"
    );

    let after = system.read_to_string(Path::new("/d/a.md")).unwrap();
    assert_eq!(
        after, tampered,
        "forgery-guard refusal must not touch disk even when repair was requested"
    );
}
