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
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::config::ResolvedConfig;
use crate::document::{self, WriteOptions, WriteProjection};
use crate::operations::migrate::MigrateIdentities;
use crate::operations::projections::{self, ProjectBatchOp, ProjectCommentParams};
use crate::operations::sign::SignSelection;
use crate::operations::verify::{VerifyReport, verify_document};
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
    /// Author type (`human` / `agent`) as resolved for the active op —
    /// whichever branch of `identity::resolve_identity` the CLI / MCP
    /// flags selected.
    pub author_type: Option<String>,
    /// Author identity as resolved for the active op — whichever branch
    /// of `identity::resolve_identity` the CLI / MCP flags selected.
    pub name: Option<String>,
    /// Whether the configured private key would load and sign successfully
    /// for this op. `false` means the op would still commit under the
    /// active mode, but without a signature attached.
    pub would_sign: bool,
}

impl PlanIdentity {
    /// Canonical builder shared by every adapter (CLI + MCP).
    ///
    /// `would_sign` is `true` when a key path is configured. The key is
    /// not loaded here — `plan` stays side-effect-free per rem-bhk. Both
    /// adapters must use this constructor so plan reports are byte-
    /// identical across surfaces (rem-3a2).
    #[must_use]
    pub fn from_config(cfg: &ResolvedConfig) -> Self {
        let author_type = cfg.author_type.as_ref().map(|t| String::from(t.as_str()));
        Self::new(cfg.identity.clone(), author_type, cfg.key_path.is_some())
    }

    /// Build a [`PlanIdentity`] from the three fields documented in
    /// rem-bhk. The constructor exists so external crates can populate
    /// the struct without tripping `#[non_exhaustive]`.
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
    /// Config-mutation projection for the `restrict` op (rem-puy5).
    /// `None` for every Markdown op (the document-level fields above
    /// describe those) AND for `unprotect` (which carries a typed
    /// reverse projection in [`PlanReport::unprotect_diff`] instead).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_diff: Option<ConfigPlanDiff>,
    /// Which identity the plan was computed under. `would_sign` reports
    /// whether signing would succeed without actually invoking the key.
    pub identity: PlanIdentity,
    /// File-relocation projection emitted by the `plan mv` op
    /// (rem-0j2x / T44). `None` for every other op. The document-level
    /// fields (`comments`, `changed_line_ranges`, `checksum_*`,
    /// `verify_after`) are vacuously empty for `mv` — the bytes do not
    /// change, only the file's location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mv_diff: Option<MvDiff>,
    /// `true` when the projected content is byte-identical to the source
    /// content (`checksum_before == checksum_after`).
    pub noop: bool,
    /// The mutating op label (`write`, `comment`, `ack`, `batch`, ...).
    pub op: String,
    /// Human-readable reason when `would_commit` is `false`. `None` when
    /// the projection would commit cleanly. Specific enough to act on
    /// (which comment, which invariant).
    pub reject_reason: Option<String>,
    /// Reverse projection emitted by the `plan unprotect` op. Names
    /// every file the live `unprotect` would touch (`.remargin.yaml`,
    /// project + user settings, sidecar) plus every detectable
    /// drift conflict (manual edits, missing entries). `None` for
    /// every other op — restrict carries its own forward projection
    /// in [`PlanReport::config_diff`], document ops use the
    /// document-level fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unprotect_diff: Option<UnprotectConfigDiff>,
    /// Full post-op verify report computed against the projected
    /// document under the active mode.
    pub verify_after: PlanVerifyReport,
    /// Aggregate verdict: `true` when the op would land successfully
    /// under the current mode and invariants.
    pub would_commit: bool,
}

