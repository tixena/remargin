//! Core infrastructure for the `remargin plan` subcommand.
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
//! This module is intentionally pure: it never reads the filesystem,
//! never calls into the signing key, and never mutates either input.

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::advice::OpAdvice;
use crate::comment_style;
use crate::config::{Mode, ResolvedConfig};
use crate::document::allowlist;
use crate::document::{self, WriteOptions, WriteProjection};
use crate::operations::projections::{self, ProjectBatchOp, ProjectCommentParams};
use crate::operations::sign::SignSelection;
use crate::operations::verify::{Anomaly, VerifyReport, anomalies_for_doc, verify_document};
use crate::parser::{self, ParsedDocument};
use crate::permissions::claude_sync::rule_shape::OverlapKind;
use crate::permissions::restrict::{RestrictArgs, RestrictEntryProjection};
use crate::permissions::unprotect::UnprotectArgs;

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
    pub checksum_ok: bool,
    pub id: String,
    /// Lowercase status name: `valid`, `invalid`, `missing` or `unknown_author`.
    pub signature: String,
}

/// Serialization-friendly mirror of [`VerifyReport`].
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanVerifyReport {
    pub ok: bool,
    /// In document order.
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

/// One `(comment_id, anomaly_kind)` pair surfaced by [`PlanSubsetGate`].
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanAnomaly {
    pub id: String,
    /// Stable anomaly kind name, as [`crate::operations::verify::AnomalyKind::as_str`] renders it.
    pub kind: String,
}

/// Subset-gate refusal carried inside a [`PlanReport`].
///
/// Mirrors [`crate::operations::verify::SubsetGateFailure`] in shape so
/// `plan` reports and live `commit_with_verify` errors render
/// identically. `would_commit` is `false` whenever this field is
/// `Some`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanSubsetGate {
    pub headline: String,
    pub hint: String,
    /// Anomalies the projected op would introduce that the pre-state did not have.
    pub introduced: Vec<PlanAnomaly>,
    /// The active mode after realm escalation.
    pub mode: String,
    pub path: PathBuf,
}

impl PlanSubsetGate {
    fn from_introduced(introduced: Vec<Anomaly>, mode: &Mode, path: PathBuf) -> Self {
        let n = introduced.len();
        let path_str = path.display().to_string();
        let headline = if n == 1 {
            format!("op would introduce 1 new anomaly in {path_str}")
        } else {
            format!("op would introduce {n} new anomalies in {path_str}")
        };
        let hint = format!("Try `remargin verify {path_str} --json` for the full breakdown.");
        let plan_anomalies = introduced
            .into_iter()
            .map(|a| PlanAnomaly {
                id: a.id,
                kind: String::from(a.kind.as_str()),
            })
            .collect();
        Self {
            headline,
            hint,
            introduced: plan_anomalies,
            mode: String::from(mode.as_str()),
            path,
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
    pub added: Vec<String>,
    pub destroyed: Vec<String>,
    pub modified: Vec<String>,
    pub preserved: Vec<String>,
}

/// Identity block for a plan report. The core projection helper treats it as opaque data the
/// caller owns.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanIdentity {
    pub author_type: Option<String>,
    pub name: Option<String>,
    /// `false` means the op would still commit under the active mode, but unsigned.
    pub would_sign: bool,
}

impl PlanIdentity {
    /// Canonical builder shared by every adapter (CLI + MCP).
    ///
    /// `would_sign` is `true` when a key path is configured. The key is not loaded here: `plan`
    /// stays side-effect-free.
    #[must_use]
    pub fn from_config(cfg: &ResolvedConfig) -> Self {
        let author_type = cfg.author_type.as_ref().map(|t| String::from(t.as_str()));
        Self::new(cfg.identity.clone(), author_type, cfg.key_path.is_some())
    }

    /// Build a [`PlanIdentity`] from the three fields. The
    /// constructor exists so external crates can populate the struct
    /// without tripping `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(name: Option<String>, author_type: Option<String>, would_sign: bool) -> Self {
        Self {
            author_type,
            name,
            would_sign,
        }
    }
}

/// Structured prediction of what a mutating op would do against a
/// [`ParsedDocument`], without touching disk.
///
/// Populated by [`project_report`] for the in-memory diff fields;
/// per-op wiring layers on top to populate [`PlanReport::identity`]
/// and any op-specific metadata.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlanReport {
    /// 1-indexed inclusive `[start, end]` ranges; empty when `noop` is `true`.
    pub changed_line_ranges: Vec<[usize; 2]>,
    /// Whole-file sha256 of the projected markdown, as `sha256:<hex>`.
    pub checksum_after: String,
    /// Whole-file sha256 of the source markdown, as `sha256:<hex>`.
    pub checksum_before: String,
    pub comments: CommentDiff,
    /// Set only by the `restrict` op.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_diff: Option<ConfigPlanDiff>,
    /// Set only by the `cp` op, whose document-level fields stay empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cp_diff: Option<CpDiff>,
    pub identity: PlanIdentity,
    /// Set only by the `mv` op, whose document-level fields stay empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mv_diff: Option<MvDiff>,
    /// `checksum_before == checksum_after`.
    pub noop: bool,
    pub op: String,
    /// Set only by a recursive purge, whose document-level fields stay empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purge_dir_diff: Option<PurgeDirDiff>,
    /// `None` when the projection would commit cleanly.
    pub reject_reason: Option<String>,
    /// Set alongside `reject_reason` when the op would introduce new anomalies; `None` when the
    /// refusal came from elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subset_gate: Option<PlanSubsetGate>,
    /// Set only by the `unprotect` op.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unprotect_diff: Option<UnprotectConfigDiff>,
    /// Computed against the projected document under the active mode.
    pub verify_after: PlanVerifyReport,
    /// Warn-tier notes on the projected bodies, each tagged with its sub-op (`0` for a single-body
    /// op). Advice only, never a failure signal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<OpAdvice>,
    pub would_commit: bool,
}

/// Per-file projection emitted by the `plan restrict` op.
///
/// `restrict` is a sanctioned config write that touches four files in
/// one go: `<anchor>/.remargin.yaml`, the project + user-scope
/// `.claude/settings(.local).json`, and the
/// `.claude/.remargin-restrictions.json` sidecar. This struct names
/// every file, every entry that would be added vs. left alone, and
/// every detectable conflict, so callers can preview the full mutation
/// before committing.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ConfigPlanDiff {
    /// Canonical; for the wildcard form this is the anchor root.
    pub absolute_path: PathBuf,
    /// `.claude/`-bearing ancestor that anchors the write.
    pub anchor: PathBuf,
    /// Advisory: `would_commit` stays `true` when this is non-empty.
    pub conflicts: Vec<ConfigConflict>,
    pub remargin_yaml: RemarginYamlDiff,
    /// Project scope first, user scope second.
    pub settings_files: Vec<SettingsFileDiff>,
    pub sidecar: SidecarDiff,
}

/// Projection of the `<anchor>/.remargin.yaml` write performed by
/// `restrict`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct RemarginYamlDiff {
    pub entry_action: EntryAction,
    pub path: PathBuf,
    /// The on-disk entry for this path; `None` when none matches.
    pub previous_entry: Option<RestrictEntryProjection>,
    pub projected_entry: Option<RestrictEntryProjection>,
    pub will_be_created: bool,
}

