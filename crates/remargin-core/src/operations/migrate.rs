//! Old-format migration.
//!
//! Convert legacy inline comments (`user comments` / `agent comments`) to the
//! Remargin format with proper IDs, checksums, and metadata.
//!
//! Two extras shipped together (rem-mxxx):
//!
//! 1. **Per-role identities.** Callers may supply a `MigrateIdentities`
//!    carrying a fully-resolved identity (author + optional signing key)
//!    for each legacy role. When supplied, migrated comments are
//!    attributed to that identity and signed with that key, so the
//!    document survives the strict-mode `commit_with_verify` gate. When
//!    omitted, the op falls back to the historical `legacy-user` /
//!    `legacy-agent` placeholder with no signature — the open-mode
//!    behaviour callers already depended on stays byte-identical.
//!
//! 2. **Automatic threading.** Within a section, alternating legacy
//!    comments are treated as a conversation: the second comment
//!    replies to the first, the third replies to the second, and so on.
//!    The chain breaks on:
//!    - any ATX heading at any level in body between two consecutive
//!      legacy comments
//!    - any non-whitespace prose in body between them (whitespace-only
//!      body is *not* a break — adjacent fences with blank lines stay
//!      linked)
//!    - an already-Remargin `Segment::Comment` between them (foreign
//!      conversation; do not splice through it)
//!    - same role consecutively (`U → U` / `A → A`) — the second comment
//!      starts a new root.
//!
//!    Each link also pushes an implicit `Acknowledgment` from the
//!    replier onto the parent's `ack` list, since under the legacy
//!    convention a reply *was* the acknowledgment.

#[cfg(test)]
mod tests;

extern crate alloc;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveTime, TimeZone as _, Utc};
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::crypto::{compute_checksum, compute_signature};
use crate::frontmatter;
use crate::id;
use crate::operations::verify::commit_with_verify;
use crate::parser::{self, Acknowledgment, AuthorType, Comment, LegacyRole, Segment};
use crate::permissions::op_guard::pre_mutate_check;
use crate::reactions::Reactions;
use crate::writer;

/// Per-role identity used to attribute and sign migrated comments.
///
/// Carries only what the op needs: an author name and an optional
/// signing key path. The author type is fixed by the legacy role, so we
/// don't carry it. Resolved by the CLI / MCP adapter via branch 1 of
/// `config::identity::resolve_identity` and handed in.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MigrateRoleIdentity {
    /// Author name written into the migrated comment's `author:` field.
    pub identity: String,
    /// Optional signing key. When `Some`, the migrated comment is signed
    /// before the verify gate runs.
    pub key_path: Option<PathBuf>,
}

impl MigrateRoleIdentity {
    /// Build a role identity. The struct is `#[non_exhaustive]` so
    /// out-of-crate adapters need this constructor instead of the
    /// literal expression.
    #[must_use]
    pub const fn new(identity: String, key_path: Option<PathBuf>) -> Self {
        Self { identity, key_path }
    }
}

/// The two role identities migrate may use, one per legacy role.
///
/// `Default::default()` is the historical `legacy-user` / `legacy-agent`
/// fallback path: every field `None`, no signing.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MigrateIdentities {
    /// Identity for `agent comments` blocks. `None` falls back to
    /// hardcoded `legacy-agent`.
    pub agent: Option<MigrateRoleIdentity>,
    /// Identity for `user comments` blocks. `None` falls back to
    /// hardcoded `legacy-user`.
    pub human: Option<MigrateRoleIdentity>,
}

impl MigrateIdentities {
    /// Build a `MigrateIdentities` from independent per-role slots.
    /// `#[non_exhaustive]` blocks the literal-expression form for
    /// out-of-crate callers.
    #[must_use]
    pub const fn new(
        human: Option<MigrateRoleIdentity>,
        agent: Option<MigrateRoleIdentity>,
    ) -> Self {
        Self { agent, human }
    }
}

/// Record of a migrated comment.
#[derive(Debug)]
#[non_exhaustive]
pub struct MigratedComment {
    /// The new Remargin ID assigned.
    pub new_id: String,
    /// The original role (user or agent).
    pub original_role: String,
}

/// In-progress chain link. Captured when we emit a comment so the next
/// legacy comment can decide whether to link to it.
struct ChainLink {
    /// Comment id of the chain head we'd reply to.
    parent_id: String,
    /// Index of the parent in the `new_segments` vec, so we can append
    /// the implicit ack to the parent's `ack` list when the reply
    /// links.
    parent_idx: usize,
    /// The parent's role — alternation requires the next role differs.
    parent_role: LegacyRole,
    /// Stable thread root id, propagated to every reply in the chain.
    thread_root: String,
}

