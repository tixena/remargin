//! Wire schema for a remargin comment block.

// Field declaration order mirrors the writer's canonical on-disk emit
// order — not the lint's alphabetical default — because serde emits
// struct fields in declaration order and the YAML bytes hitting disk
// are observed.
#![expect(
    clippy::arbitrary_source_item_ordering,
    reason = "fields ordered for serde emission, not the lint's alphabetical default"
)]

extern crate alloc;

use alloc::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use chrono::DateTime;
use serde::{Deserialize, Deserializer, Serialize};
use tixschema::model_schema;

use crate::parser::{Acknowledgment, AuthorType, Comment};
use crate::reactions::{
    ReactionEntry, Reactions, ReactionsExt as _, deserialize_with_legacy, legacy_sentinel_ts,
};
use crate::writer::dedupe_acks;

/// One reaction entry on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
#[model_schema]
pub struct ReactionEntryOnDisk {
    pub author: String,
    pub ts: String,
}

/// Wire-only mirror of [`Comment`] used as the single source of truth
/// for on-disk YAML key names.
///
/// Every key the writer emits and the parser reads is pinned by a
/// `#[serde(rename = "...")]` here. Adding a field forces a compile
/// error on both `From` impls below, preventing writer/parser drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
#[model_schema]
pub struct OnDiskComment {
    pub id: String,
    pub author: String,
    #[serde(rename = "type")]
    pub author_type: String,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, rename = "reply-to", skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remargin_kind: Vec<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_reactions_with_legacy",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub reactions: BTreeMap<String, Vec<ReactionEntryOnDisk>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ack: Vec<String>,
    pub checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

fn deserialize_reactions_with_legacy<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, Vec<ReactionEntryOnDisk>>, D::Error>
where
    D: Deserializer<'de>,
{
    let reactions: Reactions = deserialize_with_legacy(deserializer)?;
    Ok(reactions
        .into_iter()
        .map(|(emoji, entries)| {
            let on_disk: Vec<ReactionEntryOnDisk> = entries
                .into_iter()
                .map(|entry: ReactionEntry| ReactionEntryOnDisk {
                    author: entry.author,
                    ts: entry.ts.to_rfc3339(),
                })
                .collect();
            (emoji, on_disk)
        })
        .collect())
}

impl From<&Comment> for OnDiskComment {
    // Explicit `_` patterns for `content` and `line` are intentional:
    // the destructure is the compile-time check that every Comment
    // field has been consciously routed (or skipped). `..` would let a
    // future field slip through silently.
    #[expect(
        clippy::unneeded_field_pattern,
        reason = "exhaustiveness check guards against silently dropped fields"
    )]
    fn from(comment: &Comment) -> Self {
        let Comment {
            ack,
            attachments,
            author,
            author_type,
            checksum,
            content: _,
            edited_at,
            id,
            line: _,
            reactions,
            remargin_kind,
            reply_to,
            signature,
            thread,
            to,
            ts,
        } = comment;

        let deduped_acks = dedupe_acks(ack);
        let ack_strings: Vec<String> = deduped_acks
            .into_iter()
            .map(|entry| format!("{}@{}", entry.author, entry.ts.to_rfc3339()))
            .collect();

        let reactions_on_disk: BTreeMap<String, Vec<ReactionEntryOnDisk>> = reactions
            .entries_by_emoji()
            .into_iter()
            .map(|(emoji, entries)| {
                let on_disk = entries
                    .into_iter()
                    .map(|entry| ReactionEntryOnDisk {
                        author: entry.author,
                        ts: entry.ts.to_rfc3339(),
                    })
                    .collect();
                (emoji, on_disk)
            })
            .collect();

        Self {
            id: id.clone(),
            author: author.clone(),
            author_type: String::from(author_type.as_str()),
            ts: ts.to_rfc3339(),
            edited_at: edited_at.map(|t| t.to_rfc3339()),
            to: to.clone(),
            reply_to: reply_to.clone(),
            thread: thread.clone(),
            attachments: attachments.clone(),
            remargin_kind: remargin_kind.clone().unwrap_or_default(),
            reactions: reactions_on_disk,
            ack: ack_strings,
            checksum: checksum.clone(),
            signature: signature.clone(),
        }
    }
}