/// Projection of one Claude settings file
/// (`.claude/settings.local.json` for the project scope or
/// `~/.claude/settings.json` for the user scope) that
/// `restrict` would write into.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SettingsFileDiff {
    pub allow_rules_already_present: Vec<String>,
    pub allow_rules_to_add: Vec<String>,
    pub deny_rules_already_present: Vec<String>,
    pub deny_rules_to_add: Vec<String>,
    pub path: PathBuf,
    pub will_be_created: bool,
}

/// Projection of `<anchor>/.claude/.remargin-restrictions.json`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SidecarDiff {
    pub entry_action: EntryAction,
    pub path: PathBuf,
    pub will_be_created: bool,
}

/// Detectable conflict surfaced in [`ConfigPlanDiff::conflicts`].
///
/// All variants are advisory — a non-empty `conflicts` array does
/// not flip `would_commit` to false. Callers decide whether to apply
/// the projection anyway. Strict / fail-closed modes can branch on
/// the variant in a wrapper.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConfigConflict {
    /// An existing `permissions.allow` rule overlaps a rule the projection would add to
    /// `permissions.deny` in the same settings file.
    AllowDenyOverlap {
        allow_rule: String,
        overlap_kind: OverlapKind,
        projected_deny_rule: String,
        settings_file: PathBuf,
    },
    /// `find_claude_anchor` walked above the caller's `cwd`, so the anchor is an ancestor of it.
    AnchorIsAncestor { anchor: PathBuf, cwd: PathBuf },
    /// `trusted_roots` already has an entry for the path with different `also_deny_bash` or
    /// `cli_allowed`; the live op overwrites it silently.
    YamlEntryWouldChange {
        path: String,
        previous: RestrictEntryProjection,
        projected: RestrictEntryProjection,
    },
}

/// What [`RemarginYamlDiff`] / [`SidecarDiff`] would do to its target
/// entry. Mirrors a write-versus-skip decision.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EntryAction {
    Added,
    Noop,
    Updated,
}

/// Reverse projection emitted by the `plan unprotect` op.
///
/// Symmetric mirror of [`ConfigPlanDiff`] for the reverse direction.
/// `unprotect` is the explicit, sanctioned reversal of a previous
/// `restrict`: it removes the matching `permissions.trusted_roots` entry
/// from `<anchor>/.remargin.yaml`, scrubs the sidecar-tracked rules
/// from each Claude settings file the original `apply_rules` recorded,
/// and finally drops the sidecar entry. This struct names every file,
/// every entry that would be removed vs. left alone, and every
/// detectable drift conflict, so callers can preview the full
/// reversal before committing.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UnprotectConfigDiff {
    /// Canonical; for the wildcard form this is the anchor root.
    pub absolute_path: PathBuf,
    /// `.claude/`-bearing ancestor that anchors the reversal.
    pub anchor: PathBuf,
    /// Advisory: `would_commit` stays `true` when this is non-empty.
    pub conflicts: Vec<UnprotectConflict>,
    pub remargin_yaml: UnprotectYamlDiff,
    /// Sourced from the sidecar's `added_to_files`; empty when the sidecar has no entry for the path.
    pub settings_files: Vec<UnprotectSettingsDiff>,
    pub sidecar: UnprotectSidecarDiff,
}

/// Projection of the `<anchor>/.remargin.yaml` write performed by
/// `unprotect` — mirror of [`RemarginYamlDiff`] for the reverse
/// direction.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UnprotectYamlDiff {
    pub entry_action: UnprotectEntryAction,
    pub path: PathBuf,
    /// `None` when no existing entry matches the path.
    pub previous_entry: Option<RestrictEntryProjection>,
}

/// Projection of one Claude settings file that `unprotect` would
/// scrub — mirror of [`SettingsFileDiff`] for the reverse direction.
///
/// Covers both the project-scope `.claude/settings.local.json` and
/// the user-scope `~/.claude/settings.json`. The actual list of
/// targets is sourced from the sidecar's `added_to_files` array
/// captured at apply time.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UnprotectSettingsDiff {
    pub path: PathBuf,
    /// Tracked by the sidecar but missing from the file; each one also surfaces as a
    /// [`UnprotectConflict::RuleAlreadyAbsent`].
    pub rules_already_absent: Vec<String>,
    pub rules_to_remove: Vec<String>,
}

/// Projection of `<anchor>/.claude/.remargin-restrictions.json` —
/// mirror of [`SidecarDiff`] for the reverse direction.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UnprotectSidecarDiff {
    pub entry_action: UnprotectEntryAction,
    pub path: PathBuf,
}

/// What [`UnprotectYamlDiff`] / [`UnprotectSidecarDiff`] would do to
/// its target entry. Mirrors a remove-versus-skip decision.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnprotectEntryAction {
    Absent,
    WouldBeRemoved,
}

/// Detectable drift conflict surfaced in
/// [`UnprotectConfigDiff::conflicts`].
///
/// All variants are advisory — a non-empty `conflicts` array does
/// not flip `would_commit` to false. Callers decide whether to
/// apply the projection anyway.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnprotectConflict {
    /// A rule the sidecar lists for `settings_file` is missing from that file.
    RuleAlreadyAbsent {
        rule: String,
        settings_file: PathBuf,
    },
    /// The sidecar has no entry for the path: the YAML removal proceeds, the settings files are
    /// left alone.
    SidecarEntryMissing { path: PathBuf },
    /// `trusted_roots` has no entry for the path: the sidecar removal proceeds, the YAML is left
    /// alone.
    YamlEntryMissing { path: PathBuf },
}

/// Recursive-purge projection emitted by `plan purge --recursive`.
///
/// Mirrors the read-only side of
/// [`crate::operations::purge::purge_dir`]: enumerates every visible
/// `.md` file the live op would attempt under the requested directory,
/// names per-file projected outcomes (would-purge / would-noop /
/// would-refuse), and surfaces refusal reasons verbatim so a caller
/// previewing the recursive purge can see exactly which files would
/// be touched and which would be blocked by `op_guard` / allow-list.
///
/// The document-level fields on the carrying [`PlanReport`]
/// (`comments`, `changed_line_ranges`, `checksum_*`, `verify_after`)
/// stay vacuously empty because each per-file projection here carries
/// its own counters; the directory case has no single before/after
/// document to diff against.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PurgeDirDiff {
    pub directory: PathBuf,
    /// Sorted by path.
    pub files: Vec<PurgeDirFileDiff>,
    pub no_md_files: bool,
}

/// One per-file outcome inside a [`PurgeDirDiff`].
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PurgeDirFileDiff {
    pub attachments_cleaned: usize,
    pub comments_removed: usize,
    pub outcome: PurgeDirFileOutcome,
    pub path: PathBuf,
    /// Set only when `outcome` is `Refused`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
}

/// What the live `purge_dir` would do to one file in a
/// [`PurgeDirDiff`]. Mirrors [`PurgeBulkResult`]'s three buckets.
///
/// [`PurgeBulkResult`]: crate::operations::purge::PurgeBulkResult
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PurgeDirFileOutcome {
    /// Refused by `op_guard`, the allow-list or a forbidden target; the rest of the projection
    /// goes on.
    Refused,
    /// The file has no remargin comments, so the live op never writes it.
    Skipped,
    WouldPurge,
}

