//! Tests for the comment wire schema: field rendering, timestamps and the round trip.

use super::{OnDiskComment, comment_from_on_disk};
use crate::parser::{AcknowledgmentDraft, AuthorType, Comment, CommentDraft};
use crate::reactions::{Reactions, ReactionsExt as _};
use jiff::Timestamp;

fn sample_comment() -> Comment {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction(
        "+1",
        "bob",
        "2026-04-26T12:00:00Z".parse::<Timestamp>().unwrap(),
    );
    CommentDraft {
        ack: vec![
            AcknowledgmentDraft {
                author: String::from("jorge"),
                ts: "2026-04-06T15:00:00Z".parse::<Timestamp>().unwrap(),
            }
            .into(),
        ],
        attachments: vec![String::from("diagram.png")],
        author: String::from("eduardo"),
        author_type: AuthorType::Agent,
        checksum: String::from("sha256:deadbeef"),
        content: String::from("body"),
        edited_at: Some("2026-04-07T10:00:00Z".parse::<Timestamp>().unwrap()),
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
        ts: "2026-04-06T14:32:00Z".parse::<Timestamp>().unwrap(),
    }
    .into()
}

#[test]
fn from_comment_pins_author_type_to_lowercase_string() {
    let on_disk = OnDiskComment::from(&sample_comment());
    assert_eq!(on_disk.author_type, "agent");
}

#[test]
fn from_comment_formats_ack_as_author_at_ts_string() {
    let on_disk = OnDiskComment::from(&sample_comment());
    assert_eq!(on_disk.ack, vec!["jorge@2026-04-06T15:00:00Z"]);
}

/// A zero offset lands on disk as `Z`, matching what serde emits in a JSON payload.
#[test]
fn from_comment_renders_every_zero_offset_timestamp_with_z() {
    let mut comment = sample_comment().into_draft();
    comment.ts = "2026-04-06T14:32:00+00:00".parse::<Timestamp>().unwrap();
    comment.edited_at = Some("2026-04-07T10:00:00+00:00".parse::<Timestamp>().unwrap());
    comment.ack = vec![
        AcknowledgmentDraft {
            author: String::from("jorge"),
            ts: "2026-04-06T15:00:00+00:00".parse::<Timestamp>().unwrap(),
        }
        .into(),
    ];
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction(
        "+1",
        "bob",
        "2026-04-26T12:00:00+00:00".parse::<Timestamp>().unwrap(),
    );
    comment.reactions = reactions;

    let on_disk = OnDiskComment::from(&Comment::from(comment));
    assert_eq!(on_disk.ts, "2026-04-06T14:32:00Z");
    assert_eq!(on_disk.edited_at.as_deref(), Some("2026-04-07T10:00:00Z"));
    assert_eq!(on_disk.ack, vec!["jorge@2026-04-06T15:00:00Z"]);
    assert_eq!(on_disk.reactions["+1"][0].ts, "2026-04-26T12:00:00Z");
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
    let draft = original.into_draft();
    let restored = comment_from_on_disk(on_disk, draft.content, draft.line).unwrap();
    assert_eq!(restored.id, "full");
    assert_eq!(restored.author, "eduardo");
    assert_eq!(restored.author_type, AuthorType::Agent);
    assert_eq!(
        restored.ts,
        "2026-04-06T14:32:00Z".parse::<Timestamp>().unwrap()
    );
    assert_eq!(
        restored.edited_at,
        Some("2026-04-07T10:00:00Z".parse::<Timestamp>().unwrap())
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
    let mut comment = sample_comment().into_draft();
    comment.remargin_kind = None;
    let on_disk = OnDiskComment::from(&Comment::from(comment.clone()));
    assert_eq!(on_disk.remargin_kind, [] as [String; 0]);
    let restored = comment_from_on_disk(on_disk, comment.content, 0).unwrap();
    assert!(restored.remargin_kind.is_none());
}

#[test]
fn dedupes_acks_at_wire_boundary() {
    let mut comment = sample_comment().into_draft();
    comment.ack = vec![
        AcknowledgmentDraft {
            author: String::from("alice"),
            ts: "2026-04-27T05:01:00+00:00".parse::<Timestamp>().unwrap(),
        }
        .into(),
        AcknowledgmentDraft {
            author: String::from("alice"),
            ts: "2026-04-27T05:02:00+00:00".parse::<Timestamp>().unwrap(),
        }
        .into(),
    ];
    let on_disk = OnDiskComment::from(&Comment::from(comment));
    assert_eq!(on_disk.ack.len(), 1, "duplicate acks must collapse");
    assert!(
        on_disk.ack[0].contains("2026-04-27T05:02:00Z"),
        "survivor must carry latest ts: {}",
        on_disk.ack[0]
    );
}