/// Convert an [`OnDiskComment`] back into a rich [`Comment`]. The
/// caller supplies `line` and `content` because neither lives in the
/// wire schema.
///
/// # Errors
///
/// Returns an error when:
/// - `ts` or `edited_at` is not a valid RFC 3339 timestamp
/// - `author_type` is not `"human"` or `"agent"`
/// - any `ack` entry is missing the `@` separator or carries a
///   non-RFC-3339 timestamp
/// - any reaction entry's `ts` is not RFC 3339
pub fn comment_from_on_disk(
    on_disk: OnDiskComment,
    content: String,
    line: usize,
) -> Result<Comment> {
    let OnDiskComment {
        id,
        author,
        author_type: author_type_raw,
        ts: ts_raw,
        edited_at: edited_at_raw,
        to,
        reply_to,
        thread,
        attachments,
        remargin_kind,
        reactions: reactions_raw,
        ack: ack_raw,
        checksum,
        signature,
    } = on_disk;

    let ts = DateTime::parse_from_rfc3339(&ts_raw)
        .map_err(|e| anyhow!("invalid timestamp {ts_raw:?}: {e}"))?;

    let edited_at = match edited_at_raw {
        Some(raw) => Some(
            DateTime::parse_from_rfc3339(&raw)
                .map_err(|e| anyhow!("invalid edited_at {raw:?}: {e}"))?,
        ),
        None => None,
    };

    let author_type = match author_type_raw.as_str() {
        "agent" => AuthorType::Agent,
        "human" => AuthorType::Human,
        other => bail!("unknown author type: {other}"),
    };

    let mut ack_list = Vec::with_capacity(ack_raw.len());
    for entry in &ack_raw {
        ack_list.push(parse_ack_string(entry)?);
    }

    let mut rich_reactions = Reactions::new();
    for (emoji, entries) in reactions_raw {
        let mut converted = Vec::with_capacity(entries.len());
        for entry in entries {
            // Empty ts (legacy) maps to the sentinel; the surrounding
            // backfill replaces it against the comment ts and ack list.
            let entry_ts = if entry.ts.is_empty() {
                legacy_sentinel_ts()
            } else {
                DateTime::parse_from_rfc3339(&entry.ts)
                    .map_err(|e| anyhow!("invalid reaction ts {:?}: {e}", entry.ts))?
            };
            converted.push(ReactionEntry {
                author: entry.author,
                ts: entry_ts,
            });
        }
        let _previous = rich_reactions.insert(emoji, converted);
    }
    rich_reactions.backfill_legacy_timestamps(ts, &ack_list);

    let kinds = if remargin_kind.is_empty() {
        None
    } else {
        Some(remargin_kind)
    };

    Ok(Comment {
        ack: ack_list,
        attachments,
        author,
        author_type,
        checksum,
        content,
        edited_at,
        id,
        line,
        reactions: rich_reactions,
        remargin_kind: kinds,
        reply_to,
        signature,
        thread,
        to,
        ts,
    })
}

fn parse_ack_string(entry: &str) -> Result<Acknowledgment> {
    let at_pos = entry
        .find('@')
        .ok_or_else(|| anyhow!("ack entry missing '@': {entry}"))?;
    let author = entry[..at_pos].to_owned();
    let ts_str = &entry[at_pos + 1..];
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .map_err(|e| anyhow!("invalid ack timestamp {ts_str:?}: {e}"))?;
    Ok(Acknowledgment { author, ts })
}

#[cfg(test)]
mod tests {
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
            id: String::from("full"),
            line: 0,
            reactions,
            remargin_kind: Some(vec![String::from("question")]),
            reply_to: Some(String::from("xyz")),
            signature: Some(String::from("ed25519:sig==")),
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
}