/// File-relocation projection emitted by the `plan mv` op.
///
/// Mirrors the read-only side of [`crate::operations::mv::mv`]: names
/// the canonical src/dst, whether the destination already exists (and
/// would therefore require `--force` to overwrite), whether the
/// call would be a same-path no-op or an idempotent re-run after a
/// previous successful move, and — when the source is a directory
/// — the count of nested files that would move with it.
/// Bool fields are split into two `#[serde(flatten)]` substructs
/// (`MvExistence`, `MvState`) so the canonical JSON output stays a
/// flat object — every key visible to consumers stays unchanged —
/// while no single struct holds more than three bools.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MvDiff {
    pub dst_absolute: PathBuf,
    #[serde(flatten)]
    pub existence: MvExistence,
    /// `0` for a file move, a no-op or an already-settled re-run.
    pub nested_files_moved: usize,
    /// When the source is missing this is the lexical join of `base_dir` and the requested path.
    pub src_absolute: PathBuf,
    #[serde(flatten)]
    pub state: MvState,
}

/// What `src` and `dst` resolve to on disk, plus whether the source
/// is a directory. Flattened into [`MvDiff`]'s JSON output.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MvExistence {
    pub dst_exists: bool,
    pub is_directory: bool,
    pub src_exists: bool,
}

/// Terminal-no-work-needed indicators for the move. Flattened into
/// [`MvDiff`]'s JSON output.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MvState {
    /// The source is missing and the destination already exists: the live op settles with
    /// `bytes_moved = 0`.
    pub idempotent_already_settled: bool,
    pub noop_same_path: bool,
}

/// Projection of the `cp` op — what a live `cp` would do.
///
/// Emitted in [`PlanReport::cp_diff`] when `op = cp`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct CpDiff {
    /// `0` for the `Verbatim` and `Noop` paths.
    pub comments_to_drop: usize,
    pub dst_absolute: PathBuf,
    pub dst_exists: bool,
    pub kind: String,
    /// When the source is missing this is the lexical join of `base_dir` and the requested path.
    pub src_absolute: PathBuf,
}

/// A `plan` request for a single mutating op, normalized so CLI + MCP
/// can share one dispatch path.
///
/// Each variant mirrors one mutating op. Adapters construct the variant
/// from their native input shape; [`dispatch`] converts it to a
/// [`PlanReport`]. Adapters must not re-implement the per-op projection
/// wiring; when a new plan op lands, extend this enum and [`dispatch`]
/// once — both surfaces pick up the change automatically.
#[non_exhaustive]
pub enum PlanRequest<'req> {
    Ack {
        /// Already joined against the base dir.
        path: PathBuf,
        ids: Vec<String>,
        remove: bool,
    },
    Batch {
        path: PathBuf,
        ops: Vec<ProjectBatchOp>,
    },
    Comment {
        path: PathBuf,
        params: ProjectCommentParams<'req>,
    },
    Cp {
        src: PathBuf,
        dst: PathBuf,
        force: bool,
    },
    Delete {
        path: PathBuf,
        ids: Vec<String>,
    },
    Edit {
        path: PathBuf,
        id: &'req str,
        content: &'req str,
    },
    Mv {
        src: PathBuf,
        dst: PathBuf,
        force: bool,
    },
    /// With `recursive`, `path` is a directory and the report carries a [`PurgeDirDiff`].
    Purge {
        path: PathBuf,
        recursive: bool,
    },
    React {
        path: PathBuf,
        id: &'req str,
        emoji: &'req str,
        remove: bool,
    },
    Restrict {
        /// Used for anchor discovery.
        cwd: PathBuf,
        args: RestrictArgs,
        /// Project and user scope, resolved by the adapter before dispatch.
        settings_files: Vec<PathBuf>,
    },
    SandboxAdd {
        path: PathBuf,
    },
    SandboxRemove {
        path: PathBuf,
    },
    /// Unlike most plan ops this loads the signing key and attaches real signatures to the
    /// projected document, so `verify_after` predicts the post-op gate.
    Sign {
        path: PathBuf,
        selection: SignSelection,
    },
    Unprotect {
        /// Used for anchor discovery.
        cwd: PathBuf,
        args: UnprotectArgs,
    },
    Write {
        /// Relative to `base_dir`, exactly as the adapter received it.
        path: PathBuf,
        content: &'req str,
        opts: WriteOptions,
    },
}

impl PlanRequest<'_> {
    /// Short human-readable label used as [`PlanReport::op`].
    #[must_use]
    pub const fn op_label(&self) -> &'static str {
        match self {
            Self::Ack {
                path: _,
                ids: _,
                remove: _,
            } => "ack",
            Self::Batch { path: _, ops: _ } => "batch",
            Self::Comment { path: _, params: _ } => "comment",
            Self::Cp {
                src: _,
                dst: _,
                force: _,
            } => "cp",
            Self::Delete { path: _, ids: _ } => "delete",
            Self::Edit {
                path: _,
                id: _,
                content: _,
            } => "edit",
            Self::Mv {
                src: _,
                dst: _,
                force: _,
            } => "mv",
            Self::Purge {
                path: _,
                recursive: _,
            } => "purge",
            Self::React {
                path: _,
                id: _,
                emoji: _,
                remove: _,
            } => "react",
            Self::Restrict {
                cwd: _,
                args: _,
                settings_files: _,
            } => "restrict",
            Self::SandboxAdd { path: _ } => "sandbox-add",
            Self::SandboxRemove { path: _ } => "sandbox-remove",
            Self::Sign {
                path: _,
                selection: _,
            } => "sign",
            Self::Unprotect { cwd: _, args: _ } => "unprotect",
            Self::Write {
                path: _,
                content: _,
                opts: _,
            } => "write",
        }
    }
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
///
/// # Errors
///
/// Propagates [`ParsedDocument::to_markdown`]'s `serde_yaml::Error`
/// when serializing the before/after documents.
pub fn project_report(
    op_label: &str,
    before: &ParsedDocument,
    after: &ParsedDocument,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
) -> Result<PlanReport> {
    let before_md = before.to_markdown()?;
    let after_md = after.to_markdown()?;

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

    let (would_commit, reject_reason, subset_gate) = decide_commit(before, after, cfg, None);

    Ok(PlanReport {
        changed_line_ranges,
        checksum_after,
        checksum_before,
        comments,
        config_diff: None,
        cp_diff: None,
        identity,
        mv_diff: None,
        noop,
        op: String::from(op_label),
        purge_dir_diff: None,
        reject_reason,
        subset_gate,
        unprotect_diff: None,
        verify_after,
        warnings: Vec::new(),
        would_commit,
    })
}

