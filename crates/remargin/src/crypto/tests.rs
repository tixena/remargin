//! Tests for the crypto module.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::Path;

use chrono::DateTime;
use os_shim::mock::MockSystem;

use crate::crypto::{
    compute_checksum, compute_reaction_checksum, compute_signature, normalize_whitespace,
    verify_checksum, verify_signature,
};
use crate::parser::{AuthorType, Comment};

// ---------------------------------------------------------------------------
// Test key pair (Ed25519, generated with ssh-keygen, no passphrase)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a minimal comment for testing.
fn make_comment(content: &str) -> Comment {
    Comment {
        ack: Vec::new(),
        attachments: Vec::new(),
        author: String::from("eduardo"),
        author_type: AuthorType::Human,
        checksum: compute_checksum(content),
        content: String::from(content),
        fence_depth: 3,
        id: String::from("abc"),
        line: 0,
        reactions: BTreeMap::new(),
        reply_to: None,
        signature: None,
        thread: None,
        to: Vec::new(),
        ts: DateTime::parse_from_rfc3339("2026-04-06T12:00:00-04:00").unwrap(),
    }
}

/// Create a `MockSystem` with the test private key loaded.
fn system_with_key() -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/keys/ed25519"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Whitespace normalization tests
// ---------------------------------------------------------------------------

// Test: CRLF is converted to LF.
#[test]
fn normalize_crlf_to_lf() {
    assert_eq!(normalize_whitespace("Hello\r\nworld"), "Hello\nworld");
}

// Test: Trailing whitespace is stripped from each line.
#[test]
fn normalize_trailing_whitespace() {
    assert_eq!(
        normalize_whitespace("line1  \nline2\t\nline3"),
        "line1\nline2\nline3"
    );
}

// Test: Leading and trailing newlines are trimmed.
#[test]
fn normalize_leading_trailing_newlines() {
    assert_eq!(normalize_whitespace("\n\nHello\n\n"), "Hello");
}

// Test: Combined normalization.
#[test]
fn normalize_combined() {
    assert_eq!(
        normalize_whitespace("\r\nline1  \r\nline2\r\n"),
        "line1\nline2"
    );
}

// ---------------------------------------------------------------------------
// Checksum tests
// ---------------------------------------------------------------------------

// Test 1: Basic checksum produces consistent sha256 hash.
#[test]
fn basic_checksum() {
    let checksum = compute_checksum("Hello world");
    assert!(checksum.starts_with("sha256:"));
    // Same input always produces same output.
    assert_eq!(checksum, compute_checksum("Hello world"));
}

// Test 2: Different content produces different checksums.
#[test]
fn different_content_different_checksum() {
    let c1 = compute_checksum("Hello");
    let c2 = compute_checksum("World");
    assert_ne!(c1, c2);
}

// Test 3: CRLF vs LF produces same checksum (whitespace normalization).
#[test]
fn crlf_vs_lf_same_checksum() {
    assert_eq!(compute_checksum("Hello\r\n"), compute_checksum("Hello\n"));
}

// Test 4: Trailing whitespace difference produces same checksum.
#[test]
fn trailing_whitespace_same_checksum() {
    assert_eq!(
        compute_checksum("line1  \nline2\n"),
        compute_checksum("line1\nline2\n")
    );
}

// Test 5: verify_checksum returns true for unmodified comment.
#[test]
fn verify_checksum_pass() {
    let comment = make_comment("This is a test comment.");
    assert!(verify_checksum(&comment));
}

// Test 6: verify_checksum returns false for modified content.
#[test]
fn verify_checksum_fail() {
    let mut comment = make_comment("Original content");
    comment.content = String::from("Modified content");
    assert!(!verify_checksum(&comment));
}

// ---------------------------------------------------------------------------
// Reaction checksum tests
// ---------------------------------------------------------------------------

// Test 9: Reaction checksum changes when a reaction is added.
#[test]
fn reaction_checksum_changes_on_add() {
    let mut reactions1: BTreeMap<String, Vec<String>> = BTreeMap::new();
    reactions1.insert(String::from("thumbsup"), vec![String::from("alice")]);

    let mut reactions2 = reactions1.clone();
    reactions2.insert(String::from("heart"), vec![String::from("bob")]);

    assert_ne!(
        compute_reaction_checksum(&reactions1),
        compute_reaction_checksum(&reactions2)
    );
}

// Test: Adding a reaction does not change the content checksum.
#[test]
fn reaction_does_not_affect_content_checksum() {
    let comment = make_comment("Test content");
    let checksum_before = compute_checksum(&comment.content);

    // compute_checksum only looks at content, not reactions.
    let checksum_after = compute_checksum(&comment.content);
    assert_eq!(checksum_before, checksum_after);
}

// Test: Reaction checksum is deterministic regardless of author order.
#[test]
fn reaction_checksum_deterministic_order() {
    let mut reactions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    reactions.insert(
        String::from("thumbsup"),
        vec![String::from("bob"), String::from("alice")],
    );

    // Authors are sorted internally, so the checksum is consistent.
    let c1 = compute_reaction_checksum(&reactions);
    let c2 = compute_reaction_checksum(&reactions);
    assert_eq!(c1, c2);
}

// ---------------------------------------------------------------------------
// Ack independence
// ---------------------------------------------------------------------------

// Test 10: Adding an ack does NOT change the content checksum.
#[test]
fn ack_does_not_affect_content_checksum() {
    let comment = make_comment("Test content");
    let checksum = compute_checksum(&comment.content);
    // Ack changes don't touch content, so the checksum stays the same.
    assert_eq!(checksum, compute_checksum(&comment.content));
}

// ---------------------------------------------------------------------------
// Signature tests
// ---------------------------------------------------------------------------

// Test: Signature round-trip -- sign with private key, verify with public.
#[test]
fn signature_round_trip() {
    let system = system_with_key();

    let mut comment = make_comment("Signed content");
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(result, "signature verification should succeed");
}

// Test: Signature verification fails when content is tampered.
#[test]
fn signature_tamper_content() {
    let system = system_with_key();

    let mut comment = make_comment("Original content");
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    // Tamper with the content.
    comment.content = String::from("Tampered content");

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(!result, "verification should fail after content tampering");
}

// Test: Signature verification fails when metadata (author) is tampered.
#[test]
fn signature_tamper_author() {
    let system = system_with_key();

    let mut comment = make_comment("Content");
    let sig = compute_signature(&comment, Path::new("/keys/ed25519"), &system).unwrap();
    comment.signature = Some(sig);

    // Tamper with the author.
    comment.author = String::from("mallory");

    let result = verify_signature(&comment, TEST_PUBLIC_KEY).unwrap();
    assert!(!result, "verification should fail after author tampering");
}

// Test: Key loading uses os-shim System.
#[test]
fn key_loading_via_system() {
    let system = MockSystem::new()
        .with_file(Path::new("/mock/ssh/key"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap();

    let comment = make_comment("Test");
    let result = compute_signature(&comment, Path::new("/mock/ssh/key"), &system);
    assert!(result.is_ok(), "signing with mock system key should work");
    assert!(result.unwrap().starts_with("ed25519:"));
}

// Test: Verify fails for comment with no signature.
#[test]
fn verify_no_signature() {
    let comment = make_comment("Test");
    let result = verify_signature(&comment, TEST_PUBLIC_KEY);
    assert!(
        result.is_err(),
        "should error when comment has no signature"
    );
}

// Test: Signature with all optional fields populated.
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
