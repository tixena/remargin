//! Core infrastructure for the `remargin plan` subcommand (rem-bhk / rem-2qr).
//!
//! `plan` answers the question "what would this op do?" without committing
//! anything to disk. Given a before/after pair of [`ParsedDocument`]s and
//! the active [`ResolvedConfig`], [`project_report`] computes:
//!
//! - The diff of serialized content (whole-file sha256 checksums, changed
//!   line ranges).
//! - The partition of comment ids into `destroyed` / `added` / `modified` /
//!   `preserved`.
//! - A full [`VerifyReport`] projected against the `after` document under
//!   the active mode.
//! - A `would_commit` verdict plus human-readable `reject_reason` when the
//!   projected verify would fail.
//!
//! Per-op wiring lives in follow-up issues (rem-imc, rem-3uo, rem-qll).
//! This module is intentionally pure: it never reads the filesystem,
//! never calls into the signing key, and never mutates either input.
//!
//! The [`PlanReport`] shape matches the JSON payload documented in
//! rem-bhk.

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt::Write as _;

use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::config::ResolvedConfig;
use crate::operations::verify::{VerifyReport, verify_document};
use crate::parser::ParsedDocument;

/// Serialization-friendly mirror of one row of a [`VerifyReport`].
///
/// [`crate::operations::verify::RowStatus`] is deliberately not
/// `Serialize` (the public verify output has its own JSON shape owned by
/// the CLI / MCP layer); `plan` produces JSON directly from
/// [`PlanReport`] so we mirror the fields here with the field names the
/// plan payload documents.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanVerifyRow {
    /// Whether the per-comment content checksum would re-verify.
    pub checksum_ok: bool,
    /// Comment ID.
    pub id: String,
    /// Lowercase signature status name (`valid`, `invalid`, `missing`,
    /// `unknown_author`).
    pub signature: String,
}

/// Serialization-friendly mirror of [`VerifyReport`].
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanVerifyReport {
    /// Aggregate verdict under the active mode.
    pub ok: bool,
    /// Per-comment rows in document order.
    pub rows: Vec<PlanVerifyRow>,
}

impl PlanVerifyReport {
    fn from_report(report: &VerifyReport) -> Self {
        let rows = report
            .results
            .iter()
            .map(|row| PlanVerifyRow {
                checksum_ok: row.checksum_ok,
                id: row.id.clone(),
                signature: String::from(row.signature.as_str()),
            })
            .collect();
        Self {
            ok: report.ok,
            rows,
        }
    }
}

/// Comment-id partition for a single plan projection.
///
/// Every pre-existing comment id lands in exactly one bucket:
///
/// - `destroyed`: present in `before`, absent in `after`.
/// - `modified`: present in both, but the content checksum changed.
/// - `preserved`: present in both with an unchanged content checksum.
///
/// Newly-created comment ids (present only in `after`) land in `added`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct CommentDiff {
    /// Newly-created comment ids (present only in `after`).
    pub added: Vec<String>,
    /// Pre-existing comment ids that would no longer exist in `after`.
    pub destroyed: Vec<String>,
    /// Pre-existing comment ids whose content checksum changed in `after`.
    pub modified: Vec<String>,
    /// Pre-existing comment ids that survive with unchanged content.
    pub preserved: Vec<String>,
}

/// Identity block for a plan report.
///
/// Populated by per-op wiring (follow-up issues); the core projection
/// helper treats it as opaque data the caller owns.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanIdentity {
    /// Author type (`human` / `agent`) as resolved from the active config
    /// and any CLI / MCP override.
    pub author_type: Option<String>,
    /// Author identity as resolved from the active config and any CLI /
    /// MCP override.
    pub name: Option<String>,
    /// Whether the configured private key would load and sign successfully
    /// for this op. `false` means the op would still commit under the
    /// active mode, but without a signature attached.
    pub would_sign: bool,
}