/// Doc-bearing variant of [`project_report`].
///
/// Escalates the mode to the doc's realm (mirroring
/// [`crate::operations::verify::commit_with_verify`]) and surfaces the
/// structured [`PlanSubsetGate`] under that realm so plan dry-runs match
/// what the live op would refuse.
///
/// # Errors
///
/// Propagates a read-gate refusal from the doc's realm,
/// [`ParsedDocument::to_markdown`]'s `serde_yaml::Error`, and any failure
/// from [`crate::config::ResolvedConfig::escalate_mode_for_doc`] (e.g.
/// unreadable realm `.remargin.yaml`).
pub fn project_doc_report(
    system: &dyn System,
    path: &Path,
    op_label: &str,
    before: &ParsedDocument,
    after: &ParsedDocument,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
) -> Result<PlanReport> {
    cfg.ensure_can_read(system, path)?;
    let realm_cfg = cfg.escalate_mode_for_doc(system, path)?;

    let before_md = before.to_markdown()?;
    let after_md = after.to_markdown()?;

    let checksum_before = whole_file_checksum(&before_md);
    let checksum_after = whole_file_checksum(&after_md);
    let noop = checksum_before == checksum_after;

    let changed_line_ranges = if noop {
        Vec::new()
    } else {
        diff_line_ranges(&before_md, &after_md)
    };

    let comments = diff_comment_sets(before, after);
    let raw_verify = verify_document(after, &realm_cfg);
    let verify_after = PlanVerifyReport::from_report(&raw_verify);

    let (would_commit, reject_reason, subset_gate) =
        decide_commit(before, after, &realm_cfg, Some(path.to_path_buf()));

    Ok(PlanReport {
        changed_line_ranges,
        checksum_after,
        checksum_before,
        comments,
        config_diff: None,
        cp_diff: None,
        identity,
        mv_diff: None,
        noop,
        op: String::from(op_label),
        purge_dir_diff: None,
        reject_reason,
        subset_gate,
        unprotect_diff: None,
        verify_after,
        warnings: Vec::new(),
        would_commit,
    })
}

/// Canonical plan dispatcher shared by CLI + MCP.
///
/// Runs the right `project_*` helper for the requested op, folds the
/// result through [`project_report`] with [`PlanIdentity::from_config`],
/// and returns a [`PlanReport`] both adapters can serialize in their
/// native format. `base_dir` is the CLI's `cwd` or the MCP server's
/// `base_dir`; only the [`PlanRequest::Write`] arm consults it
/// (every other projection is already handed a joined `path`).
///
/// # Errors
///
/// Propagates preflight failures from the per-op projection helpers
/// (missing identity, linter violations, bad frontmatter, etc.).
pub fn dispatch(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    request: &PlanRequest<'_>,
) -> Result<PlanReport> {
    let label = request.op_label();
    let identity = PlanIdentity::from_config(cfg);

    match request {
        PlanRequest::Ack { path, ids, remove } => {
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let (before, after) = projections::project_ack(system, path, cfg, &id_refs, *remove)?;
            project_doc_report(system, path, label, &before, &after, cfg, identity)
        }
        PlanRequest::Batch { path, ops } => {
            let (before, after) = projections::project_batch(system, path, cfg, ops)?;
            let mut report =
                project_doc_report(system, path, label, &before, &after, cfg, identity)?;
            report.warnings = body_warnings(ops.iter().map(|op| op.content.as_str()));
            Ok(report)
        }
        PlanRequest::Comment { path, params } => {
            let (before, after) = projections::project_comment(system, path, cfg, params)?;
            let mut report =
                project_doc_report(system, path, label, &before, &after, cfg, identity)?;
            report.warnings = body_warnings([params.content]);
            Ok(report)
        }
        PlanRequest::Delete { path, ids } => {
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let (before, after) = projections::project_delete(system, path, cfg, &id_refs)?;
            project_doc_report(system, path, label, &before, &after, cfg, identity)
        }
        PlanRequest::Cp { src, dst, force } => {
            dispatch_cp(system, base_dir, cfg, identity, src, dst, *force)
        }
        PlanRequest::Edit { path, id, content } => {
            let (before, after) = projections::project_edit(system, path, cfg, id, content)?;
            let mut report =
                project_doc_report(system, path, label, &before, &after, cfg, identity)?;
            report.warnings = body_warnings([*content]);
            Ok(report)
        }
        PlanRequest::Mv { src, dst, force } => {
            dispatch_mv(system, base_dir, cfg, identity, src, dst, *force)
        }
        PlanRequest::Purge { path, recursive } => {
            if *recursive {
                dispatch_purge_dir(system, cfg, identity, path)
            } else {
                let (before, after) = projections::project_purge(system, path, cfg)?;
                project_doc_report(system, path, label, &before, &after, cfg, identity)
            }
        }
        PlanRequest::React {
            path,
            id,
            emoji,
            remove,
        } => {
            let (before, after) =
                projections::project_react(system, path, cfg, id, emoji, *remove)?;
            project_doc_report(system, path, label, &before, &after, cfg, identity)
        }
        PlanRequest::Restrict {
            cwd,
            args,
            settings_files,
        } => dispatch_restrict(system, cfg, identity, cwd, args, settings_files),
        PlanRequest::SandboxAdd { path } => {
            let (before, after) = projections::project_sandbox_add(system, path, cfg)?;
            project_doc_report(system, path, label, &before, &after, cfg, identity)
        }
        PlanRequest::SandboxRemove { path } => {
            let (before, after) = projections::project_sandbox_remove(system, path, cfg)?;
            project_doc_report(system, path, label, &before, &after, cfg, identity)
        }
        PlanRequest::Sign { path, selection } => {
            let (before, after) = projections::project_sign(system, path, cfg, selection)?;
            project_doc_report(system, path, label, &before, &after, cfg, identity)
        }
        PlanRequest::Unprotect { cwd, args } => {
            dispatch_unprotect(system, cfg, identity, cwd, args)
        }
        PlanRequest::Write {
            path,
            content,
            opts,
        } => {
            let projection = document::project_write(system, base_dir, path, content, cfg, *opts)?;
            dispatch_write_projection(system, path, &projection, cfg, identity)
        }
    }
}

/// Build a [`PlanReport`] from the `restrict` projection's verdict.
/// Mirrors [`dispatch_write_projection`]'s handling of the document
/// `Unsupported` arm: a hard reject from
/// [`projections::project_restrict`] flips `would_commit` to false and
/// surfaces the carried reason verbatim.
fn dispatch_restrict(
    system: &dyn System,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
    cwd: &Path,
    args: &RestrictArgs,
    settings_files: &[PathBuf],
) -> Result<PlanReport> {
    let projection = projections::restrict::project_restrict(system, cwd, args, settings_files)?;
    let empty = parser::parse("").context("parsing empty before-document for plan restrict")?;
    let mut report = project_report("restrict", &empty, &empty, cfg, identity)?;
    match projection {
        projections::restrict::RestrictProjection::Diff(diff) => {
            report.noop = is_diff_noop(&diff);
            report.config_diff = Some(*diff);
        }
        projections::restrict::RestrictProjection::Reject(reason) => {
            report.reject_reason = Some(reason);
            report.would_commit = false;
        }
    }
    Ok(report)
}

/// Build a [`PlanReport`] from the `unprotect` projection's verdict.
/// Symmetric mirror of [`dispatch_restrict`] for the reverse
/// direction: a hard reject from [`projections::project_unprotect`]
/// flips `would_commit` to false and surfaces the carried reason
/// verbatim.
fn dispatch_unprotect(
    system: &dyn System,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
    cwd: &Path,
    args: &UnprotectArgs,
) -> Result<PlanReport> {
    let projection = projections::unprotect::project_unprotect(system, cwd, args)?;
    let empty = parser::parse("").context("parsing empty before-document for plan unprotect")?;
    let mut report = project_report("unprotect", &empty, &empty, cfg, identity)?;
    match projection {
        projections::unprotect::UnprotectProjection::Diff(diff) => {
            report.noop = is_unprotect_diff_noop(&diff);
            report.would_commit = !report.noop;
            report.unprotect_diff = Some(*diff);
        }
        projections::unprotect::UnprotectProjection::Reject(reason) => {
            report.reject_reason = Some(reason);
            report.would_commit = false;
        }
    }
    Ok(report)
}

