//! `remargin activity` core (rem-g3sy.3 / T33).
//!
//! [`gather_activity`] is the single public entry point. Given a
//! starting path (file or directory) and an optional `since` cutoff,
//! it walks managed `.md` files and returns a structured per-file
//! list of "what's changed" — comments, ack-roster entries, and
//! sandbox-roster entries whose timestamp is after the cutoff.
//!
//! When `since` is `None`, the per-file cutoff is derived from the
//! caller's last action in that file (latest of: a comment they
//! authored, an ack they signed, or a sandbox roster entry they
//! own). Files where the caller has never acted return everything
//! (the "initial-touch" fallback). The design lives in
//! `discussions/2026-04-26__activity_command_design.md` (eburgos
//! notes vault).
//!
//! ## Surface boundaries
//!
//! - **Comments**: when [`crate::parser::Comment::edited_at`] is
//!   set, the surface predicate uses `max(ts, edited_at)` and the
//!   carried `ts` on the Change is that max so consumers see the
//!   timestamp that triggered the surface (rem-g3sy.2).
//! - **Acks**: each entry on a comment's ack roster is its own
//!   change record (multiple acks on one comment → multiple
//!   `Change::Ack` entries).
//! - **Sandbox**: each sandbox-roster entry is its own change
//!   record. Re-sandboxing is visible because rem-g3sy.1 makes the
//!   timestamp refresh on every successful add.
//!
//! ## Out of scope
//!
//! Per the design doc:
//! - No event framing — output is "changes," not events with deltas.
//! - No body content hashing.
//! - Deletions are silent (no tombstones in the data layer).
//! - Reactions are NOT in v1.
//!
//! CLI / MCP wiring + `--pretty` formatting belong to T34
//! (`rem-g3sy.4`).

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, FixedOffset};
use os_shim::System;
use serde::Serialize;

use crate::config::load_config_filtered_with_path;
use crate::frontmatter::read_sandbox_entries;
use crate::parser::{self, Comment, SandboxEntry};

/// One file's changes, sorted by ts ascending. Files with no
/// emitted changes are omitted from the wider [`ActivityResult`] —
/// callers never see zero-length entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FileChanges {
    /// One change per surfaced event (comment, ack, sandbox-add).
    /// Sorted ts-ascending; ties broken by `kind` then by id /
    /// `by` so the order is deterministic across runs.
    pub changes: Vec<Change>,
    /// Latest ts across all changes in this file. Mirrors the max
    /// of `changes[*].ts`; surfaced separately so the activity
    /// summary view does not need to fold the changes vec.
    pub newest_ts: Option<DateTime<FixedOffset>>,
    /// File path (canonical absolute, mirroring how the walker
    /// emits it).
    pub path: PathBuf,
}

/// Discriminated change record. `kind` field is the JSON tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[non_exhaustive]
pub enum Change {
    /// An ack landed on a comment's roster after the cutoff. One
    /// record per (comment, ack-author) pair.
    Ack {
        by: String,
        comment_id: String,
        ts: DateTime<FixedOffset>,
    },
    /// A comment was created (or, when `edited_at` is set, edited)
    /// after the cutoff. The `ts` field carries
    /// `max(ts, edited_at)` so consumers see the timestamp that
    /// triggered the surface.
    Comment {
        author: String,
        author_type: String,
        comment_id: String,
        line_end: usize,
        line_start: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
        to: Vec<String>,
        ts: DateTime<FixedOffset>,
    },
    /// A sandbox-roster entry landed (or refreshed via rem-g3sy.1)
    /// after the cutoff. One record per (file, identity) pair.
    Sandbox {
        by: String,
        ts: DateTime<FixedOffset>,
    },
}

impl Change {
    fn id_for_sort(&self) -> &str {
        match self {
            Self::Ack { comment_id, .. } | Self::Comment { comment_id, .. } => comment_id,
            Self::Sandbox { by, .. } => by,
        }
    }

    const fn kind_label(&self) -> &'static str {
        match self {
            Self::Ack { .. } => "ack",
            Self::Comment { .. } => "comment",
            Self::Sandbox { .. } => "sandbox",
        }
    }

    /// Carrying ts of the change. Drives both the per-file sort
    /// and the per-file `newest_ts` fold.
    #[must_use]
    pub const fn ts(&self) -> DateTime<FixedOffset> {
        match self {
            Self::Ack { ts, .. } | Self::Comment { ts, .. } | Self::Sandbox { ts, .. } => *ts,
        }
    }
}

/// Top-level result: one [`FileChanges`] per managed `.md` that
/// surfaced at least one change, plus the overall newest ts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ActivityResult {
    /// Files with at least one change. Sorted by path so the
    /// output is deterministic.
    pub files: Vec<FileChanges>,
    /// Max of every file's `newest_ts`. `None` when no file had
    /// any change.
    pub newest_ts_overall: Option<DateTime<FixedOffset>>,
}