/// Structured prediction of what a mutating op would do against a
/// [`ParsedDocument`], without touching disk.
///
/// Mirrors the JSON payload documented in rem-bhk. Populated by
/// [`project_report`] for the in-memory diff fields; per-op wiring layers
/// on top to populate [`PlanReport::identity`] and any op-specific
/// metadata.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanReport {
    /// 1-indexed inclusive `[start, end]` line ranges that would be
    /// modified. Empty when `noop` is `true`.
    pub changed_line_ranges: Vec<[usize; 2]>,
    /// Whole-file sha256 of the projected markdown (`after.to_markdown()`)
    /// in the `sha256:<hex>` format used by [`crate::crypto::compute_checksum`].
    pub checksum_after: String,
    /// Whole-file sha256 of the source markdown (`before.to_markdown()`)
    /// in the `sha256:<hex>` format used by [`crate::crypto::compute_checksum`].
    pub checksum_before: String,
    /// Partition of comment ids across the projection. See
    /// [`CommentDiff`].
    pub comments: CommentDiff,
    /// Which identity the plan was computed under. `would_sign` reports
    /// whether signing would succeed without actually invoking the key.
    pub identity: PlanIdentity,
    /// `true` when the projected content is byte-identical to the source
    /// content (`checksum_before == checksum_after`).
    pub noop: bool,
    /// The mutating op label (`write`, `comment`, `ack`, `batch`, ...).
    pub op: String,
    /// Human-readable reason when `would_commit` is `false`. `None` when
    /// the projection would commit cleanly. Specific enough to act on
    /// (which comment, which invariant).
    pub reject_reason: Option<String>,
    /// Full post-op verify report computed against the projected
    /// document under the active mode.
    pub verify_after: PlanVerifyReport,
    /// Aggregate verdict: `true` when the op would land successfully
    /// under the current mode and invariants.
    pub would_commit: bool,
}

/// Compute a [`PlanReport`] from a `before`/`after` pair of documents.
///
/// Pure: no disk IO, no signing, no registry mutation. `op_label` is the
/// literal string carried into [`PlanReport::op`] (`"write"`,
/// `"comment"`, `"batch"`, ...). `identity` is threaded through
/// unchanged; per-op wiring owns populating it.
///
/// The comment partition is keyed on [`crate::parser::Comment::checksum`]:
/// a matching id + matching checksum counts as `preserved`, id match with
/// a differing checksum counts as `modified`. Whole-file content diff is
/// reported as 1-indexed inclusive `[start, end]` ranges; contiguous
/// differing lines are coalesced into a single range.
#[must_use]
pub fn project_report(
    op_label: &str,
    before: &ParsedDocument,
    after: &ParsedDocument,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
) -> PlanReport {
    let before_md = before.to_markdown();
    let after_md = after.to_markdown();

    let checksum_before = whole_file_checksum(&before_md);
    let checksum_after = whole_file_checksum(&after_md);
    let noop = checksum_before == checksum_after;

    let changed_line_ranges = if noop {
        Vec::new()
    } else {
        diff_line_ranges(&before_md, &after_md)
    };

    let comments = diff_comment_sets(before, after);
    let raw_verify = verify_document(after, cfg);
    let verify_after = PlanVerifyReport::from_report(&raw_verify);

    let (would_commit, reject_reason) = decide_commit(&raw_verify, cfg);

    PlanReport {
        changed_line_ranges,
        checksum_after,
        checksum_before,
        comments,
        identity,
        noop,
        op: String::from(op_label),
        reject_reason,
        verify_after,
        would_commit,
    }
}

/// Partition pre-existing comment ids into `destroyed` / `modified` /
/// `preserved`, plus new ids into `added`.
///
/// Pure. Keyed on [`crate::parser::Comment::checksum`] for the
/// modified-vs-preserved split.
#[must_use]
pub fn diff_comment_sets(before: &ParsedDocument, after: &ParsedDocument) -> CommentDiff {
    let mut before_ids: BTreeMap<String, String> = BTreeMap::new();
    for cm in before.comments() {
        let _: Option<String> = before_ids.insert(cm.id.clone(), cm.checksum.clone());
    }
    let mut after_ids: BTreeMap<String, String> = BTreeMap::new();
    for cm in after.comments() {
        let _: Option<String> = after_ids.insert(cm.id.clone(), cm.checksum.clone());
    }

    let mut added: Vec<String> = Vec::new();
    let mut destroyed: Vec<String> = Vec::new();
    let mut modified: Vec<String> = Vec::new();
    let mut preserved: Vec<String> = Vec::new();

    for (id, checksum) in &before_ids {
        match after_ids.get(id) {
            None => destroyed.push(id.clone()),
            Some(after_checksum) => {
                if after_checksum == checksum {
                    preserved.push(id.clone());
                } else {
                    modified.push(id.clone());
                }
            }
        }
    }
    for id in after_ids.keys() {
        if !before_ids.contains_key(id) {
            added.push(id.clone());
        }
    }

    CommentDiff {
        added,
        destroyed,
        modified,
        preserved,
    }
}