/// The warn tier every projected body earns, tagged with its position in
/// the request. Reached only after the projection succeeded, so the
/// reject tier is already spent and everything here is advice about a
/// body that would land.
fn body_warnings<'body>(bodies: impl IntoIterator<Item = &'body str>) -> Vec<OpAdvice> {
    bodies
        .into_iter()
        .enumerate()
        .flat_map(|(idx, body)| {
            comment_style::notes(body)
                .into_iter()
                .map(move |note| OpAdvice::new(idx, note))
        })
        .collect()
}

/// Build a [`PlanReport`] for the `cp` op.
///
/// Pure: no disk writes, no identity load. Resolves both endpoints
/// through the same sandbox boundary the live op uses, surfaces a
/// `reject_reason` plus `would_commit = false` for hard preflight
/// failures, and otherwise returns a populated [`CpDiff`] with
/// `would_commit = true`.
fn dispatch_cp(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
    src: &Path,
    dst: &Path,
    force: bool,
) -> Result<PlanReport> {
    let empty = parser::parse("").context("parsing empty before-document for plan cp")?;
    let mut report = project_report("cp", &empty, &empty, cfg, identity)?;

    let projection = project_cp(system, base_dir, cfg, src, dst, force);

    match projection {
        Ok(diff) => {
            report.noop = diff.kind == "noop";
            report.would_commit = true;
            report.cp_diff = Some(diff);
        }
        Err(err) => {
            report.reject_reason = Some(format!("{err:#}"));
            report.would_commit = false;
        }
    }

    Ok(report)
}

/// Run every preflight check the live `cp` op would run and assemble a
/// [`CpDiff`] describing the projected end state. Kept structurally
/// parallel to the live op so plan and live `cp` stay in sync.
fn project_cp(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    src: &Path,
    dst: &Path,
    force: bool,
) -> Result<CpDiff> {
    use crate::permissions::op_guard::pre_mutate_check_for_caller;

    let (src_resolved, dst_resolved) = plan_cp_resolve_endpoints(system, base_dir, cfg, src, dst)?;

    let dst_exists = system.exists(&dst_resolved).unwrap_or(false);

    if src_resolved == dst_resolved {
        return Ok(CpDiff {
            comments_to_drop: 0,
            dst_absolute: dst_resolved,
            dst_exists,
            kind: String::from("noop"),
            src_absolute: src_resolved,
        });
    }

    let caller = cfg.caller_info();
    pre_mutate_check_for_caller(system, "cp", &dst_resolved, &caller)?;
    pre_mutate_check_for_caller(system, "cp", &src_resolved, &caller)?;

    if dst_exists && !force {
        anyhow::bail!(
            "destination exists: {} (pass --force to overwrite)",
            dst.display()
        );
    }

    plan_cp_diff_kind(system, src_resolved, dst_resolved, dst_exists)
}

/// Validate source/destination shapes and resolve both endpoints through
/// the sandbox boundary. Returns `(src_resolved, dst_resolved)`.
fn plan_cp_resolve_endpoints(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    src: &Path,
    dst: &Path,
) -> Result<(PathBuf, PathBuf)> {
    use crate::writer::ensure_not_forbidden_target as forbid;

    forbid(src)?;
    forbid(dst)?;

    let src_lexical = if src.is_absolute() {
        src.to_path_buf()
    } else {
        base_dir.join(src)
    };
    let dst_lexical = if dst.is_absolute() {
        dst.to_path_buf()
    } else {
        base_dir.join(dst)
    };

    if !system.exists(&src_lexical).unwrap_or(false) {
        anyhow::bail!("source not found: {}", src.display());
    }
    if system.is_dir(&src_lexical).unwrap_or(false) {
        anyhow::bail!(
            "source is a directory: {} (recursive copy is not supported in v1)",
            src.display()
        );
    }
    if system.is_dir(&dst_lexical).unwrap_or(false) {
        anyhow::bail!(
            "destination is a directory: {} (pass an explicit destination path)",
            dst.display()
        );
    }

    let src_resolved =
        allowlist::resolve_sandboxed(system, base_dir, src, cfg.unrestricted, &cfg.trusted_roots)?;
    let dst_resolved = allowlist::resolve_sandboxed_create(
        system,
        base_dir,
        dst,
        cfg.unrestricted,
        &cfg.trusted_roots,
    )?;

    forbid(&src_resolved)?;
    forbid(&dst_resolved)?;

    Ok((src_resolved, dst_resolved))
}

/// Determine the copy kind by inspecting the source content and return
/// the corresponding [`CpDiff`].
fn plan_cp_diff_kind(
    system: &dyn System,
    src_resolved: PathBuf,
    dst_resolved: PathBuf,
    dst_exists: bool,
) -> Result<CpDiff> {
    let is_md = src_resolved
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            let l = e.to_ascii_lowercase();
            l == "md" || l == "mdx"
        });

    if !is_md {
        return Ok(CpDiff {
            comments_to_drop: 0,
            dst_absolute: dst_resolved,
            dst_exists,
            kind: String::from("verbatim"),
            src_absolute: src_resolved,
        });
    }

    let src_content = system
        .read_to_string(&src_resolved)
        .with_context(|| format!("reading {}", src_resolved.display()))?;
    let parsed = parser::parse(&src_content)
        .with_context(|| format!("parsing {}", src_resolved.display()))?;
    let comment_count = parsed.comments().len();

    let (kind, comments_to_drop) = if comment_count == 0 {
        (String::from("verbatim"), 0)
    } else {
        (String::from("body_only"), comment_count)
    };

    Ok(CpDiff {
        comments_to_drop,
        dst_absolute: dst_resolved,
        dst_exists,
        kind,
        src_absolute: src_resolved,
    })
}

/// Build a [`PlanReport`] for the `mv` op.
///
/// Pure: no disk writes, no identity load. Resolves both endpoints
/// through the same sandbox boundary the live op uses, surfaces a
/// `reject_reason` plus `would_commit = false` for hard preflight
/// failures (path escape, forbidden basename, `trusted_roots`
/// violation, source-and-dest both missing, dst-is-a-directory while
/// src is a file), and otherwise returns a populated [`MvDiff`] with
/// `would_commit = true`. A directory source is supported — the diff
/// carries `is_directory: true` and the nested file count.
fn dispatch_mv(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
    src: &Path,
    dst: &Path,
    force: bool,
) -> Result<PlanReport> {
    let empty = parser::parse("").context("parsing empty before-document for plan mv")?;
    let mut report = project_report("mv", &empty, &empty, cfg, identity)?;

    let projection = project_mv(system, base_dir, cfg, src, dst, force);

    match projection {
        Ok(diff) => {
            report.noop = diff.state.noop_same_path || diff.state.idempotent_already_settled;
            report.would_commit = true;
            report.mv_diff = Some(diff);
        }
        Err(err) => {
            report.reject_reason = Some(format!("{err:#}"));
            report.would_commit = false;
        }
    }

    Ok(report)
}

