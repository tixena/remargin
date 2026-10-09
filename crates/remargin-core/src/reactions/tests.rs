//! Tests for reactions: both wire shapes, the timestamp backfill, add and remove, and
//! serialization.

use super::{
    Acknowledgment, ReactionEntry, Reactions, ReactionsExt, deserialize_with_legacy,
    format_reaction_entry_block, legacy_sentinel_ts, quote_emoji_key,
};
use chrono::{DateTime, FixedOffset};
use serde_yaml::Value;

fn ts(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

fn parse(yaml: &str) -> Reactions {
    let de = serde_yaml::Deserializer::from_str(yaml);
    deserialize_with_legacy(de).unwrap()
}

fn try_parse(yaml: &str) -> Result<Reactions, serde_yaml::Error> {
    let de = serde_yaml::Deserializer::from_str(yaml);
    deserialize_with_legacy(de)
}

fn entries_for(reactions: &Reactions, emoji: &str) -> Vec<ReactionEntry> {
    reactions.get(emoji).cloned().unwrap()
}

#[test]
fn deserialize_legacy_shape_uses_sentinel_ts() {
    let value = parse("+1: [eduardo, claude]\nheart: [alice]\n");
    assert_eq!(value.len(), 2);
    let plus_one = entries_for(&value, "+1");
    assert_eq!(plus_one.len(), 2);
    assert_eq!(plus_one[0].author, "eduardo");
    assert_eq!(plus_one[1].author, "claude");
    let sentinel = legacy_sentinel_ts();
    assert_eq!(plus_one[0].ts, sentinel);
    assert_eq!(plus_one[1].ts, sentinel);
}

#[test]
fn deserialize_new_shape_keeps_explicit_ts() {
    let yaml = "+1:\n  - author: eduardo\n    ts: 2026-04-26T12:00:00-04:00\n  - author: claude\n    ts: 2026-04-26T12:01:00-04:00\n";
    let value = parse(yaml);
    let entry_list = entries_for(&value, "+1");
    assert_eq!(entry_list.len(), 2);
    assert_eq!(entry_list[0].author, "eduardo");
    assert_eq!(entry_list[0].ts, ts("2026-04-26T12:00:00-04:00"));
    assert_eq!(entry_list[1].ts, ts("2026-04-26T12:01:00-04:00"));
}

#[test]
fn deserialize_mixed_per_emoji_shapes() {
    let yaml = "\
+1: [eduardo]
heart:
  - author: claude
    ts: 2026-04-26T12:00:00-04:00
";
    let value = parse(yaml);
    assert_eq!(value.len(), 2);
    assert_eq!(entries_for(&value, "+1")[0].ts, legacy_sentinel_ts());
    assert_eq!(
        entries_for(&value, "heart")[0].ts,
        ts("2026-04-26T12:00:00-04:00")
    );
}

#[test]
fn deserialize_rejects_unknown_field() {
    let yaml = "+1:\n  - author: eduardo\n    ts: 2026-04-26T12:00:00-04:00\n    foo: bar\n";
    let err = try_parse(yaml).unwrap_err();
    let _: String = err.to_string();
}

#[test]
fn deserialize_rejects_missing_ts() {
    let yaml = "+1:\n  - author: eduardo\n";
    let err = try_parse(yaml).unwrap_err();
    let _: String = err.to_string();
}

#[test]
fn backfill_uses_ack_ts_when_author_acked() {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction("+1", "eduardo", legacy_sentinel_ts());
    let comment_ts = ts("2026-04-26T10:00:00-04:00");
    let ack_ts = ts("2026-04-26T11:00:00-04:00");
    let acks = vec![Acknowledgment {
        author: String::from("eduardo"),
        ts: ack_ts,
    }];
    reactions.backfill_legacy_timestamps(comment_ts, &acks);
    assert_eq!(entries_for(&reactions, "+1")[0].ts, ack_ts);
}

#[test]
fn backfill_falls_back_to_comment_ts_without_ack() {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction("+1", "claude", legacy_sentinel_ts());
    let comment_ts = ts("2026-04-26T10:00:00-04:00");
    reactions.backfill_legacy_timestamps(comment_ts, &[]);
    assert_eq!(entries_for(&reactions, "+1")[0].ts, comment_ts);
}

#[test]
fn backfill_clamps_to_comment_ts_floor() {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction("+1", "eduardo", legacy_sentinel_ts());
    let comment_ts = ts("2026-04-26T10:00:00-04:00");
    let stale_ack = ts("2024-01-01T00:00:00+00:00");
    let acks = vec![Acknowledgment {
        author: String::from("eduardo"),
        ts: stale_ack,
    }];
    reactions.backfill_legacy_timestamps(comment_ts, &acks);
    assert_eq!(entries_for(&reactions, "+1")[0].ts, comment_ts);
}

#[test]
fn backfill_leaves_explicit_ts_alone() {
    let explicit_ts = ts("2026-04-26T12:00:00-04:00");
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction("+1", "eduardo", explicit_ts);
    let comment_ts = ts("2026-04-26T10:00:00-04:00");
    let acks = vec![Acknowledgment {
        author: String::from("eduardo"),
        ts: ts("2026-04-26T11:00:00-04:00"),
    }];
    reactions.backfill_legacy_timestamps(comment_ts, &acks);
    assert_eq!(entries_for(&reactions, "+1")[0].ts, explicit_ts);
}

#[test]
fn add_is_idempotent_for_same_author() {
    let mut reactions = Reactions::new();
    let first = reactions.add_reaction("+1", "eduardo", ts("2026-04-26T12:00:00-04:00"));
    let second = reactions.add_reaction("+1", "eduardo", ts("2026-04-26T13:00:00-04:00"));
    assert!(first);
    assert!(!second);
    let entry_list = entries_for(&reactions, "+1");
    assert_eq!(entry_list.len(), 1);
    assert_eq!(entry_list[0].ts, ts("2026-04-26T12:00:00-04:00"));
}

#[test]
fn remove_drops_emoji_when_empty() {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction("+1", "eduardo", ts("2026-04-26T12:00:00-04:00"));
    let removed = ReactionsExt::remove_reaction(&mut reactions, "+1", "eduardo");
    assert!(removed);
    assert!(reactions.is_empty());
}

#[test]
fn remove_keeps_other_authors() {
    let mut reactions = Reactions::new();
    let _added_eduardo = reactions.add_reaction("+1", "eduardo", ts("2026-04-26T12:00:00-04:00"));
    let _added_claude = reactions.add_reaction("+1", "claude", ts("2026-04-26T12:01:00-04:00"));
    let removed = ReactionsExt::remove_reaction(&mut reactions, "+1", "eduardo");
    assert!(removed);
    let entry_list = entries_for(&reactions, "+1");
    assert_eq!(entry_list.len(), 1);
    assert_eq!(entry_list[0].author, "claude");
}

#[test]
fn serialize_emits_new_shape() {
    let mut reactions = Reactions::new();
    let _added = reactions.add_reaction("+1", "eduardo", ts("2026-04-26T12:00:00-04:00"));
    let yaml = serde_yaml::to_string(&reactions).unwrap();
    let value: Value = serde_yaml::from_str(&yaml).unwrap();
    let plus_one = value
        .as_mapping()
        .unwrap()
        .get(Value::String(String::from("+1")))
        .unwrap();
    let entry_list = plus_one.as_sequence().unwrap();
    let first = entry_list[0].as_mapping().unwrap();
    assert_eq!(
        first.get(Value::String(String::from("author"))),
        Some(&Value::String(String::from("eduardo")))
    );
    assert!(
        first.contains_key(Value::String(String::from("ts"))),
        "serialized entry must carry an explicit `ts` field"
    );
}

#[test]
fn quote_emoji_key_handles_special_chars() {
    assert_eq!(quote_emoji_key("+1"), "\"+1\"");
    assert_eq!(quote_emoji_key("\u{1f44d}"), "\"\u{1f44d}\"");
    assert_eq!(quote_emoji_key("a\"b"), "\"a\\\"b\"");
}

#[test]
fn format_reaction_entry_block_has_two_lines_and_trailing_newline() {
    let entry = ReactionEntry::new(String::from("eduardo"), ts("2026-04-26T12:00:00-04:00"));
    let block = format_reaction_entry_block("    ", &entry);
    let lines: Vec<&str> = block.split('\n').collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "    - author: eduardo");
    assert_eq!(lines[1], "      ts: 2026-04-26T12:00:00-04:00");
    assert_eq!(lines[2], "");
}