/// Per-file projection emitted by the `plan restrict` op (rem-puy5).
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
    /// Canonical absolute restricted path. For the wildcard form,
    /// this is the anchor root.
    pub absolute_path: PathBuf,
    /// `.claude/`-bearing ancestor that anchors the write.
    pub anchor: PathBuf,
    /// Detected conflicts. Empty when the projection is clean.
    /// Conflicts are advisory: `would_commit` stays `true` so the
    /// caller can apply anyway with full information.
    pub conflicts: Vec<ConfigConflict>,
    /// What would happen to `<anchor>/.remargin.yaml`.
    pub remargin_yaml: RemarginYamlDiff,
    /// One entry per settings file the synchronizer would touch
    /// (project-scope first, user-scope second when both are passed).
    pub settings_files: Vec<SettingsFileDiff>,
    /// Sidecar projection.
    pub sidecar: SidecarDiff,
}

/// Projection of the `<anchor>/.remargin.yaml` write performed by
/// `restrict`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct RemarginYamlDiff {
    /// What the projection would do to the `permissions.restrict`
    /// entry list: append, overwrite the existing entry for this
    /// path, or report a noop because the existing entry already
    /// matches.
    pub entry_action: EntryAction,
    /// Resolved on-disk path of the YAML file.
    pub path: PathBuf,
    /// On-disk entry that would be replaced. `None` when no existing
    /// entry matches the projected path. Always populated when
    /// `entry_action == Updated` so the user can see the full delta;
    /// also populated when `entry_action == Noop` to make the
    /// "matches existing" case unambiguous.
    pub previous_entry: Option<RestrictEntryProjection>,
    /// Entry that would be written into
    /// `permissions.restrict`. `None` only on the noop path when there
    /// is somehow no projected entry to record.
    pub projected_entry: Option<RestrictEntryProjection>,
    /// `true` when the YAML file does not exist on disk.
    pub will_be_created: bool,
}

/// Projection of one Claude settings file
/// (`.claude/settings.local.json` for the project scope or
/// `~/.claude/settings.json` for the user scope) that
/// `restrict` would write into.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SettingsFileDiff {
    /// Allow rules already in `permissions.allow` — the synchronizer
    /// would skip these.
    pub allow_rules_already_present: Vec<String>,
    /// Allow rules the synchronizer would append to
    /// `permissions.allow`.
    pub allow_rules_to_add: Vec<String>,
    /// Deny rules already in `permissions.deny` — the synchronizer
    /// would skip these.
    pub deny_rules_already_present: Vec<String>,
    /// Deny rules the synchronizer would append to
    /// `permissions.deny`.
    pub deny_rules_to_add: Vec<String>,
    /// Resolved on-disk path of the settings file.
    pub path: PathBuf,
    /// `true` when the file does not currently exist (the synchronizer
    /// would create it).
    pub will_be_created: bool,
}

/// Projection of `<anchor>/.claude/.remargin-restrictions.json`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SidecarDiff {
    /// What the projection would do to the entry under
    /// `entries[<absolute_path>]`: append, replace the existing entry,
    /// or noop because the existing entry already matches.
    pub entry_action: EntryAction,
    /// Resolved on-disk path of the sidecar.
    pub path: PathBuf,
    /// `true` when the sidecar file does not currently exist.
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
    /// An existing rule in `permissions.allow` overlaps a rule the
    /// projection would add to `permissions.deny` in the same settings
    /// file. The motivating bug for rem-puy5 — Claude's settings
    /// semantics resolve allow-vs-deny in ways the user may not
    /// expect, so the conflict is surfaced for review before
    /// committing.
    ///
    /// `overlap_kind` distinguishes exact matches from prefix /
    /// subtree overlap (rem-aovx) so callers can tailor the message.
    AllowDenyOverlap {
        /// Existing allow rule string.
        allow_rule: String,
        /// Relationship between the existing allow and the projected
        /// deny.
        overlap_kind: OverlapKind,
        /// Projected deny rule string.
        projected_deny_rule: String,
        /// Settings file the conflict was detected in.
        settings_file: PathBuf,
    },
    /// `find_claude_anchor` walked above the caller's `cwd` to find a
    /// `.claude/`-bearing ancestor. Surfaced because realm boundaries
    /// have surprised users in the past — the agent thought it was
    /// restricting `~/.local/realm/secret` but the anchor was actually
    /// `~/`.
    AnchorIsAncestor {
        /// Anchor `find_claude_anchor` resolved to.
        anchor: PathBuf,
        /// Caller's `cwd` (canonicalized).
        cwd: PathBuf,
    },
    /// `permissions.restrict` already has an entry for the same path
    /// but with different `also_deny_bash` / `cli_allowed`. Surfaced
    /// because the live op silently overwrites (rem-yj1j.5 / rem-aqnn).
    YamlEntryWouldChange {
        /// On-disk path of the existing entry.
        path: String,
        /// Snapshot of the existing entry.
        previous: RestrictEntryProjection,
        /// Snapshot of the entry the projection would write.
        projected: RestrictEntryProjection,
    },
}