/// Run every preflight check the live `mv` op would run, in the same
/// order, and assemble an [`MvDiff`] describing the resolved end state.
/// Kept structurally parallel to the live op so plan and live mv stay
/// byte-equivalent in their accept/reject behaviour.
fn project_mv(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    src: &Path,
    dst: &Path,
    force: bool,
) -> Result<MvDiff> {
    use crate::writer::ensure_not_forbidden_target;

    ensure_not_forbidden_target(src)?;
    ensure_not_forbidden_target(dst)?;

    let (src_lexical, dst_lexical) = mv_lexical_paths(base_dir, src, dst);
    let src_is_dir = system.is_dir(&src_lexical).unwrap_or(false);
    let dst_is_existing_dir = system.is_dir(&dst_lexical).unwrap_or(false);

    check_mv_dst_directory_ambiguity(src_is_dir, dst_is_existing_dir, dst)?;

    let src_exists = system.exists(&src_lexical).unwrap_or(false);
    let (src_resolved, dst_resolved) =
        resolve_mv_paths(system, base_dir, cfg, src, dst, src_exists)?;
    ensure_not_forbidden_target(&dst_resolved)?;

    let dst_exists = system.exists(&dst_resolved).unwrap_or(false);
    let noop_same_path = src_exists && src_resolved == dst_resolved;
    let idempotent_already_settled = !src_exists && dst_exists;

    if !src_exists && !dst_exists {
        anyhow::bail!(
            "source not found: {} (and destination does not exist either)",
            src.display()
        );
    }

    run_mv_pre_mutate_checks(system, cfg, &src_resolved, &dst_resolved, src_exists)?;
    let already_settled = noop_same_path || idempotent_already_settled;
    check_mv_dst_overwrite(dst_exists, already_settled, force, dst)?;

    let nested_files_moved = if src_is_dir && src_exists {
        mv_directory_size(system, &src_resolved)
    } else {
        0
    };

    Ok(MvDiff {
        dst_absolute: dst_resolved,
        existence: MvExistence {
            dst_exists,
            is_directory: src_is_dir && src_exists,
            src_exists,
        },
        nested_files_moved,
        src_absolute: if src_exists {
            src_resolved
        } else {
            src_lexical
        },
        state: MvState {
            idempotent_already_settled,
            noop_same_path,
        },
    })
}

fn mv_lexical_paths(base_dir: &Path, src: &Path, dst: &Path) -> (PathBuf, PathBuf) {
    let src_lexical = if src.is_absolute() {
        src.to_path_buf()
    } else {
        base_dir.join(src)
    };
    let dst_lexical = if dst.is_absolute() {
        dst.to_path_buf()
    } else {
        base_dir.join(dst)
    };
    (src_lexical, dst_lexical)
}

/// Same gate as the live op: file → existing-directory dst is
/// ambiguous (caller probably meant `dst/<basename>`). Directory
/// source → existing-directory dst is the overwrite-with-force case
/// handled by [`check_mv_dst_overwrite`], so do not reject here.
fn check_mv_dst_directory_ambiguity(
    src_is_dir: bool,
    dst_is_existing_dir: bool,
    dst: &Path,
) -> Result<()> {
    if !src_is_dir && dst_is_existing_dir {
        anyhow::bail!(
            "destination is a directory: {} (this op moves a single file; pass an explicit destination path)",
            dst.display()
        );
    }
    Ok(())
}

fn resolve_mv_paths(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    src: &Path,
    dst: &Path,
    src_exists: bool,
) -> Result<(PathBuf, PathBuf)> {
    let src_resolved = if src_exists {
        allowlist::resolve_sandboxed(system, base_dir, src, cfg.unrestricted, &cfg.trusted_roots)?
    } else {
        allowlist::resolve_sandboxed_create(
            system,
            base_dir,
            src,
            cfg.unrestricted,
            &cfg.trusted_roots,
        )?
    };
    let dst_resolved = allowlist::resolve_sandboxed_create(
        system,
        base_dir,
        dst,
        cfg.unrestricted,
        &cfg.trusted_roots,
    )?;
    Ok((src_resolved, dst_resolved))
}

fn run_mv_pre_mutate_checks(
    system: &dyn System,
    cfg: &ResolvedConfig,
    src_resolved: &Path,
    dst_resolved: &Path,
    src_exists: bool,
) -> Result<()> {
    use crate::permissions::op_guard::pre_mutate_check_for_caller;
    let caller = cfg.caller_info();
    if src_exists {
        pre_mutate_check_for_caller(system, "mv", src_resolved, &caller)?;
    }
    pre_mutate_check_for_caller(system, "mv", dst_resolved, &caller)
}

fn check_mv_dst_overwrite(
    dst_exists: bool,
    already_settled: bool,
    force: bool,
    dst: &Path,
) -> Result<()> {
    if dst_exists && !already_settled && !force {
        anyhow::bail!(
            "destination exists: {} (pass --force to overwrite)",
            dst.display()
        );
    }
    Ok(())
}

/// Count nested regular files under `dir` for the plan projection's
/// `nested_files_moved` field. Best-effort — failures are reported as
/// `0` so the projection still carries a populated [`MvDiff`].
fn mv_directory_size(system: &dyn System, dir: &Path) -> usize {
    system
        .walk_dir(dir, false, false)
        .map_or(0, |entries| entries.iter().filter(|e| e.is_file).count())
}

/// Build a [`PlanReport`] for the recursive `purge` op.
///
/// Pure: no disk writes. Walks the directory the same way
/// [`crate::operations::purge::purge_dir`] does, runs the same per-file
/// `op_guard` / parser preflight on each candidate, and returns a
/// [`PurgeDirDiff`] enumerating projected per-file outcomes. The
/// document-level fields stay empty — the directory case has no single
/// before/after pair to diff.
///
/// `would_commit` is `true` whenever the walk itself succeeds; per-file
/// refusals are advisory and live in
/// [`PurgeDirFileDiff::reject_reason`]. A walk-level failure (missing
/// directory, I/O error) propagates as `Err` and is rendered as a
/// rejected report by the caller.
fn dispatch_purge_dir(
    system: &dyn System,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
    dir: &Path,
) -> Result<PlanReport> {
    let empty = parser::parse("").context("parsing empty before-document for plan purge")?;
    let mut report = project_report("purge", &empty, &empty, cfg, identity)?;

    if !system.exists(dir).unwrap_or(false) {
        report.reject_reason = Some(format!("directory does not exist: {}", dir.display()));
        report.would_commit = false;
        return Ok(report);
    }
    if !system.is_dir(dir).unwrap_or(false) {
        report.reject_reason = Some(format!("not a directory: {}", dir.display()));
        report.would_commit = false;
        return Ok(report);
    }

    let entries = system
        .walk_dir(dir, false, false)
        .with_context(|| format!("walking directory {}", dir.display()))?;

    // Reuse the same candidate filter the live op uses so plan and
    // apply enumerate exactly the same set.
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in &entries {
        if !entry.is_file {
            continue;
        }
        if path_has_dot_component_under_plan(&entry.path, dir) {
            continue;
        }
        if !allowlist::is_visible(&entry.path, false) {
            continue;
        }
        let is_md = entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !is_md {
            continue;
        }
        candidates.push(entry.path.clone());
    }
    candidates.sort();

    let mut files: Vec<PurgeDirFileDiff> = Vec::with_capacity(candidates.len());
    let mut any_would_purge = false;
    for path in &candidates {
        let projection = project_one_purge_file(system, cfg, path);
        if matches!(projection.outcome, PurgeDirFileOutcome::WouldPurge) {
            any_would_purge = true;
        }
        files.push(projection);
    }

    let no_md_files = candidates.is_empty();
    report.noop = !any_would_purge;
    report.would_commit = true;
    report.purge_dir_diff = Some(PurgeDirDiff {
        directory: dir.to_path_buf(),
        files,
        no_md_files,
    });
    Ok(report)
}

