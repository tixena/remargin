//! Wire schema for a remargin comment block.

extern crate alloc;

use alloc::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use chrono::DateTime;
use serde::ser::SerializeStruct as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
///
/// **Wire-order vs source-order:** the on-disk YAML emit order is
/// fixed by the manual [`Serialize`] impl below — NOT by struct field
/// declaration order. Source fields stay alphabetical (clippy's
/// `arbitrary_source_item_ordering` requirement); the canonical YAML
/// byte sequence stays as it has always been (`id`, `author`, `type`,
/// `ts`, `edited_at`, `to`, `reply-to`, `thread`, `attachments`,
/// `remargin_kind`, `reactions`, `ack`, `checksum`, `signature`).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
#[model_schema]
pub struct OnDiskComment {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ack: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    pub author: String,
    #[serde(rename = "type")]
    pub author_type: String,
    pub checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    pub id: String,
    #[serde(
        default,
        deserialize_with = "deserialize_reactions_with_legacy",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub reactions: BTreeMap<String, Vec<ReactionEntryOnDisk>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remargin_kind: Vec<String>,
    #[serde(default, rename = "reply-to", skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    pub ts: String,
}

/// Manual [`Serialize`] impl pinning the canonical on-disk YAML emit
/// order. Source-field order is alphabetical (per
/// `arbitrary_source_item_ordering`); this impl emits in the original
/// wire order so existing markdown documents stay byte-identical when
/// rewritten.
impl Serialize for OnDiskComment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Field count includes the always-emitted ones (5) plus any
        // conditional fields whose `skip_serializing_if` predicate
        // returns false for this instance.
        let mut count = 5_usize; // id, author, type, ts, checksum
        if self.edited_at.is_some() {
            count += 1;
        }
        if !self.to.is_empty() {
            count += 1;
        }
        if self.reply_to.is_some() {
            count += 1;
        }
        if self.thread.is_some() {
            count += 1;
        }
        if !self.attachments.is_empty() {
            count += 1;
        }
        if !self.remargin_kind.is_empty() {
            count += 1;
        }
        if !self.reactions.is_empty() {
            count += 1;
        }
        if !self.ack.is_empty() {
            count += 1;
        }
        if self.signature.is_some() {
            count += 1;
        }

        let mut state = serializer.serialize_struct("OnDiskComment", count)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("author", &self.author)?;
        state.serialize_field("type", &self.author_type)?;
        state.serialize_field("ts", &self.ts)?;
        if let Some(edited_at) = self.edited_at.as_ref() {
            state.serialize_field("edited_at", edited_at)?;
        }
        if !self.to.is_empty() {
            state.serialize_field("to", &self.to)?;
        }
        if let Some(reply_to) = self.reply_to.as_ref() {
            state.serialize_field("reply-to", reply_to)?;
        }
        if let Some(thread) = self.thread.as_ref() {
            state.serialize_field("thread", thread)?;
        }
        if !self.attachments.is_empty() {
            state.serialize_field("attachments", &self.attachments)?;
        }
        if !self.remargin_kind.is_empty() {
            state.serialize_field("remargin_kind", &self.remargin_kind)?;
        }
        if !self.reactions.is_empty() {
            state.serialize_field("reactions", &self.reactions)?;
        }
        if !self.ack.is_empty() {
            state.serialize_field("ack", &self.ack)?;
        }
        state.serialize_field("checksum", &self.checksum)?;
        if let Some(signature) = self.signature.as_ref() {
            state.serialize_field("signature", signature)?;
        }
        state.end()
    }
}

impl From<&Comment> for OnDiskComment {
    // Bound-but-unused `_content` / `_line` patterns are intentional:
    // the destructure is the compile-time check that every Comment
    // field has been consciously routed (or skipped). `..` would let a
    // future field slip through silently. Underscore-prefixed names
    // satisfy `clippy::unneeded_field_pattern` (they are bindings, not
    // wildcards) while still suppressing unused-variable warnings.
    fn from(comment: &Comment) -> Self {
        let Comment {
            ack,
            attachments,
            author,
            author_type,
            checksum,
            content: _content,
            edited_at,
            el: _el,
            id,
            line: _line,
            reactions,
            remargin_kind,
            reply_to,
            signature,
            sl: _sl,
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
        el: None,
        id,
        line,
        reactions: rich_reactions,
        remargin_kind: kinds,
        reply_to,
        signature,
        sl: None,
        thread,
        to,
        ts,
    })
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
mod tests;