/// What [`RemarginYamlDiff`] / [`SidecarDiff`] would do to its target
/// entry. Mirrors a write-versus-skip decision.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EntryAction {
    /// New entry would be appended.
    Added,
    /// Existing entry already matches the projection. No write.
    Noop,
    /// Existing entry would be replaced.
    Updated,
}

/// Reverse projection emitted by the `plan unprotect` op (rem-6eop / T43).
///
/// Symmetric mirror of [`ConfigPlanDiff`] for the reverse direction.
/// `unprotect` is the explicit, sanctioned reversal of a previous
/// `restrict`: it removes the matching `permissions.restrict` entry
/// from `<anchor>/.remargin.yaml`, scrubs the sidecar-tracked rules
/// from each Claude settings file the original `apply_rules` recorded,
/// and finally drops the sidecar entry. This struct names every file,
/// every entry that would be removed vs. left alone, and every
/// detectable drift conflict, so callers can preview the full
/// reversal before committing.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UnprotectConfigDiff {
    /// Canonical absolute path that would be unprotected. For the
    /// wildcard form, this is the anchor root.
    pub absolute_path: PathBuf,
    /// `.claude/`-bearing ancestor that anchors the reversal.
    pub anchor: PathBuf,
    /// Detected drift conflicts. Empty when the projection is clean.
    /// Conflicts are advisory: `would_commit` stays `true` so the
    /// caller can apply anyway with full information.
    pub conflicts: Vec<UnprotectConflict>,
    /// What would happen to `<anchor>/.remargin.yaml`.
    pub remargin_yaml: UnprotectYamlDiff,
    /// One entry per settings file the reversal would touch, sourced
    /// from the sidecar's `added_to_files` list. Empty when no
    /// sidecar entry exists for the target path.
    pub settings_files: Vec<UnprotectSettingsDiff>,
    /// Sidecar projection.
    pub sidecar: UnprotectSidecarDiff,
}

/// Projection of the `<anchor>/.remargin.yaml` write performed by
/// `unprotect` — mirror of [`RemarginYamlDiff`] for the reverse
/// direction.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UnprotectYamlDiff {
    /// What the projection would do to the matching
    /// `permissions.restrict` entry: remove it, or report no-op
    /// because no entry currently matches the path.
    pub entry_action: UnprotectEntryAction,
    /// Resolved on-disk path of the YAML file.
    pub path: PathBuf,
    /// On-disk entry that would be removed. `None` when no existing
    /// entry matches the projected path. Always populated when
    /// `entry_action == WouldBeRemoved`.
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
    /// Resolved on-disk path of the settings file. Sourced from the
    /// sidecar's `added_to_files` list.
    pub path: PathBuf,
    /// Rules the sidecar tracked but that are no longer present in
    /// the file (manual-edit drift). Each rule here also surfaces as
    /// a [`UnprotectConflict::RuleAlreadyAbsent`] conflict.
    pub rules_already_absent: Vec<String>,
    /// Rules the reversal would scrub from `permissions.allow` /
    /// `permissions.deny`.
    pub rules_to_remove: Vec<String>,
}