/// Compute 1-indexed inclusive `[start, end]` line ranges that differ
/// between `before` and `after`.
///
/// Contiguous differing lines are coalesced into a single range. When
/// the two strings have different line counts, the overhang is reported
/// as a trailing range. Returns an empty vec iff the two strings are
/// byte-identical (the caller already handles the `noop` fast path).
fn diff_line_ranges(before: &str, after: &str) -> Vec<[usize; 2]> {
    let before_lines: Vec<&str> = before.split('\n').collect();
    let after_lines: Vec<&str> = after.split('\n').collect();
    let max_len = before_lines.len().max(after_lines.len());

    let mut ranges: Vec<[usize; 2]> = Vec::new();
    let mut active: Option<[usize; 2]> = None;

    for i in 0..max_len {
        let b = before_lines.get(i).copied();
        let a = after_lines.get(i).copied();
        let differs = b != a;
        let line_no = i.saturating_add(1);
        match (&mut active, differs) {
            (Some(range), true) => {
                range[1] = line_no;
            }
            (Some(range), false) => {
                ranges.push(*range);
                active = None;
            }
            (None, true) => {
                active = Some([line_no, line_no]);
            }
            (None, false) => {}
        }
    }
    if let Some(range) = active {
        ranges.push(range);
    }
    ranges
}

/// sha256 of the raw markdown bytes, rendered as `sha256:<hex>` to match
/// [`crate::crypto::compute_checksum`]'s format. Does *not* apply
/// whitespace normalization — this is a whole-file fingerprint, not a
/// per-comment content checksum.
fn whole_file_checksum(content: &str) -> String {
    let hash = Sha256::digest(content.as_bytes());
    let mut hex = String::with_capacity(hash.len() * 2);
    for byte in hash {
        let _ = write!(hex, "{byte:02x}");
    }
    format!("sha256:{hex}")
}

/// Collapse a [`VerifyReport`] into the `would_commit` / `reject_reason`
/// pair emitted in [`PlanReport`].
fn decide_commit(report: &VerifyReport, cfg: &ResolvedConfig) -> (bool, Option<String>) {
    if report.ok {
        return (true, None);
    }
    let mut reason = format!("verify_after would fail under mode {}:", cfg.mode.as_str());
    for row in &report.results {
        if !row.checksum_ok {
            let _ = write!(reason, " checksum mismatch on {};", row.id);
        }
    }
    (false, Some(reason))
}

#[cfg(test)]
mod tests {
    use super::{PlanIdentity, diff_comment_sets, project_report, whole_file_checksum};
    use crate::config::{Mode, ResolvedConfig};
    use crate::parser;
    use crate::parser::AuthorType;

    // DOC_AAA_BAD_CHECKSUM deliberately keeps the original sha256 value
    // from DOC_ONE_COMMENT while editing the content — the checksum is
    // re-verified inside the plan projection and must be flagged as
    // checksum_ok=false (a bad row under every mode).
    const DOC_AAA_BAD_CHECKSUM: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment, edited.\n```\n";

    // DOC_AAA_EDITED is the valid follow-up to DOC_ONE_COMMENT: same id,
    // new content, and the recomputed checksum for the new content. Used
    // to test the `modified` bucket of CommentDiff.
    const DOC_AAA_EDITED: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:be02ec5d99642fe8cb4aa92cf85b1c7a05673353e7e4e8069ca3ce5a227162a6\n---\nFirst comment, edited.\n```\n";

    const DOC_ONE_COMMENT: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n";

    const DOC_TWO_COMMENTS: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n\n```remargin\n---\nid: bbb\nauthor: bob\ntype: human\nts: 2026-04-06T11:00:00-04:00\nchecksum: sha256:91f4d2a3dce415f7e893f7d93f37be404da42b1a7a1133ef759ab3fe747ad726\n---\nSecond comment.\n```\n";

    fn open_config() -> ResolvedConfig {
        ResolvedConfig {
            assets_dir: String::from("assets"),
            author_type: Some(AuthorType::Human),
            identity: Some(String::from("eduardo")),
            ignore: Vec::new(),
            key_path: None,
            mode: Mode::Open,
            registry: None,
            unrestricted: false,
        }
    }

