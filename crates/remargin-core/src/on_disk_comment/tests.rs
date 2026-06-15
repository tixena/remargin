use super::{OnDiskComment, comment_from_on_disk};
use crate::parser::{Acknowledgment, AuthorType, Comment};
use crate::reactions::{Reactions, ReactionsExt as _};
use chrono::DateTime;

fn sample_comment() -> Comment {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction(
        "+1",
        "bob",
        DateTime::parse_from_rfc3339("2026-04-26T12:00:00-04:00").unwrap(),
    );
    Comment {
        ack: vec![Acknowledgment {
            author: String::from("jorge"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T15:00:00-04:00").unwrap(),
        }],
        attachments: vec![String::from("diagram.png")],
        author: String::from("eduardo"),
        author_type: AuthorType::Agent,
        checksum: String::from("sha256:deadbeef"),
        content: String::from("body"),
        edited_at: Some(DateTime::parse_from_rfc3339("2026-04-07T10:00:00-04:00").unwrap()),
        el: None,
        id: String::from("full"),
        line: 0,
        reactions,
        remargin_kind: Some(vec![String::from("question")]),
        reply_to: Some(String::from("xyz")),
        signature: Some(String::from("ed25519:sig==")),
        sl: None,
        thread: Some(String::from("t01")),
        to: vec![String::from("jorge"), String::from("claude")],
        ts: DateTime::parse_from_rfc3339("2026-04-06T14:32:00-04:00").unwrap(),
    }
}

#[test]
fn from_comment_pins_author_type_to_lowercase_string() {
    let on_disk = OnDiskComment::from(&sample_comment());
    assert_eq!(on_disk.author_type, "agent");
}

#[test]
fn from_comment_formats_ack_as_author_at_ts_string() {
    let on_disk = OnDiskComment::from(&sample_comment());
    assert_eq!(on_disk.ack, vec!["jorge@2026-04-06T15:00:00-04:00"]);
}

#[test]
fn from_comment_drops_in_memory_only_fields() {
    let on_disk = OnDiskComment::from(&sample_comment());
    let round_trip = comment_from_on_disk(on_disk, String::from("body"), 0).unwrap();
    assert_eq!(round_trip.content, "body");
    assert_eq!(round_trip.line, 0);
}

#[test]
fn round_trip_preserves_all_wire_fields() {
    let original = sample_comment();
    let on_disk = OnDiskComment::from(&original);
    let restored = comment_from_on_disk(on_disk, original.content, original.line).unwrap();
    assert_eq!(restored.id, "full");
    assert_eq!(restored.author, "eduardo");
    assert_eq!(restored.author_type, AuthorType::Agent);
    assert_eq!(
        restored.ts,
        DateTime::parse_from_rfc3339("2026-04-06T14:32:00-04:00").unwrap()
    );
    assert_eq!(
        restored.edited_at,
        Some(DateTime::parse_from_rfc3339("2026-04-07T10:00:00-04:00").unwrap())
    );
    assert_eq!(restored.to, vec!["jorge", "claude"]);
    assert_eq!(restored.reply_to.as_deref(), Some("xyz"));
    assert_eq!(restored.thread.as_deref(), Some("t01"));
    assert_eq!(restored.attachments, vec!["diagram.png"]);
    assert_eq!(
        restored.remargin_kind.as_deref(),
        Some(&[String::from("question")][..])
    );
    assert_eq!(restored.ack.len(), 1);
    assert_eq!(restored.ack[0].author, "jorge");
    assert_eq!(restored.checksum, "sha256:deadbeef");
    assert_eq!(restored.signature.as_deref(), Some("ed25519:sig=="));
    assert_eq!(restored.reactions.len(), 1);
}

#[test]
fn empty_remargin_kind_round_trips_to_none() {
    let mut comment = sample_comment();
    comment.remargin_kind = None;
    let on_disk = OnDiskComment::from(&comment);
    assert!(on_disk.remargin_kind.is_empty());
    let restored = comment_from_on_disk(on_disk, comment.content, 0).unwrap();
    assert!(restored.remargin_kind.is_none());
}

#[test]
fn dedupes_acks_at_wire_boundary() {
    let mut comment = sample_comment();
    comment.ack = vec![
        Acknowledgment {
            author: String::from("alice"),
            ts: DateTime::parse_from_rfc3339("2026-04-27T05:01:00+00:00").unwrap(),
        },
        Acknowledgment {
            author: String::from("alice"),
            ts: DateTime::parse_from_rfc3339("2026-04-27T05:02:00+00:00").unwrap(),
        },
    ];
    let on_disk = OnDiskComment::from(&comment);
    assert_eq!(on_disk.ack.len(), 1, "duplicate acks must collapse");
    assert!(
        on_disk.ack[0].contains("2026-04-27T05:02:00+00:00"),
        "survivor must carry latest ts: {}",
        on_disk.ack[0]
    );
}