/// `true` when `path` has any path component (relative to `root`)
/// whose name starts with `.`. Mirrors the helper in
/// `operations::purge` so plan + apply enumerate the same files.
fn path_has_dot_component_under_plan(path: &Path, root: &Path) -> bool {
    let suffix = path.strip_prefix(root).unwrap_or(path);
    suffix.components().any(|c| {
        if let Component::Normal(part) = c {
            part.to_str().is_some_and(|s| s.starts_with('.'))
        } else {
            false
        }
    })
}

/// Project one file's purge outcome for the recursive plan dispatch.
///
/// Mirrors `purge_one_for_bulk` in the live op: runs the per-file
/// `op_guard` preflight (refusal -> `Refused`), parses the file, counts
/// the comments that would be removed, and decides between
/// `WouldPurge` (>0 comments) and `Skipped` (no comments to strip).
fn project_one_purge_file(
    system: &dyn System,
    cfg: &ResolvedConfig,
    path: &Path,
) -> PurgeDirFileDiff {
    use crate::permissions::op_guard::pre_mutate_check_for_caller;
    use crate::writer::ensure_not_forbidden_target;

    let preflight = (|| -> Result<()> {
        ensure_not_forbidden_target(path)?;
        pre_mutate_check_for_caller(system, "purge", path, &cfg.caller_info())?;
        Ok(())
    })();

    if let Err(err) = preflight {
        return PurgeDirFileDiff {
            attachments_cleaned: 0,
            comments_removed: 0,
            outcome: PurgeDirFileOutcome::Refused,
            path: path.to_path_buf(),
            reject_reason: Some(format!("{err:#}")),
        };
    }

    let parsed = match parser::parse_file(system, path) {
        Ok(doc) => doc,
        Err(err) => {
            return PurgeDirFileDiff {
                attachments_cleaned: 0,
                comments_removed: 0,
                outcome: PurgeDirFileOutcome::Refused,
                path: path.to_path_buf(),
                reject_reason: Some(format!("{err:#}")),
            };
        }
    };

    let comments = parsed.comments();
    let comments_removed = comments.len();
    let attachments_cleaned = comments.iter().map(|cm| cm.attachments.len()).sum();

    if comments_removed == 0 {
        PurgeDirFileDiff {
            attachments_cleaned: 0,
            comments_removed: 0,
            outcome: PurgeDirFileOutcome::Skipped,
            path: path.to_path_buf(),
            reject_reason: None,
        }
    } else {
        PurgeDirFileDiff {
            attachments_cleaned,
            comments_removed,
            outcome: PurgeDirFileOutcome::WouldPurge,
            path: path.to_path_buf(),
            reject_reason: None,
        }
    }
}

/// Decide whether an [`UnprotectConfigDiff`] amounts to a noop. True
/// when both the YAML entry and the sidecar entry are absent AND no
/// rule would be removed from any settings file. Conflicts do not
/// flip the noop verdict — they're advisory.
fn is_unprotect_diff_noop(diff: &UnprotectConfigDiff) -> bool {
    let yaml_noop = matches!(
        diff.remargin_yaml.entry_action,
        UnprotectEntryAction::Absent
    );
    let sidecar_noop = matches!(diff.sidecar.entry_action, UnprotectEntryAction::Absent);
    let settings_noop = diff
        .settings_files
        .iter()
        .all(|sf| sf.rules_to_remove.is_empty());
    yaml_noop && sidecar_noop && settings_noop
}

/// Decide whether a [`ConfigPlanDiff`] amounts to a noop. True when
/// every per-file projection reports `entry_action == Noop` and no
/// rule would be added to any settings file. Conflicts do not flip
/// the noop verdict — they're advisory.
fn is_diff_noop(diff: &ConfigPlanDiff) -> bool {
    let yaml_noop = matches!(diff.remargin_yaml.entry_action, EntryAction::Noop);
    let sidecar_noop = matches!(diff.sidecar.entry_action, EntryAction::Noop);
    let settings_noop = diff.settings_files.iter().all(|sf| {
        sf.allow_rules_to_add.is_empty() && sf.deny_rules_to_add.is_empty() && !sf.will_be_created
    });
    yaml_noop && sidecar_noop && settings_noop
}

/// Convert a [`WriteProjection`] into a [`PlanReport`]. Shared by every
/// caller so the Markdown / Unsupported handling does not drift between
/// adapters.
fn dispatch_write_projection(
    system: &dyn System,
    path: &Path,
    projection: &WriteProjection,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
) -> Result<PlanReport> {
    match projection {
        WriteProjection::Markdown {
            before,
            after,
            noop,
        } => {
            let mut report =
                project_doc_report(system, path, "write", before, after, cfg, identity)?;
            report.noop = report.noop || *noop;
            Ok(report)
        }
        WriteProjection::Unsupported { reason } => {
            let empty =
                parser::parse("").context("parsing empty before-document for plan write")?;
            let mut report = project_report("write", &empty, &empty, cfg, identity)?;
            report.reject_reason = Some(reason.clone());
            report.would_commit = false;
            Ok(report)
        }
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

/// Mirror the subset gate from [`commit_with_verify`]. Plan flips
/// `would_commit` to false iff Q ⊄ P — the projected mutation would
/// introduce a new anomaly not present in the pre-state.
///
/// `subset_gate_path`, when provided, produces a structured
/// [`PlanSubsetGate`] that mirrors
/// [`crate::operations::verify::SubsetGateFailure`].
fn decide_commit(
    before: &ParsedDocument,
    after: &ParsedDocument,
    cfg: &ResolvedConfig,
    subset_gate_path: Option<PathBuf>,
) -> (bool, Option<String>, Option<PlanSubsetGate>) {
    let pre = anomalies_for_doc(before, cfg);
    let post = anomalies_for_doc(after, cfg);
    let mut introduced: Vec<Anomaly> = post.difference(&pre).cloned().collect();
    if introduced.is_empty() {
        return (true, None, None);
    }
    introduced.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.kind.as_str().cmp(b.kind.as_str()))
    });
    let mut reason = format!("op would introduce {} new anomal", introduced.len());
    if introduced.len() == 1 {
        reason.push('y');
    } else {
        reason.push_str("ies");
    }
    reason.push_str(" under mode ");
    reason.push_str(cfg.mode.as_str());
    reason.push(':');
    for a in &introduced {
        let _ = write!(reason, " {}:{};", a.id, a.kind.as_str());
    }
    let subset_gate =
        subset_gate_path.map(|p| PlanSubsetGate::from_introduced(introduced, &cfg.mode, p));
    (false, Some(reason), subset_gate)
}