/// Migrate all legacy comments in a document to Remargin format.
///
/// If `backup` is true, writes a `.bak` copy before modifying.
///
/// `identities` controls how each legacy comment is attributed and
/// (optionally) signed. Pass `&MigrateIdentities::default()` to keep
/// the historical `legacy-user` / `legacy-agent` fallback.
///
/// Callers who want to preview the outcome without writing should use
/// `remargin plan migrate` (rem-0ry dropped the per-op `--dry-run` flag
/// in favour of the uniform plan projection).
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read or written
/// - The document cannot be parsed
/// - A signing key supplied via `identities` cannot be read or parsed
/// - The post-write document fails the verify gate (e.g. strict mode
///   without supplied identities — the original "strict mode is dead"
///   bug surfaces here as an error rather than silently writing a
///   broken doc)
pub fn migrate(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    identities: &MigrateIdentities,
    backup: bool,
) -> Result<Vec<MigratedComment>> {
    writer::ensure_not_forbidden_target(path)?;
    pre_mutate_check(system, "migrate", path)?;
    let mut doc = parser::parse_file(system, path)?;

    let legacy_count = doc.legacy_comments().len();
    if legacy_count == 0 {
        return Ok(Vec::new());
    }

    if backup {
        let backup_path = path.with_extension("md.bak");
        let content = system
            .read_to_string(path)
            .context("reading file for backup")?;
        system
            .write(&backup_path, content.as_bytes())
            .context("writing backup")?;
    }

    let now = Utc::now().fixed_offset();
    let (new_segments, results) = build_migrated_segments(system, &doc.segments, identities, now)?;
    doc.segments = new_segments;

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    let added_ids: HashSet<String> = results.iter().map(|r| r.new_id.clone()).collect();
    let removed: HashSet<String> = HashSet::new();
    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(system, path, verified_doc, &added_ids, &removed)
    })?;

    Ok(results)
}

/// Walk the segment list once, emitting Remargin comments and
/// preserving body / pre-existing comment segments. Threading state is
/// reset on every chain-breaking event so the rules stay local to this
/// loop.
///
/// Shared between [`migrate`] and
/// [`crate::operations::projections::project_migrate`] so both produce
/// byte-identical output — `plan migrate` is the user-visible preview
/// of what `migrate` will write, and the two diverging would defeat the
/// purpose.
pub(crate) fn build_migrated_segments(
    system: &dyn System,
    segments: &[Segment],
    identities: &MigrateIdentities,
    now: DateTime<FixedOffset>,
) -> Result<(Vec<Segment>, Vec<MigratedComment>)> {
    let mut new_segments: Vec<Segment> = Vec::new();
    let mut results: Vec<MigratedComment> = Vec::new();
    let mut chain: Option<ChainLink> = None;
    let mut emit_index: i64 = 0;

    for seg in segments {
        match seg {
            Segment::Body(text) => {
                if body_breaks_chain(text) {
                    chain = None;
                }
                new_segments.push(Segment::Body(text.clone()));
            }
            Segment::Comment(cm) => {
                // A pre-existing Remargin comment between two legacy
                // comments is treated as a chain break: it represents a
                // foreign conversation, not a continuation of the legacy
                // exchange we are migrating.
                chain = None;
                new_segments.push(Segment::Comment(cm.clone()));
            }
            Segment::LegacyComment(lc) => {
                let existing_ids = collect_ids_from_segments(&new_segments);
                let new_id = id::generate(&existing_ids);
                let ts = now + Duration::microseconds(emit_index);
                emit_index += 1;

                let (author, key_path_opt) = role_attribution(lc.role, identities);
                let author_type = match lc.role {
                    LegacyRole::Agent => AuthorType::Agent,
                    LegacyRole::User => AuthorType::Human,
                };
                let role_str = match lc.role {
                    LegacyRole::Agent => "agent",
                    LegacyRole::User => "user",
                };

                let ack = legacy_done_ack(lc, identities);

                let (reply_to, thread) = match &chain {
                    Some(prev) if prev.parent_role != lc.role => {
                        ack_parent_for_reply(&mut new_segments, prev.parent_idx, &author, ts);
                        (Some(prev.parent_id.clone()), Some(prev.thread_root.clone()))
                    }
                    _ => (None, None),
                };

                // Legacy migration never produces remargin_kind — those
                // are a post-migration concept. `None` keeps the
                // migrated-comment checksum byte-for-byte identical to
                // the pre-rem-n4x7 implementation and leaves the
                // `remargin_kind:` YAML line absent from the migrated
                // block.
                let remargin_kind: Option<Vec<String>> = None;
                let checksum = compute_checksum(&lc.content, &[]);
                let mut comment = Comment {
                    ack,
                    attachments: Vec::new(),
                    author: author.clone(),
                    author_type,
                    checksum,
                    content: lc.content.clone(),
                    id: new_id.clone(),
                    line: 0,
                    reactions: Reactions::new(),
                    remargin_kind,
                    reply_to,
                    signature: None,
                    thread: thread.clone(),
                    to: Vec::new(),
                    ts,
                };

                if let Some(key_path) = key_path_opt {
                    let sig =
                        compute_signature(&comment, &key_path, system).with_context(|| {
                            format!("signing migrated {role_str} comment {new_id:?}")
                        })?;
                    comment.signature = Some(sig);
                }

                results.push(MigratedComment {
                    new_id: new_id.clone(),
                    original_role: String::from(role_str),
                });

                let parent_idx = new_segments.len();
                new_segments.push(Segment::Comment(Box::new(comment)));

                let new_thread_root = thread.unwrap_or_else(|| new_id.clone());
                chain = Some(ChainLink {
                    parent_id: new_id,
                    parent_idx,
                    parent_role: lc.role,
                    thread_root: new_thread_root,
                });
            }
        }
    }

    Ok((new_segments, results))
}