/// Projection of `<anchor>/.claude/.remargin-restrictions.json` —
/// mirror of [`SidecarDiff`] for the reverse direction.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UnprotectSidecarDiff {
    /// What the projection would do to the entry under
    /// `entries[<absolute_path>]`: remove it, or report no-op because
    /// no entry currently exists.
    pub entry_action: UnprotectEntryAction,
    /// Resolved on-disk path of the sidecar.
    pub path: PathBuf,
}

/// What [`UnprotectYamlDiff`] / [`UnprotectSidecarDiff`] would do to
/// its target entry. Mirrors a remove-versus-skip decision.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnprotectEntryAction {
    /// No matching entry exists; nothing would happen here.
    Absent,
    /// Existing entry would be removed.
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
    /// A rule the sidecar lists for `settings_file` is no longer
    /// present in that file (the user manually deleted it between
    /// `restrict` and `unprotect`). Surfaced per-rule per-file.
    RuleAlreadyAbsent {
        /// The rule string the sidecar expected to find.
        rule: String,
        /// Settings file the rule was supposed to live in.
        settings_file: PathBuf,
    },
    /// The sidecar has no entry for the target path. The YAML
    /// removal would still proceed but the Claude settings files
    /// won't be touched (the reversal cannot guess which rules to
    /// scrub).
    SidecarEntryMissing {
        /// Canonical absolute path the projection looked up in the
        /// sidecar.
        path: PathBuf,
    },
    /// The YAML file has no `permissions.restrict` entry matching
    /// the requested path. The sidecar removal would still proceed
    /// but `.remargin.yaml` won't be touched.
    YamlEntryMissing {
        /// Resolved on-disk path of the YAML file.
        path: PathBuf,
    },
}

/// File-relocation projection emitted by the `plan mv` op
/// (rem-0j2x / T44).
///
/// Mirrors the read-only side of [`crate::operations::mv::mv`]: names
/// the canonical src/dst, whether the destination already exists (and
/// would therefore require `--force` to overwrite), and whether the
/// call would be a same-path no-op or an idempotent re-run after a
/// previous successful move.
///
/// The four boolean fields are each surfaced in the documented JSON
/// shape (`dst_exists`, `idempotent_already_settled`, `noop_same_path`,
/// `src_exists`); collapsing them would lose API surface.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a documented JSON output field (rem-0j2x)"
)]
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MvDiff {
    /// Canonical absolute destination path. Matches
    /// [`crate::operations::mv::MvOutcome::dst_absolute`].
    pub dst_absolute: PathBuf,
    /// `true` when the destination currently exists. The live op
    /// requires `--force` (or the equivalent MCP flag) to overwrite.
    pub dst_exists: bool,
    /// `true` when the source is missing and the destination already
    /// exists at the requested path. The live op would settle as a
    /// `bytes_moved = 0` success.
    pub idempotent_already_settled: bool,
    /// `true` when src and dst resolve to the same canonical path.
    pub noop_same_path: bool,
    /// Canonical absolute source path. When the source is missing
    /// this is the lexical join of `base_dir` + the requested path.
    pub src_absolute: PathBuf,
    /// `true` when the source path resolves to an existing file.
    pub src_exists: bool,
}

