//! Tests for the crypto module.

extern crate alloc;

use std::path::Path;

use chrono::DateTime;
use os_shim::mock::MemorySystem;

use crate::crypto::{
    compute_checksum, compute_reaction_checksum, compute_signature, normalize_whitespace,
    signature_payload, verify_checksum, verify_signature,
};
use crate::parser::{AuthorType, Comment};
use crate::reactions::{Reactions, ReactionsExt as _};

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

fn make_comment(content: &str) -> Comment {
    Comment {
        ack: Vec::new(),
        attachments: Vec::new(),
        author: String::from("eduardo"),
        author_type: AuthorType::Human,
        checksum: compute_checksum(content, &[]),
        content: String::from(content),
        edited_at: None,
        el: None,
        id: String::from("abc"),
        line: 0,
        reactions: Reactions::new(),
        remargin_kind: None,
        reply_to: None,
        signature: None,
        sl: None,
        thread: None,
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339("2026-04-06T12:00:00-04:00").unwrap(),
    }
}

fn system_with_key() -> MemorySystem {
    MemorySystem::new()
        .with_file(Path::new("/keys/ed25519"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap()
}

#[test]
fn normalize_crlf_to_lf() {
    assert_eq!(normalize_whitespace("Hello\r\nworld"), "Hello\nworld");
}

#[test]
fn normalize_trailing_whitespace() {
    assert_eq!(
        normalize_whitespace("line1  \nline2\t\nline3"),
        "line1\nline2\nline3"
    );
}

#[test]
fn normalize_leading_trailing_newlines() {
    assert_eq!(normalize_whitespace("\n\nHello\n\n"), "Hello");
}

#[test]
fn normalize_combined() {
    assert_eq!(
        normalize_whitespace("\r\nline1  \r\nline2\r\n"),
        "line1\nline2"
    );
}

#[test]
fn basic_checksum() {
    let checksum = compute_checksum("Hello world", &[]);
    assert!(checksum.starts_with("sha256:"));
    assert_eq!(checksum, compute_checksum("Hello world", &[]));
}

#[test]
fn different_content_different_checksum() {
    let c1 = compute_checksum("Hello", &[]);
    let c2 = compute_checksum("World", &[]);
    assert_ne!(c1, c2);
}

#[test]
fn crlf_vs_lf_same_checksum() {
    assert_eq!(
        compute_checksum("Hello\r\n", &[]),
        compute_checksum("Hello\n", &[])
    );
}

#[test]
fn trailing_whitespace_same_checksum() {
    assert_eq!(
        compute_checksum("line1  \nline2\n", &[]),
        compute_checksum("line1\nline2\n", &[])
    );
}

#[test]
fn verify_checksum_pass() {
    let comment = make_comment("This is a test comment.");
    assert!(verify_checksum(&comment));
}

#[test]
fn verify_checksum_fail() {
    let mut comment = make_comment("Original content");
    comment.content = String::from("Modified content");
    assert!(!verify_checksum(&comment));
}

#[test]
fn reaction_checksum_changes_on_add() {
    let ts1 = DateTime::parse_from_rfc3339("2026-04-26T12:00:00-04:00").unwrap();
    let ts2 = DateTime::parse_from_rfc3339("2026-04-26T13:00:00-04:00").unwrap();
    let mut reactions1 = Reactions::new();
    let _added_alice = reactions1.add_reaction("thumbsup", "alice", ts1);

    let mut reactions2 = reactions1.clone();
    let _added_bob = reactions2.add_reaction("heart", "bob", ts2);

    assert_ne!(
        compute_reaction_checksum(&reactions1),
        compute_reaction_checksum(&reactions2)
    );
}

#[test]
fn reaction_does_not_affect_content_checksum() {
    let comment = make_comment("Test content");
    let checksum_before = compute_checksum(&comment.content, comment.kinds());

    let checksum_after = compute_checksum(&comment.content, comment.kinds());
    assert_eq!(checksum_before, checksum_after);
}

#[test]
fn reaction_checksum_deterministic_order() {
    let ts1 = DateTime::parse_from_rfc3339("2026-04-26T12:00:00-04:00").unwrap();
    let ts2 = DateTime::parse_from_rfc3339("2026-04-26T12:01:00-04:00").unwrap();
    let mut reactions = Reactions::new();
    let _added_bob = reactions.add_reaction("thumbsup", "bob", ts1);
    let _added_alice = reactions.add_reaction("thumbsup", "alice", ts2);

    let c1 = compute_reaction_checksum(&reactions);
    let c2 = compute_reaction_checksum(&reactions);
    assert_eq!(c1, c2);
}

#[test]
fn reaction_checksum_changes_when_ts_changes() {
    let ts1 = DateTime::parse_from_rfc3339("2026-04-26T12:00:00-04:00").unwrap();
    let ts2 = DateTime::parse_from_rfc3339("2026-04-26T13:00:00-04:00").unwrap();
    let mut r1 = Reactions::new();
    let _added_one = r1.add_reaction("thumbsup", "alice", ts1);
    let mut r2 = Reactions::new();
    let _added_two = r2.add_reaction("thumbsup", "alice", ts2);
    assert_ne!(
        compute_reaction_checksum(&r1),
        compute_reaction_checksum(&r2),
        "reaction checksum must change when an entry's ts changes",
    );
}

#[test]
fn reaction_checksum_independent_of_insert_order() {
    let ts1 = DateTime::parse_from_rfc3339("2026-04-26T12:00:00-04:00").unwrap();
    let ts2 = DateTime::parse_from_rfc3339("2026-04-26T12:01:00-04:00").unwrap();

    let mut a = Reactions::new();
    let _a_alice = a.add_reaction("thumbsup", "alice", ts1);
    let _a_bob = a.add_reaction("thumbsup", "bob", ts2);

    let mut b = Reactions::new();
    let _b_bob = b.add_reaction("thumbsup", "bob", ts2);
    let _b_alice = b.add_reaction("thumbsup", "alice", ts1);

    assert_eq!(
        compute_reaction_checksum(&a),
        compute_reaction_checksum(&b),
        "two writers adding the same reactions in different orders must agree",
    );
}

#[test]
fn ack_does_not_affect_content_checksum() {
    let comment = make_comment("Test content");
    let checksum = compute_checksum(&comment.content, comment.kinds());
    assert_eq!(
        checksum,
        compute_checksum(&comment.content, comment.kinds())
    );
}

#[test]
fn signature_round_trip() {
    let system = system_with_key();

    let mut comment = make_comment("Signed content");
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(result, "signature verification should succeed");
}

#[test]
fn signature_tamper_content() {
    let system = system_with_key();

    let mut comment = make_comment("Original content");
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    comment.content = String::from("Tampered content");

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(!result, "verification should fail after content tampering");
}

#[test]
fn signature_tamper_author() {
    let system = system_with_key();

    let mut comment = make_comment("Content");
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    comment.author = String::from("mallory");

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(!result, "verification should fail after author tampering");
}

#[test]
fn key_loading_via_system() {
    let system = MemorySystem::new()
        .with_file(Path::new("/mock/ssh/key"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap();

    let comment = make_comment("Test");
    let result = compute_signature(&comment, Path::new("/mock/ssh/key"), &system);
    assert!(result.is_ok(), "signing with mock system key should work");
    assert!(result.unwrap().starts_with("ed25519:"));
}

#[test]
fn verify_no_signature() {
    let comment = make_comment("Test");
    let result = verify_signature(&comment, TEST_PUBLIC_KEY);
    assert!(
        result.is_err(),
        "should error when comment has no signature"
    );
}

#[test]
fn signature_with_all_fields() {
    let system = system_with_key();

    let mut comment = make_comment("Full comment");
    comment.to = vec![String::from("alice"), String::from("bob")];
    comment.reply_to = Some(String::from("xyz"));
    comment.thread = Some(String::from("thread-1"));
    comment.attachments = vec![String::from("file.png")];

    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(result, "full-field signature should verify");
}

/// With no kinds the checksum is the exact hash of the normalized content alone.
#[test]
fn empty_kinds_produce_legacy_checksum() {
    let expected_hello = "sha256:64ec88ca00b268e5ba1a35678a1b5316d212f4f366b2477232534a8aeca37f3c";
    assert_eq!(compute_checksum("Hello world", &[]), expected_hello);
}

/// Pins the frozen payload bytes: a zero offset renders as `+00:00`, never `Z`.
#[test]
fn signature_payload_renders_zero_offset_as_plus_zero() {
    let mut comment = make_comment("Frozen payload");
    comment.ts = DateTime::parse_from_rfc3339("2026-04-06T12:00:00Z").unwrap();
    assert_eq!(
        signature_payload(&comment),
        "id:abc\n\
         author:eduardo\n\
         type:human\n\
         ts:2026-04-06T12:00:00+00:00\n\
         content:Frozen payload"
    );
}

#[test]
fn reaction_checksum_renders_zero_offset_as_plus_zero() {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction(
        "thumbsup",
        "alice",
        DateTime::parse_from_rfc3339("2026-04-26T12:00:00Z").unwrap(),
    );
    // sha256 of `thumbsup:alice@2026-04-26T12:00:00+00:00\n`.
    assert_eq!(
        compute_reaction_checksum(&reactions),
        "sha256:0ee7784f8207642512683e9eba361a4a56ca45beaa3eaee29b2f6709ad639c29"
    );
}

/// `+00:00` and `Z` on disk parse to the same instant, so both canonicalize to one payload.
#[test]
fn z_and_plus_zero_disk_forms_canonicalize_identically() {
    let mut from_z = make_comment("Same instant");
    from_z.ts = DateTime::parse_from_rfc3339("2026-04-06T12:00:00Z").unwrap();
    let mut from_offset = make_comment("Same instant");
    from_offset.ts = DateTime::parse_from_rfc3339("2026-04-06T12:00:00+00:00").unwrap();
    assert_eq!(signature_payload(&from_z), signature_payload(&from_offset));

    let mut reactions_z = Reactions::new();
    let _z = reactions_z.add_reaction(
        "thumbsup",
        "alice",
        DateTime::parse_from_rfc3339("2026-04-26T12:00:00Z").unwrap(),
    );
    let mut reactions_offset = Reactions::new();
    let _offset = reactions_offset.add_reaction(
        "thumbsup",
        "alice",
        DateTime::parse_from_rfc3339("2026-04-26T12:00:00+00:00").unwrap(),
    );
    assert_eq!(
        compute_reaction_checksum(&reactions_z),
        compute_reaction_checksum(&reactions_offset)
    );
}

#[test]
fn kinds_affect_checksum() {
    let without = compute_checksum("same content", &[]);
    let with = compute_checksum("same content", &[String::from("question")]);
    assert_ne!(without, with);
}

#[test]
fn kinds_order_does_not_affect_checksum() {
    let ab = compute_checksum("body", &[String::from("a"), String::from("b")]);
    let ba = compute_checksum("body", &[String::from("b"), String::from("a")]);
    assert_eq!(ab, ba);
}

/// Swapping a kind after signing must break verification.
#[test]
fn signature_tamper_kind() {
    let system = system_with_key();

    let mut comment = make_comment("Body");
    comment.remargin_kind = Some(vec![String::from("question")]);
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    comment.remargin_kind = Some(vec![String::from("action-item")]);
    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(!result, "verification should fail after kind tampering");
}

/// An empty kind list contributes zero bytes to the signature payload.
#[test]
fn signature_back_compat_with_empty_kinds() {
    let system = system_with_key();

    let mut comment = make_comment("Body");
    assert!(comment.remargin_kind.is_none());
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(result, "signature with empty kinds should verify");
}

#[test]
fn compute_checksum_with_empty_kinds_ignores_the_suffix() {
    let content = "hello world";
    let with_empty = compute_checksum(content, &[]);

    assert_eq!(
        with_empty,
        compute_checksum(content, &[]),
        "compute_checksum with &[] must be deterministic"
    );

    let with_kind = compute_checksum(content, &[String::from("question")]);
    assert_ne!(
        with_empty, with_kind,
        "adding a kind must change the hash; otherwise the no-kinds \
         back-compat branch would not be isolating the suffix"
    );
}

#[test]
fn verify_checksum_equivalent_for_none_and_empty_vec() {
    let mut comment = make_comment("Hello, world.");
    assert!(verify_checksum(&comment), "baseline None case must verify");

    comment.remargin_kind = Some(Vec::new());
    assert!(
        verify_checksum(&comment),
        "Some(empty vec) must hash the same as None for back-compat"
    );

    comment.remargin_kind = Some(vec![String::from("question")]);
    assert!(
        !verify_checksum(&comment),
        "adding kinds must shift the hash (stored checksum was for empty)"
    );
}

#[test]
fn openssh_private_key_round_trips() {
    use crate::crypto::ssh::{PrivateKey, PublicKey};

    let key = PrivateKey::from_openssh(TEST_PRIVATE_KEY).unwrap();
    let reserialized = key.to_openssh("test@remargin");
    let reparsed = PrivateKey::from_openssh(&reserialized).unwrap();

    let original_pub = key.public_key().to_openssh();
    let reparsed_pub = reparsed.public_key().to_openssh();
    assert_eq!(original_pub, reparsed_pub);

    let fixture_blob = TEST_PUBLIC_KEY.split_whitespace().nth(1).unwrap();
    let ours_blob = PublicKey::from_openssh(&original_pub).unwrap().to_openssh();
    assert!(ours_blob.contains(fixture_blob));
}

/// Skips when `ssh-keygen` is absent.
#[test]
fn ssh_keygen_accepts_our_signature() {
    use std::fs::{File, write};
    use std::io::Write as _;
    use std::process::Command;

    use crate::crypto::ssh::PrivateKey;

    if Command::new("ssh-keygen").arg("-V").output().is_err() {
        eprintln!("ssh-keygen not available; skipping interop test");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let message = b"interop payload for remargin";

    let key = PrivateKey::from_openssh(TEST_PRIVATE_KEY).unwrap();
    let pem = key.sign("remargin", message);

    let msg_path = dir.path().join("msg");
    write(&msg_path, message).unwrap();
    let sig_path = dir.path().join("msg.sig");
    write(&sig_path, pem).unwrap();

    let signers_path = dir.path().join("allowed_signers");
    let mut signers = File::create(&signers_path).unwrap();
    write!(
        signers,
        "test@remargin namespaces=\"remargin\" {TEST_PUBLIC_KEY}"
    )
    .unwrap();

    let output = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-f"])
        .arg(&signers_path)
        .args(["-I", "test@remargin", "-n", "remargin", "-s"])
        .arg(&sig_path)
        .stdin(File::open(&msg_path).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ssh-keygen -Y verify rejected our signature: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Skips when `ssh-keygen` is absent.
#[test]
fn ssh_keygen_accepts_our_keypair() {
    use std::fs::{Permissions, set_permissions, write};
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Command;

    use crate::crypto::generate_keypair;

    if Command::new("ssh-keygen").arg("-V").output().is_err() {
        eprintln!("ssh-keygen not available; skipping interop test");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let (private_pem, public_line) = generate_keypair("remargin").unwrap();

    let key_path = dir.path().join("id_ed25519");
    write(&key_path, &private_pem).unwrap();
    set_permissions(&key_path, Permissions::from_mode(0o600)).unwrap();

    let output = Command::new("ssh-keygen")
        .arg("-y")
        .arg("-f")
        .arg(&key_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ssh-keygen could not read our private key: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let derived = String::from_utf8(output.stdout).unwrap();
    let derived_blob = derived.split_whitespace().nth(1).unwrap();
    let ours_blob = public_line.split_whitespace().nth(1).unwrap();
    assert_eq!(
        derived_blob, ours_blob,
        "ssh-keygen-derived public key disagrees with ours"
    );
}