/// True when a body slice breaks an in-progress threading chain.
///
/// The rule is "any non-whitespace character"; an ATX heading
/// trivially contains the `#` character so the same predicate covers
/// both the heading and prose cases without a separate regex.
/// Whitespace-only bodies (blank lines between adjacent fences) keep
/// the chain alive.
fn body_breaks_chain(text: &str) -> bool {
    text.chars().any(|c| !c.is_whitespace())
}

/// Pick `(author, key_path)` for a legacy role. The role determines
/// whether we look at the human or agent slot in `identities`;
/// `None` falls back to the historical `legacy-user` / `legacy-agent`
/// placeholder with no key.
fn role_attribution(role: LegacyRole, identities: &MigrateIdentities) -> (String, Option<PathBuf>) {
    let slot = match role {
        LegacyRole::Agent => identities.agent.as_ref(),
        LegacyRole::User => identities.human.as_ref(),
    };
    slot.map_or_else(
        || (String::from(legacy_placeholder(role)), None),
        |role_identity| {
            (
                role_identity.identity.clone(),
                role_identity.key_path.clone(),
            )
        },
    )
}

/// Hardcoded fallback author used when no identity is supplied for a
/// role.
const fn legacy_placeholder(role: LegacyRole) -> &'static str {
    match role {
        LegacyRole::Agent => "legacy-agent",
        LegacyRole::User => "legacy-user",
    }
}

/// Build the `[done:DATE]`-derived ack list for a legacy comment.
///
/// Mirrors the historical behaviour: an explicit `[done:DATE]` marker
/// becomes a single `Acknowledgment` from the *opposite* role's
/// configured (or fallback) identity at the parsed date. When there is
/// no done marker, the list is empty.
fn legacy_done_ack(
    lc: &parser::LegacyComment,
    identities: &MigrateIdentities,
) -> Vec<Acknowledgment> {
    let Some(date_str) = lc.done_date.as_ref() else {
        return Vec::new();
    };
    let Some(ts) = parse_done_date(date_str) else {
        return Vec::new();
    };
    let opposite = opposite_role(lc.role);
    let (ack_author, _) = role_attribution(opposite, identities);
    vec![Acknowledgment {
        author: ack_author,
        ts,
    }]
}

/// Append an implicit-from-reply ack onto the already-emitted parent
/// comment. The replier acknowledges the parent simply by replying.
fn ack_parent_for_reply(
    new_segments: &mut [Segment],
    parent_idx: usize,
    replier_author: &str,
    ts: DateTime<FixedOffset>,
) {
    if let Some(Segment::Comment(parent)) = new_segments.get_mut(parent_idx) {
        parent.ack.push(Acknowledgment {
            author: String::from(replier_author),
            ts,
        });
    }
}

const fn opposite_role(role: LegacyRole) -> LegacyRole {
    match role {
        LegacyRole::Agent => LegacyRole::User,
        LegacyRole::User => LegacyRole::Agent,
    }
}

/// Collect comment IDs from segments built so far.
fn collect_ids_from_segments(segments: &[Segment]) -> HashSet<&str> {
    segments
        .iter()
        .filter_map(|seg| match seg {
            Segment::Comment(cm) => Some(cm.id.as_str()),
            Segment::Body(_) | Segment::LegacyComment(_) => None,
        })
        .collect()
}

/// Parse a `[done:DATE]` date string into a timestamp.
fn parse_done_date(date_str: &str) -> Option<DateTime<FixedOffset>> {
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    let naive_dt = date.and_time(NaiveTime::from_hms_opt(0, 0, 0)?);
    Some(Utc.from_utc_datetime(&naive_dt).fixed_offset())
}