/// A `plan` request for a single mutating op, normalized so CLI + MCP
/// can share one dispatch path (rem-oqv / rem-3a2).
///
/// Each variant mirrors one mutating op. Adapters construct the variant
/// from their native input shape; [`dispatch`] converts it to a
/// [`PlanReport`]. Adapters must not re-implement the per-op projection
/// wiring; when a new plan op lands, extend this enum and [`dispatch`]
/// once — both surfaces pick up the change automatically.
#[non_exhaustive]
pub enum PlanRequest<'req> {
    /// `plan ack` — projects the ack/unack of one or more comments.
    Ack {
        /// Document path (already joined against the base dir / cwd).
        path: PathBuf,
        /// Comment ids to ack / unack.
        ids: Vec<String>,
        /// `true` to remove this identity's ack; `false` to add one.
        remove: bool,
    },
    /// `plan batch` — projects atomic creation of multiple comments.
    Batch {
        path: PathBuf,
        ops: Vec<ProjectBatchOp>,
    },
    /// `plan comment` — projects creating a single comment.
    Comment {
        path: PathBuf,
        params: ProjectCommentParams<'req>,
    },
    /// `plan delete` — projects deletion of one or more comments.
    Delete { path: PathBuf, ids: Vec<String> },
    /// `plan edit` — projects editing a comment's content.
    Edit {
        path: PathBuf,
        id: &'req str,
        content: &'req str,
    },
    /// `plan migrate` — projects conversion of legacy comments.
    Migrate {
        path: PathBuf,
        /// Per-role identities (and signing keys) used by both the
        /// projection and the real op. Pass
        /// `MigrateIdentities::default()` to keep the historical
        /// `legacy-user` / `legacy-agent` placeholder behaviour.
        identities: MigrateIdentities,
    },
    /// `plan mv` — projects a file relocation (rem-0j2x / T44).
    /// Produces an [`MvDiff`] in [`PlanReport::mv_diff`] describing
    /// the resolved src/dst paths, whether the destination already
    /// exists, and whether the live op would settle as a no-op.
    Mv {
        /// Source path as supplied by the caller (relative to
        /// `base_dir` or absolute). Resolution mirrors
        /// [`crate::operations::mv::mv`].
        src: PathBuf,
        /// Destination path as supplied by the caller.
        dst: PathBuf,
        /// `true` to project the `--force` semantics (the live op
        /// would overwrite an existing destination).
        force: bool,
    },
    /// `plan purge` — projects removal of all comments.
    Purge { path: PathBuf },
    /// `plan react` — projects add/remove of an emoji reaction.
    React {
        path: PathBuf,
        id: &'req str,
        emoji: &'req str,
        /// `true` to remove the reaction; `false` to add.
        remove: bool,
    },
    /// `plan restrict` — projects a config-mutation `restrict` op
    /// (rem-puy5). Unlike the document plans above, this variant
    /// produces a [`ConfigPlanDiff`] in [`PlanReport::config_diff`]
    /// describing every file the live op would touch.
    Restrict {
        /// Caller's working directory; used for anchor discovery.
        cwd: PathBuf,
        /// Restrict args (`path`, `also_deny_bash`, `cli_allowed`).
        args: RestrictArgs,
        /// Settings files the synchronizer would write into. Adapters
        /// resolve project + user scope before dispatch.
        settings_files: Vec<PathBuf>,
    },
    /// `plan sandbox-add` — projects staging the file in the caller's sandbox.
    SandboxAdd { path: PathBuf },
    /// `plan sandbox-remove` — projects unstaging the file from the caller's sandbox.
    SandboxRemove { path: PathBuf },
    /// `plan sign` — projects back-signing missing-signature comments
    /// authored by the current identity (rem-7y3). Unlike most plan ops,
    /// this loads the configured signing key and attaches real
    /// signatures to the projected `after` document so `verify_after`
    /// can faithfully predict the post-op gate.
    Sign {
        path: PathBuf,
        /// Which comments to consider. `Ids` rejections (unknown id,
        /// forgery guard) and `AllMine` filtering match the mutating
        /// `sign` op.
        selection: SignSelection,
    },
    /// `plan unprotect` — projects a config-mutation `unprotect` op
    /// (rem-6eop). Symmetric mirror of [`PlanRequest::Restrict`] for
    /// the reverse direction. Produces an [`UnprotectConfigDiff`] in
    /// [`PlanReport::unprotect_diff`] describing every file the live
    /// op would touch and every drift conflict it would surface.
    Unprotect {
        /// Caller's working directory; used for anchor discovery.
        cwd: PathBuf,
        /// Unprotect args (`path`, `strict`).
        args: UnprotectArgs,
    },
    /// `plan write` — projects a whole-file / partial-range write.
    ///
    /// `path` is passed as-is to [`document::project_write`] so the
    /// allowlist / partial-range / create-new-file semantics land in
    /// exactly one place.
    Write {
        /// The path relative to `base_dir`, exactly as the adapter
        /// received it.
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
            Self::Ack { .. } => "ack",
            Self::Batch { .. } => "batch",
            Self::Comment { .. } => "comment",
            Self::Delete { .. } => "delete",
            Self::Edit { .. } => "edit",
            Self::Migrate { .. } => "migrate",
            Self::Mv { .. } => "mv",
            Self::Purge { .. } => "purge",
            Self::React { .. } => "react",
            Self::Restrict { .. } => "restrict",
            Self::SandboxAdd { .. } => "sandbox-add",
            Self::SandboxRemove { .. } => "sandbox-remove",
            Self::Sign { .. } => "sign",
            Self::Unprotect { .. } => "unprotect",
            Self::Write { .. } => "write",
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
        config_diff: None,
        identity,
        mv_diff: None,
        noop,
        op: String::from(op_label),
        reject_reason,
        unprotect_diff: None,
        verify_after,
        would_commit,
    }
}

