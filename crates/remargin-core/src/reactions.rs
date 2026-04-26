//! Reactions schema with per-author timestamps and legacy back-compat.
//!
//! Wire shape on disk (new):
//!
//! ```yaml
//! reactions:
//!   "+1":
//!     - author: eduardo
//!       ts: 2026-04-26T12:00:00-04:00
//!     - author: claude
//!       ts: 2026-04-26T12:01:00-04:00
//! ```
//!
//! Legacy shape (still parsed, never written):
//!
//! ```yaml
//! reactions:
//!   "+1": [eduardo, claude]
//! ```
//!
//! Legacy entries get a synthesized `ts` after the rest of the comment is
//! parsed via [`ReactionsExt::backfill_legacy_timestamps`]: if the comment
//! has an `ack:` entry from the same author, that ack's `ts` is used;
//! otherwise the comment's own `ts` is used. The synthesized value is
//! clamped to be no earlier than the comment's `ts` so a reaction can
//! never appear to have happened before the comment it's attached to.
//!
//! `Reactions` is a plain `BTreeMap<String, Vec<ReactionEntry>>` type
//! alias rather than a wrapper struct so tixschema-generated TypeScript
//! sees the wire shape directly (`Record<string, ReactionEntry[]>`) with
//! no extra wrapper types. The legacy bare-string tolerance lives in
//! [`deserialize_with_legacy`], plugged into [`crate::parser`]'s YAML
//! header struct via `#[serde(deserialize_with = ...)]`.

extern crate alloc;

use alloc::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Deserializer, Serialize};
use tixschema::model_schema;

use crate::parser::Acknowledgment;

/// One author's reaction with the time it was added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
#[model_schema]
pub struct ReactionEntry {
    pub author: String,
    pub ts: DateTime<FixedOffset>,
}

impl ReactionEntry {
    /// Construct a new entry with explicit author and timestamp.
    #[must_use]
    pub const fn new(author: String, ts: DateTime<FixedOffset>) -> Self {
        Self { author, ts }
    }
}

/// Reactions on one comment, keyed by emoji. Each emoji maps to an
/// ordered list of per-author entries with timestamps.
///
/// Stable iteration order (the keys come from `BTreeMap`) is what the
/// reaction checksum and the on-disk writer both rely on.
pub type Reactions = BTreeMap<String, Vec<ReactionEntry>>;

/// Wire shape of one item inside an emoji's list. Two forms are
/// accepted on read; only the new form is ever written. Module-private
/// because nothing outside this file should ever see the legacy form —
/// it is normalized to a [`ReactionEntry`] during deserialization.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum RawReactionItem {
    /// `"author"` — the legacy shape; `ts` is filled in later.
    Author(String),
    /// `{author: "x", ts: "..."}` — the new shape.
    Full {
        author: String,
        ts: DateTime<FixedOffset>,
    },
}

impl RawReactionItem {
    fn into_entry(self) -> ReactionEntry {
        match self {
            Self::Author(author) => ReactionEntry {
                author,
                ts: legacy_sentinel_ts(),
            },
            Self::Full { author, ts } => ReactionEntry { author, ts },
        }
    }
}

/// Extension methods on [`Reactions`].
///
/// `Reactions` is a type alias around `BTreeMap`, so its inherent
/// methods (`is_empty`, `len`, `get`, `new`, `default`) already cover
/// the trivial cases. This trait adds the reactions-specific operations
/// that need to know about emoji keys and per-author identity.
pub trait ReactionsExt {
    /// Add one author's reaction with the given timestamp. If the
    /// author already has an entry for this emoji, the call is a no-op
    /// (existing `ts` is preserved). Returns `true` when a new entry
    /// was inserted.
    fn add_reaction(&mut self, emoji: &str, author: &str, ts: DateTime<FixedOffset>) -> bool;

    /// Promote legacy entries (those whose timestamp is the
    /// [`legacy_sentinel_ts`] placeholder used during deserialization)
    /// by inferring `ts`: prefer a matching `ack` entry's `ts`, otherwise
    /// fall back to `comment_ts`.
    ///
    /// After inference, every `ts` is clamped to be no earlier than
    /// `comment_ts` so a reaction's timestamp can never predate the
    /// comment it is on.
    ///
    /// Entries that already carry a real (non-sentinel) timestamp are
    /// left untouched apart from the floor clamp.
    fn backfill_legacy_timestamps(
        &mut self,
        comment_ts: DateTime<FixedOffset>,
        acks: &[Acknowledgment],
    );

    /// Pairs of `(emoji, entries)` in stable key order. Returned as an
    /// owned `Vec` rather than an iterator so the writer / display
    /// helpers can borrow each pair without temporary-lifetime gymnastics.
    fn entries_by_emoji(&self) -> Vec<(String, Vec<ReactionEntry>)>;