/// Gather activity for `path` (file or directory) since `since`.
///
/// `caller_identity` drives the per-file initial-touch fallback
/// when `since` is `None`: the cutoff for each file is the latest
/// of the caller's own activity (comment authorship, ack, sandbox)
/// in that file. When the caller has never acted, the cutoff is
/// `None` and the function returns every change in the file.
///
/// # Errors
///
/// - `path` does not live under any `.remargin.yaml`-managed
///   realm.
/// - I/O / parse failures from the walker or the markdown parser.
pub fn gather_activity(
    system: &dyn System,
    path: &Path,
    since: Option<DateTime<FixedOffset>>,
    caller_identity: &str,
) -> Result<ActivityResult> {
    let realm_anchor = if system.is_dir(path).unwrap_or(false) {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    if load_config_filtered_with_path(system, realm_anchor, None)?.is_none() {
        bail!(
            "activity: {} is outside any .remargin.yaml-managed realm",
            path.display()
        );
    }

    let files = collect_managed_md_files(system, path)?;
    let mut result = ActivityResult::default();
    for file_path in files {
        let Some(file_changes) = gather_one_file(system, &file_path, since, caller_identity) else {
            continue;
        };
        if let Some(file_ts) = file_changes.newest_ts {
            result.newest_ts_overall = Some(result.newest_ts_overall.map_or(file_ts, |prior| {
                if prior >= file_ts { prior } else { file_ts }
            }));
        }
        result.files.push(file_changes);
    }
    result.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

fn collect_managed_md_files(system: &dyn System, path: &Path) -> Result<Vec<PathBuf>> {
    if !system.is_dir(path).unwrap_or(false) {
        return Ok(vec![path.to_path_buf()]);
    }
    let entries = system
        .walk_dir(path, false, false)
        .with_context(|| format!("walking {}", path.display()))?;
    let mut out: Vec<PathBuf> = entries
        .into_iter()
        .filter(|entry| entry.is_file && is_md_path(&entry.path))
        .map(|entry| entry.path)
        .collect();
    out.sort();
    Ok(out)
}

fn is_md_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Build the per-file change list. Returns `None` when the file
/// has no surfaced changes — the caller drops it from the result
/// so callers never see zero-length entries. Read / parse errors
/// also collapse to `None` because the activity surface is
/// best-effort: a malformed file is invisible, not fatal.
fn gather_one_file(
    system: &dyn System,
    file_path: &Path,
    since: Option<DateTime<FixedOffset>>,
    caller_identity: &str,
) -> Option<FileChanges> {
    let body = system.read_to_string(file_path).ok()?;
    let doc = parser::parse(&body).ok()?;
    let comments = doc.comments();
    let sandbox_entries = read_sandbox_entries(&doc).unwrap_or_default();

    let cutoff = since.map_or_else(
        || caller_last_action(&comments, &sandbox_entries, caller_identity),
        Some,
    );

    let mut changes: Vec<Change> = Vec::new();

    for cm in &comments {
        if let Some(change) = comment_change(cm, cutoff) {
            changes.push(change);
        }
        for ack in &cm.ack {
            if past_cutoff(ack.ts, cutoff) {
                changes.push(Change::Ack {
                    by: ack.author.clone(),
                    comment_id: cm.id.clone(),
                    ts: ack.ts,
                });
            }
        }
    }

    for entry in &sandbox_entries {
        if past_cutoff(entry.ts, cutoff) {
            changes.push(Change::Sandbox {
                by: entry.author.clone(),
                ts: entry.ts,
            });
        }
    }

    if changes.is_empty() {
        return None;
    }

    sort_changes(&mut changes);
    let newest_ts = changes.last().map(Change::ts);

    Some(FileChanges {
        changes,
        newest_ts,
        path: file_path.to_path_buf(),
    })
}

/// `max(comment.ts where caller authored, ack.ts where caller
/// acked, sandbox_entry.ts where caller is on the roster)`.
/// Returns `None` when the caller has no activity in the file —
/// triggers the initial-touch "everything" fallback.
fn caller_last_action(
    comments: &[&Comment],
    sandbox_entries: &[SandboxEntry],
    caller: &str,
) -> Option<DateTime<FixedOffset>> {
    let mut accumulator: Option<DateTime<FixedOffset>> = None;
    let consider = |slot: &mut Option<DateTime<FixedOffset>>, candidate: DateTime<FixedOffset>| {
        if slot.is_none_or(|prior| candidate > prior) {
            *slot = Some(candidate);
        }
    };

    for cm in comments {
        if cm.author == caller {
            consider(&mut accumulator, cm.effective_ts());
        }
        for ack in &cm.ack {
            if ack.author == caller {
                consider(&mut accumulator, ack.ts);
            }
        }
    }
    for entry in sandbox_entries {
        if entry.author == caller {
            consider(&mut accumulator, entry.ts);
        }
    }
    accumulator
}

fn comment_change(cm: &Comment, cutoff: Option<DateTime<FixedOffset>>) -> Option<Change> {
    let effective = cm.effective_ts();
    if !past_cutoff(effective, cutoff) {
        return None;
    }
    let line_start = cm.line;
    let line_end = cm.line + cm.content.lines().count().saturating_add(2);
    Some(Change::Comment {
        author: cm.author.clone(),
        author_type: cm.author_type.as_str().to_owned(),
        comment_id: cm.id.clone(),
        line_end,
        line_start,
        reply_to: cm.reply_to.clone(),
        to: cm.to.clone(),
        ts: effective,
    })
}

fn past_cutoff(ts: DateTime<FixedOffset>, cutoff: Option<DateTime<FixedOffset>>) -> bool {
    cutoff.is_none_or(|cut| ts > cut)
}

fn sort_changes(changes: &mut [Change]) {
    changes.sort_by(|a, b| {
        a.ts()
            .cmp(&b.ts())
            .then_with(|| a.kind_label().cmp(b.kind_label()))
            .then_with(|| a.id_for_sort().cmp(b.id_for_sort()))
    });
}