/// Canonical plan dispatcher shared by CLI + MCP (rem-oqv / rem-3a2).
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
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Batch { path, ops } => {
            let (before, after) = projections::project_batch(system, path, cfg, ops)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Comment { path, params } => {
            let (before, after) = projections::project_comment(system, path, cfg, params)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Delete { path, ids } => {
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let (before, after) = projections::project_delete(system, path, cfg, &id_refs)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Edit { path, id, content } => {
            let (before, after) = projections::project_edit(system, path, cfg, id, content)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Migrate { path, identities } => {
            let (before, after) = projections::project_migrate(system, path, cfg, identities)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Mv { src, dst, force } => {
            dispatch_mv(system, base_dir, cfg, identity, src, dst, *force)
        }
        PlanRequest::Purge { path } => {
            let (before, after) = projections::project_purge(system, path, cfg)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::React {
            path,
            id,
            emoji,
            remove,
        } => {
            let (before, after) =
                projections::project_react(system, path, cfg, id, emoji, *remove)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Restrict {
            cwd,
            args,
            settings_files,
        } => dispatch_restrict(system, cfg, identity, cwd, args, settings_files),
        PlanRequest::SandboxAdd { path } => {
            let (before, after) = projections::project_sandbox_add(system, path, cfg)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::SandboxRemove { path } => {
            let (before, after) = projections::project_sandbox_remove(system, path, cfg)?;
            Ok(project_report(label, &before, &after, cfg, identity))
        }
        PlanRequest::Sign { path, selection } => {
            let (before, after) = projections::project_sign(system, path, cfg, selection)?;
            Ok(project_report(label, &before, &after, cfg, identity))
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
            dispatch_write_projection(&projection, cfg, identity)
        }
    }
}

/// Build a [`PlanReport`] from the `restrict` projection's verdict
/// (rem-puy5). Mirrors [`dispatch_write_projection`]'s handling of the
/// document `Unsupported` arm: a hard reject from
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
    let mut report = project_report("restrict", &empty, &empty, cfg, identity);
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

/// Build a [`PlanReport`] from the `unprotect` projection's verdict
/// (rem-6eop). Symmetric mirror of [`dispatch_restrict`] for the
/// reverse direction: a hard reject from
/// [`projections::project_unprotect`] flips `would_commit` to false
/// and surfaces the carried reason verbatim.
fn dispatch_unprotect(
    system: &dyn System,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
    cwd: &Path,
    args: &UnprotectArgs,
) -> Result<PlanReport> {
    let projection = projections::unprotect::project_unprotect(system, cwd, args)?;
    let empty = parser::parse("").context("parsing empty before-document for plan unprotect")?;
    let mut report = project_report("unprotect", &empty, &empty, cfg, identity);
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

/// Build a [`PlanReport`] for the `mv` op (rem-0j2x / T44).
///
/// Pure: no disk writes, no identity load. Resolves both endpoints
/// through the same sandbox boundary the live op uses, surfaces a
/// `reject_reason` plus `would_commit = false` for hard preflight
/// failures (path escape, forbidden basename, restrict-guard violation,
/// source-and-dest both missing, source-or-dest is a directory), and
/// otherwise returns a populated [`MvDiff`] with `would_commit = true`.
fn dispatch_mv(
    system: &dyn System,
    base_dir: &Path,
    cfg: &ResolvedConfig,
    identity: PlanIdentity,
    src: &Path,
    dst: &Path,
    force: bool,
) -> Result<PlanReport> {
    use crate::document::allowlist;
    use crate::permissions::op_guard::pre_mutate_check;
    use crate::writer::ensure_not_forbidden_target;

    let empty = parser::parse("").context("parsing empty before-document for plan mv")?;
    let mut report = project_report("mv", &empty, &empty, cfg, identity);

    // Wrap every preflight that the live op would perform; a failing
    // check flips `would_commit` and surfaces the message verbatim.
    let projection = (|| -> Result<MvDiff> {
        ensure_not_forbidden_target(src)?;
        ensure_not_forbidden_target(dst)?;

        let dst_lexical = if dst.is_absolute() {
            dst.to_path_buf()
        } else {
            base_dir.join(dst)
        };
        if system.is_dir(&dst_lexical).unwrap_or(false) {
            anyhow::bail!(
                "destination is a directory: {} (this op moves a single file; pass an explicit destination path)",
                dst.display()
            );
        }

        let src_lexical = if src.is_absolute() {
            src.to_path_buf()
        } else {
            base_dir.join(src)
        };
        let src_exists = system.exists(&src_lexical).unwrap_or(false);

        let src_resolved = if src_exists {
            allowlist::resolve_sandboxed(
                system,
                base_dir,
                src,
                cfg.unrestricted,
                &cfg.trusted_roots,
            )?
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
        ensure_not_forbidden_target(&dst_resolved)?;

        let dst_exists = system.exists(&dst_resolved).unwrap_or(false);
        let noop_same_path = src_exists && src_resolved == dst_resolved;
        let idempotent_already_settled = !src_exists && dst_exists;

        if src_exists && system.is_dir(&src_resolved).unwrap_or(false) {
            anyhow::bail!(
                "source is a directory: {} (single-file moves only)",
                src.display()
            );
        }

        if !src_exists && !dst_exists {
            anyhow::bail!(
                "source not found: {} (and destination does not exist either)",
                src.display()
            );
        }

        if src_exists {
            pre_mutate_check(system, "mv", &src_resolved)?;
        }
        pre_mutate_check(system, "mv", &dst_resolved)?;

        if dst_exists && !noop_same_path && !idempotent_already_settled && !force {
            anyhow::bail!(
                "destination exists: {} (pass --force to overwrite)",
                dst.display()
            );
        }

        Ok(MvDiff {
            dst_absolute: dst_resolved,
            dst_exists,
            idempotent_already_settled,
            noop_same_path,
            src_absolute: if src_exists {
                src_resolved
            } else {
                src_lexical
            },
            src_exists,
        })
    })();

    match projection {
        Ok(diff) => {
            // Same-path no-op and the idempotent already-settled
            // branch both leave the filesystem untouched.
            report.noop = diff.noop_same_path || diff.idempotent_already_settled;
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
            let mut report = project_report("write", before, after, cfg, identity);
            report.noop = report.noop || *noop;
            Ok(report)
        }
        WriteProjection::Unsupported { reason } => {
            let empty =
                parser::parse("").context("parsing empty before-document for plan write")?;
            let mut report = project_report("write", &empty, &empty, cfg, identity);
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
            source_path: None,
            trusted_roots: Vec::new(),
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