    fn test_identity() -> PlanIdentity {
        PlanIdentity {
            author_type: Some(String::from("human")),
            name: Some(String::from("eduardo")),
            would_sign: false,
        }
    }

    #[test]
    fn whole_file_checksum_matches_known_sha256() {
        let checksum = whole_file_checksum("hello\n");
        assert_eq!(
            checksum,
            "sha256:5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
    }

    #[test]
    fn noop_plan_reports_empty_line_ranges_and_matching_checksums() {
        let before = parser::parse(DOC_ONE_COMMENT).unwrap();
        let after = parser::parse(DOC_ONE_COMMENT).unwrap();

        let report = project_report("write", &before, &after, &open_config(), test_identity());

        assert!(report.noop, "identical inputs must be a noop: {report:?}");
        assert!(report.changed_line_ranges.is_empty());
        assert_eq!(report.checksum_before, report.checksum_after);
        assert_eq!(report.op, "write");
        assert!(report.would_commit);
        assert!(report.reject_reason.is_none());
    }

    #[test]
    fn added_comment_lands_in_added_bucket() {
        let before = parser::parse(DOC_ONE_COMMENT).unwrap();
        let after = parser::parse(DOC_TWO_COMMENTS).unwrap();

        let diff = diff_comment_sets(&before, &after);

        assert_eq!(diff.added, vec![String::from("bbb")]);
        assert_eq!(diff.destroyed, Vec::<String>::new());
        assert_eq!(diff.modified, Vec::<String>::new());
        assert_eq!(diff.preserved, vec![String::from("aaa")]);
    }

    #[test]
    fn destroyed_comment_lands_in_destroyed_bucket() {
        let before = parser::parse(DOC_TWO_COMMENTS).unwrap();
        let after = parser::parse(DOC_ONE_COMMENT).unwrap();

        let diff = diff_comment_sets(&before, &after);

        assert_eq!(diff.added, Vec::<String>::new());
        assert_eq!(diff.destroyed, vec![String::from("bbb")]);
        assert_eq!(diff.modified, Vec::<String>::new());
        assert_eq!(diff.preserved, vec![String::from("aaa")]);
    }

    #[test]
    fn modified_checksum_lands_in_modified_bucket() {
        let before = parser::parse(DOC_ONE_COMMENT).unwrap();
        let after = parser::parse(DOC_AAA_EDITED).unwrap();

        let diff = diff_comment_sets(&before, &after);

        assert_eq!(diff.added, Vec::<String>::new());
        assert_eq!(diff.destroyed, Vec::<String>::new());
        assert_eq!(diff.modified, vec![String::from("aaa")]);
        assert_eq!(diff.preserved, Vec::<String>::new());
    }

    #[test]
    fn plan_report_includes_verify_rows_for_every_after_comment() {
        let before = parser::parse(DOC_ONE_COMMENT).unwrap();
        let after = parser::parse(DOC_TWO_COMMENTS).unwrap();

        let report = project_report("write", &before, &after, &open_config(), test_identity());

        assert_eq!(report.verify_after.rows.len(), 2);
        let ids: Vec<&str> = report
            .verify_after
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(ids, vec!["aaa", "bbb"]);
    }

    #[test]
    fn bad_checksum_drives_would_commit_false_with_reason() {
        // `DOC_AAA_BAD_CHECKSUM` keeps the original checksum value while
        // editing the content; the projected verify therefore flags the
        // row as checksum_ok=false, which is always "bad" regardless of
        // mode.
        let before = parser::parse(DOC_ONE_COMMENT).unwrap();
        let after = parser::parse(DOC_AAA_BAD_CHECKSUM).unwrap();

        let report = project_report("write", &before, &after, &open_config(), test_identity());

        assert!(!report.would_commit);
        assert!(
            report
                .reject_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("aaa")),
            "reject_reason should name the bad-checksum id: {:?}",
            report.reject_reason
        );
    }

    #[test]
    fn changed_line_ranges_coalesce_contiguous_runs() {
        let before = parser::parse("# Title\n\nbody a\nbody b\nbody c\n").unwrap();
        let after = parser::parse("# Title\n\nbody A\nbody B\nbody c\n").unwrap();

        let report = project_report("write", &before, &after, &open_config(), test_identity());

        assert!(!report.noop);
        // Lines 3 and 4 (1-indexed) differ; expect a single coalesced range.
        assert_eq!(report.changed_line_ranges, vec![[3_usize, 4_usize]]);
    }
}