const fn entry_action_label(action: EntryAction) -> &'static str {
    match action {
        EntryAction::Added => "added",
        EntryAction::Noop => "noop",
        EntryAction::Updated => "updated",
    }
}

const fn unprotect_entry_action_label(action: UnprotectEntryAction) -> &'static str {
    match action {
        UnprotectEntryAction::Absent => "absent",
        UnprotectEntryAction::WouldBeRemoved => "would_be_removed",
    }
}

/// Render a `plan restrict` [`PlanReport`] as a structured text block.
///
/// Returns an empty string when the report has no `config_diff`.
#[must_use]
pub fn render_plan_restrict_text(report: &PlanReport) -> String {
    let Some(diff) = report.config_diff.as_ref() else {
        return String::new();
    };
    let mut out = String::new();
    let _ = writeln!(out, "Plan: restrict {}", diff.absolute_path.display());
    let _ = writeln!(out, "  Anchor: {}", diff.anchor.display());
    let _ = writeln!(
        out,
        "  noop: {}   would_commit: {}",
        report.noop, report.would_commit,
    );
    if let Some(reason) = &report.reject_reason {
        let _ = writeln!(out, "  reject_reason: {reason}");
    }
    let _ = writeln!(
        out,
        "  .remargin.yaml: {}",
        diff.remargin_yaml.path.display()
    );
    let _ = writeln!(
        out,
        "    will be created: {}",
        diff.remargin_yaml.will_be_created,
    );
    let _ = writeln!(
        out,
        "    entry: {}",
        entry_action_label(diff.remargin_yaml.entry_action),
    );
    let _ = writeln!(out, "  Settings: {} file(s)", diff.settings_files.len());
    for sf in &diff.settings_files {
        let _ = writeln!(out, "    {}", sf.path.display());
        let _ = writeln!(out, "      will be created: {}", sf.will_be_created);
        let _ = writeln!(
            out,
            "      deny rules: +{} to add, {} already present",
            sf.deny_rules_to_add.len(),
            sf.deny_rules_already_present.len(),
        );
        let _ = writeln!(
            out,
            "      allow rules: +{} to add, {} already present",
            sf.allow_rules_to_add.len(),
            sf.allow_rules_already_present.len(),
        );
    }
    let _ = writeln!(
        out,
        "  Sidecar: {} ({})",
        diff.sidecar.path.display(),
        entry_action_label(diff.sidecar.entry_action),
    );
    if diff.conflicts.is_empty() {
        let _ = writeln!(out, "  conflicts: 0");
    } else {
        let _ = writeln!(out, "  conflicts: {}", diff.conflicts.len());
        for conflict in &diff.conflicts {
            out.push_str(&render_conflict_line(conflict));
        }
    }
    out
}

/// Render a single [`ConfigConflict`] as an indented text line.
#[must_use]
pub fn render_conflict_line(conflict: &ConfigConflict) -> String {
    let mut out = String::new();
    match conflict {
        ConfigConflict::AllowDenyOverlap {
            allow_rule,
            overlap_kind,
            projected_deny_rule,
            settings_file,
        } => {
            let kind_label = match overlap_kind {
                OverlapKind::AllowShadowedByBroaderDeny => {
                    "existing allow is shadowed by broader projected deny"
                }
                OverlapKind::DenyShadowedByBroaderAllow => {
                    "projected deny is shadowed by broader existing allow"
                }
                OverlapKind::Exact => "exact",
            };
            let _ = writeln!(
                out,
                "    allow_deny_overlap in {} ({kind_label}):",
                settings_file.display(),
            );
            let _ = writeln!(out, "      existing allow:  {allow_rule}");
            let _ = writeln!(out, "      projected deny:  {projected_deny_rule}");
        }
        ConfigConflict::AnchorIsAncestor { anchor, cwd } => {
            let _ = writeln!(
                out,
                "    anchor_is_ancestor: cwd={} anchor={}",
                cwd.display(),
                anchor.display(),
            );
        }
        ConfigConflict::YamlEntryWouldChange {
            path,
            previous,
            projected,
        } => {
            use crate::display::format_string_list;
            let _ = writeln!(out, "    yaml_entry_would_change: path={path}");
            let _ = writeln!(
                out,
                "      previous: also_deny_bash={} cli_allowed={}",
                format_string_list(&previous.also_deny_bash),
                previous.cli_allowed,
            );
            let _ = writeln!(
                out,
                "      projected: also_deny_bash={} cli_allowed={}",
                format_string_list(&projected.also_deny_bash),
                projected.cli_allowed,
            );
        }
    }
    out
}

/// Render a `plan unprotect` [`PlanReport`] as a structured text block.
///
/// Returns an empty string when the report has no `unprotect_diff`.
#[must_use]
pub fn render_plan_unprotect_text(report: &PlanReport) -> String {
    let Some(diff) = report.unprotect_diff.as_ref() else {
        return String::new();
    };
    let mut out = String::new();
    let _ = writeln!(out, "Plan: unprotect {}", diff.absolute_path.display());
    let _ = writeln!(out, "  Anchor: {}", diff.anchor.display());
    let _ = writeln!(
        out,
        "  noop: {}   would_commit: {}",
        report.noop, report.would_commit,
    );
    if let Some(reason) = &report.reject_reason {
        let _ = writeln!(out, "  reject_reason: {reason}");
    }
    let _ = writeln!(
        out,
        "  .remargin.yaml: {}",
        diff.remargin_yaml.path.display()
    );
    let _ = writeln!(
        out,
        "    entry: {}",
        unprotect_entry_action_label(diff.remargin_yaml.entry_action),
    );
    let _ = writeln!(out, "  Settings: {} file(s)", diff.settings_files.len());
    for sf in &diff.settings_files {
        let _ = writeln!(out, "    {}", sf.path.display());
        let _ = writeln!(
            out,
            "      rules: -{} to remove, {} already absent",
            sf.rules_to_remove.len(),
            sf.rules_already_absent.len(),
        );
    }
    let _ = writeln!(
        out,
        "  Sidecar: {} ({})",
        diff.sidecar.path.display(),
        unprotect_entry_action_label(diff.sidecar.entry_action),
    );
    if diff.conflicts.is_empty() {
        let _ = writeln!(out, "  conflicts: 0");
    } else {
        let _ = writeln!(out, "  conflicts: {}", diff.conflicts.len());
        for conflict in &diff.conflicts {
            out.push_str(&render_unprotect_conflict_line(conflict));
        }
    }
    out
}

/// Render a single [`UnprotectConflict`] as an indented text line.
#[must_use]
pub fn render_unprotect_conflict_line(conflict: &UnprotectConflict) -> String {
    let mut out = String::new();
    match conflict {
        UnprotectConflict::RuleAlreadyAbsent {
            rule,
            settings_file,
        } => {
            let _ = writeln!(
                out,
                "    rule_already_absent in {}: {rule}",
                settings_file.display(),
            );
        }
        UnprotectConflict::SidecarEntryMissing { path } => {
            let _ = writeln!(out, "    sidecar_entry_missing: {}", path.display());
        }
        UnprotectConflict::YamlEntryMissing { path } => {
            let _ = writeln!(out, "    yaml_entry_missing: {}", path.display());
        }
    }
    out
}

#[cfg(test)]
mod tests;
