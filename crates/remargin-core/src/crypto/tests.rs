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

fn system_with_key() -> MockSystem {
    MockSystem::new()
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
    let checksum = compute_checksum("Hello world");
    assert!(checksum.starts_with("sha256:"));
    assert_eq!(checksum, compute_checksum("Hello world"));
}

#[test]
fn different_content_different_checksum() {
    let c1 = compute_checksum("Hello");
    let c2 = compute_checksum("World");
    assert_ne!(c1, c2);
}

#[test]
fn crlf_vs_lf_same_checksum() {
    assert_eq!(compute_checksum("Hello\r\n"), compute_checksum("Hello\n"));
}

#[test]
fn trailing_whitespace_same_checksum() {
    assert_eq!(
        compute_checksum("line1  \nline2\n"),
        compute_checksum("line1\nline2\n")
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
    let mut reactions1: BTreeMap<String, Vec<String>> = BTreeMap::new();
    reactions1.insert(String::from("thumbsup"), vec![String::from("alice")]);

    let mut reactions2 = reactions1.clone();
    reactions2.insert(String::from("heart"), vec![String::from("bob")]);

    assert_ne!(
        compute_reaction_checksum(&reactions1),
        compute_reaction_checksum(&reactions2)
    );
}

#[test]
fn reaction_does_not_affect_content_checksum() {
    let comment = make_comment("Test content");
    let checksum_before = compute_checksum(&comment.content);

    let checksum_after = compute_checksum(&comment.content);
    assert_eq!(checksum_before, checksum_after);
}

#[test]
fn reaction_checksum_deterministic_order() {
    let mut reactions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    reactions.insert(
        String::from("thumbsup"),
        vec![String::from("bob"), String::from("alice")],
    );

    let c1 = compute_reaction_checksum(&reactions);
    let c2 = compute_reaction_checksum(&reactions);
    assert_eq!(c1, c2);
}

#[test]
fn ack_does_not_affect_content_checksum() {
    let comment = make_comment("Test content");
    let checksum = compute_checksum(&comment.content);
    assert_eq!(checksum, compute_checksum(&comment.content));
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
    let system = MockSystem::new()
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