    /// Remove one author's entry for an emoji. If the emoji's list
    /// becomes empty, the emoji key is removed too. Returns `true` when
    /// an entry was removed.
    fn remove_reaction(&mut self, emoji: &str, author: &str) -> bool;
}

impl ReactionsExt for Reactions {
    fn add_reaction(&mut self, emoji: &str, author: &str, ts: DateTime<FixedOffset>) -> bool {
        let entries = self.entry(String::from(emoji)).or_default();
        if entries.iter().any(|e| e.author == author) {
            return false;
        }
        entries.push(ReactionEntry {
            author: String::from(author),
            ts,
        });
        true
    }

    fn backfill_legacy_timestamps(
        &mut self,
        comment_ts: DateTime<FixedOffset>,
        acks: &[Acknowledgment],
    ) {
        let sentinel = legacy_sentinel_ts();
        for entries in self.values_mut() {
            for entry in entries.iter_mut() {
                let resolved = if entry.ts == sentinel {
                    acks.iter()
                        .find(|a| a.author == entry.author)
                        .map_or(comment_ts, |a| a.ts)
                } else {
                    entry.ts
                };
                entry.ts = if resolved < comment_ts {
                    comment_ts
                } else {
                    resolved
                };
            }
        }
    }

    fn entries_by_emoji(&self) -> Vec<(String, Vec<ReactionEntry>)> {
        self.iter()
            .map(|(emoji, entries)| (emoji.clone(), entries.clone()))
            .collect()
    }

    fn remove_reaction(&mut self, emoji: &str, author: &str) -> bool {
        let Some(entries) = self.get_mut(emoji) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|e| e.author != author);
        let removed = entries.len() != before;
        if entries.is_empty() {
            let _removed_emoji_list: Option<Vec<ReactionEntry>> = self.remove(emoji);
        }
        removed
    }
}

/// Legacy-tolerant deserializer for the `reactions:` field on a YAML
/// comment header. Plugged in via `#[serde(deserialize_with = ...)]`.
///
/// Each emoji's list may contain bare author strings (legacy shape) or
/// `{author, ts}` objects (new shape). Bare strings are promoted to a
/// [`ReactionEntry`] with the [`legacy_sentinel_ts`] placeholder; the
/// parser later calls [`ReactionsExt::backfill_legacy_timestamps`] to
/// substitute a real `ts` once the surrounding comment context is known.
///
/// # Errors
///
/// Returns the deserializer's error verbatim when the input is not a
/// map of `String -> [item, ...]` or when an item matches neither the
/// legacy bare-string shape nor the `{author, ts}` object shape.
pub fn deserialize_with_legacy<'de, D>(deserializer: D) -> Result<Reactions, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: BTreeMap<String, Vec<RawReactionItem>> = BTreeMap::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(emoji, items)| {
            (
                emoji,
                items.into_iter().map(RawReactionItem::into_entry).collect(),
            )
        })
        .collect())
}

/// Sentinel `ts` used to mark legacy entries during deserialization.
///
/// Real on-disk timestamps will never equal this exact value because
/// they come from `Utc::now().fixed_offset()` at write time and the
/// parser's backfill replaces any sentinel found.
///
/// Returns the UNIX epoch projected into a `FixedOffset` of zero
/// (`1970-01-01T00:00:00+00:00`). Built from chrono's `UNIX_EPOCH`
/// constant so the function is total — no panics, no error paths.
#[must_use]
pub fn legacy_sentinel_ts() -> DateTime<FixedOffset> {
    chrono::DateTime::UNIX_EPOCH.fixed_offset()
}

/// Serialize one [`ReactionEntry`] into a stable two-line YAML block
/// fragment used by the writer. Returns text that begins with `- `
/// and ends with a trailing newline.
#[must_use]
pub fn format_reaction_entry_block(indent: &str, entry: &ReactionEntry) -> String {
    let mut out = String::new();
    out.push_str(indent);
    out.push_str("- author: ");
    out.push_str(&entry.author);
    out.push('\n');
    out.push_str(indent);
    out.push_str("  ts: ");
    out.push_str(&entry.ts.to_rfc3339());
    out.push('\n');
    out
}

/// Quote an emoji key so the writer never produces malformed YAML for
/// keys that look like flow-syntax (`+1`, `:fire:`, etc.). Quoting
/// every key keeps the output deterministic without per-key analysis.
#[must_use]
pub fn quote_emoji_key(emoji: &str) -> String {
    let mut out = String::with_capacity(emoji.len() + 2);
    out.push('"');
    for ch in emoji.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
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
        let _added_eduardo =
            reactions.add_reaction("+1", "eduardo", ts("2026-04-26T12:00:00-04:00"));
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
        assert!(lines[2].is_empty());
    }
}
