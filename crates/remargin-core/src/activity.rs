//! `remargin activity` core. [`gather_activity`] walks managed `.md`
//! under a starting path and returns per-file "what's changed" —
//! comments, ack-roster entries, sandbox-roster entries past a cutoff.
//!
//! With `since = None` the per-file cutoff is the caller's last
//! action in that file (comment authored, ack signed, or sandbox
//! roster entry owned). Files the caller has never touched return
//! everything (initial-touch fallback).

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, FixedOffset};
use os_shim::System;
use serde::Serialize;
use serde_json::{Value, json};
use tixschema::model_schema;

use crate::config::registry::Registry;
use crate::config::{ResolvedConfig, load_config_filtered_with_path, load_registry};
use crate::frontmatter::read_sandbox_entries;
use crate::parser::{self, Comment, SandboxEntry};

/// Columns a change kind lacks are `null` in that kind's row.
pub const CHANGE_COLS: [&str; 9] = [
    "ts",
    "kind",
    "author",
    "author_type",
    "comment_id",
    "line_start",
    "line_end",
    "reply_to",
    "to",
];

/// One file's changes, sorted by ts ascending. Files with no
/// emitted changes are omitted from the wider [`ActivityResult`] —
/// callers never see zero-length entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FileChanges {
    /// Sorted by ts, then by kind, then by id or author.
    pub changes: Vec<Change>,
    /// The explicit `since`, else the caller's last action in this file, else `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff_applied: Option<DateTime<FixedOffset>>,
    pub newest_ts: Option<DateTime<FixedOffset>>,
    pub path: PathBuf,
}

/// Discriminated change record. `kind` field is the JSON tag.
///
/// Every variant exposes the actor under the same field name:
/// `author`, with optional `author_type` (`"human"` / `"agent"`).
/// `author_type` is omitted when the registry has no entry for the
/// actor (rather than silently defaulting).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[non_exhaustive]
pub enum Change {
    Ack {
        author: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        author_type: Option<String>,
        comment_id: String,
        ts: DateTime<FixedOffset>,
    },
    /// `ts` carries `max(ts, edited_at)`, the timestamp that surfaced the change.
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
    Sandbox {
        author: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        author_type: Option<String>,
        ts: DateTime<FixedOffset>,
    },
}

impl Change {
    fn id_for_sort(&self) -> &str {
        match self {
            Self::Ack {
                comment_id,
                author: _,
                author_type: _,
                ts: _,
            }
            | Self::Comment {
                comment_id,
                author: _,
                author_type: _,
                line_end: _,
                line_start: _,
                reply_to: _,
                to: _,
                ts: _,
            } => comment_id,
            Self::Sandbox {
                author,
                author_type: _,
                ts: _,
            } => author,
        }
    }

    const fn kind_label(&self) -> &'static str {
        match self {
            Self::Ack {
                author: _,
                author_type: _,
                comment_id: _,
                ts: _,
            } => "ack",
            Self::Comment {
                author: _,
                author_type: _,
                comment_id: _,
                line_end: _,
                line_start: _,
                reply_to: _,
                to: _,
                ts: _,
            } => "comment",
            Self::Sandbox {
                author: _,
                author_type: _,
                ts: _,
            } => "sandbox",
        }
    }

    #[must_use]
    pub const fn ts(&self) -> DateTime<FixedOffset> {
        match self {
            Self::Ack {
                ts,
                author: _,
                author_type: _,
                comment_id: _,
            }
            | Self::Comment {
                ts,
                author: _,
                author_type: _,
                comment_id: _,
                line_end: _,
                line_start: _,
                reply_to: _,
                to: _,
            }
            | Self::Sandbox {
                ts,
                author: _,
                author_type: _,
            } => *ts,
        }
    }
}

/// Top-level result: one [`FileChanges`] per managed `.md` that
/// surfaced at least one change, plus the overall newest ts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ActivityResult {
    pub cutoff_explicit: bool,
    /// Sorted by path.
    pub files: Vec<FileChanges>,
    pub newest_ts_overall: Option<DateTime<FixedOffset>>,
}

/// One compact change row, positional columns named by [`CHANGE_COLS`].
///
/// The three kinds share this widest shape; a column absent for a kind
/// is `None`. `author_type` is `Option<String>` (the ack / sandbox
/// arity), always `Some` for comments. `to` is `Some(vec)` for comments
/// (even an empty broadcast list), `None` for acks / sandboxes — so the
/// broadcast signal survives against not-applicable. `kind` keeps the
/// serde tag values `ack` / `comment` / `sandbox`.
#[model_schema(name = "CompactChangeRow")]
pub type CompactChangeRow = (
    DateTime<FixedOffset>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<usize>,
    Option<usize>,
    Option<String>,
    Option<Vec<String>>,
);

/// Schema anchor for one compact per-file record.
///
/// Mirrors [`FileChanges`] but its `changes` are positional
/// [`CompactChangeRow`]s. Exists so xtask emits the TS / Zod types the
/// LLM consumer reads; the runtime builds the shape in [`to_compact_activity`]
/// and the enclosing envelope (`cutoff_explicit`, `newest_ts_overall`,
/// `change_cols`, `files`) is assembled there too.
#[model_schema]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct CompactFileChanges {
    pub changes: Vec<CompactChangeRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff_applied: Option<DateTime<FixedOffset>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_ts: Option<DateTime<FixedOffset>>,
    pub path: PathBuf,
}

#[must_use]
pub fn to_compact_row(change: &Change) -> CompactChangeRow {
    let ts = change.ts();
    let kind = change.kind_label().to_owned();
    match change {
        Change::Ack {
            author,
            author_type,
            comment_id,
            ts: _,
        } => (
            ts,
            kind,
            author.clone(),
            author_type.clone(),
            Some(comment_id.clone()),
            None,
            None,
            None,
            None,
        ),
        Change::Comment {
            author,
            author_type,
            comment_id,
            line_end,
            line_start,
            reply_to,
            to,
            ts: _,
        } => (
            ts,
            kind,
            author.clone(),
            Some(author_type.clone()),
            Some(comment_id.clone()),
            Some(*line_start),
            Some(*line_end),
            reply_to.clone(),
            Some(to.clone()),
        ),
        Change::Sandbox {
            author,
            author_type,
            ts: _,
        } => (
            ts,
            kind,
            author.clone(),
            author_type.clone(),
            None,
            None,
            None,
            None,
            None,
        ),
    }
}

/// The compact activity envelope for a verbose result:
/// `{cutoff_explicit, newest_ts_overall, change_cols, files}`.
#[must_use]
pub fn to_compact_activity(result: &ActivityResult) -> Value {
    let files: Vec<Value> = result.files.iter().map(to_compact_file).collect();
    json!({
        "cutoff_explicit": result.cutoff_explicit,
        "newest_ts_overall": result.newest_ts_overall,
        "change_cols": CHANGE_COLS,
        "files": files,
    })
}

/// One compact per-file record: `path` stays named, `newest_ts` /
/// `cutoff_applied` stay named when present (absent when unset, never
/// `null`), `changes` become positional rows.
fn to_compact_file(file: &FileChanges) -> Value {
    let changes: Vec<CompactChangeRow> = file.changes.iter().map(to_compact_row).collect();
    let mut obj = serde_json::Map::new();
    obj.insert(String::from("path"), json!(file.path));
    if let Some(newest) = file.newest_ts {
        obj.insert(String::from("newest_ts"), json!(newest));
    }
    if let Some(cutoff) = file.cutoff_applied {
        obj.insert(String::from("cutoff_applied"), json!(cutoff));
    }
    obj.insert(String::from("changes"), json!(changes));
    Value::Object(obj)
}

/// Gathers the changes under `path` (a file or a directory) since `since`.
///
/// With no `since`, each file's cutoff is the caller's latest action in it, and a file the
/// caller never touched returns every change.
///
/// # Errors
///
/// Returns an error when `config` carries no identity, `path` is under no managed realm, a strict
/// realm covering `path` does not admit the caller, or the walk or the parser fails.
pub fn gather_activity(
    system: &dyn System,
    path: &Path,
    since: Option<DateTime<FixedOffset>>,
    config: &ResolvedConfig,
) -> Result<ActivityResult> {
    let Some(caller_identity) = config.identity.as_deref() else {
        bail!(
            "activity: caller identity required (declare one via the \
             identity flags or a config path)"
        );
    };
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
    config.ensure_can_read(system, path)?;

    let registry = load_registry(system, realm_anchor)?;
    let files = collect_managed_md_files(system, path, config)?;
    let mut result = ActivityResult {
        cutoff_explicit: since.is_some(),
        ..ActivityResult::default()
    };
    for file_path in files {
        let Some(file_changes) = gather_one_file(
            system,
            &file_path,
            since,
            caller_identity,
            registry.as_ref(),
        ) else {
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

fn collect_managed_md_files(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<Vec<PathBuf>> {
    if !system.is_dir(path).unwrap_or(false) {
        return Ok(vec![path.to_path_buf()]);
    }
    let entries = system
        .walk_dir(path, false, false)
        .with_context(|| format!("walking {}", path.display()))?;
    let mut gate = config.read_gate();
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in entries {
        if !entry.is_file || !is_md_path(&entry.path) {
            continue;
        }
        if !gate.admits(system, &entry.path)? {
            continue;
        }
        out.push(entry.path);
    }
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
    registry: Option<&Registry>,
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
                    author: ack.author.clone(),
                    author_type: lookup_author_type(registry, &ack.author),
                    comment_id: cm.id.clone(),
                    ts: ack.ts,
                });
            }
        }
    }

    for entry in &sandbox_entries {
        if past_cutoff(entry.ts, cutoff) {
            changes.push(Change::Sandbox {
                author: entry.author.clone(),
                author_type: lookup_author_type(registry, &entry.author),
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
        cutoff_applied: cutoff,
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

/// Resolve `author_type` for a sandbox / ack actor against the
/// loaded registry. Returns `None` when the registry is absent or
/// has no entry for `identity` so the JSON output omits the field
/// rather than guessing — consumers see a clear "unknown" signal.
fn lookup_author_type(registry: Option<&Registry>, identity: &str) -> Option<String> {
    registry?
        .participants
        .get(identity)
        .map(|p| p.author_type.clone())
}

fn sort_changes(changes: &mut [Change]) {
    changes.sort_by(|a, b| {
        a.ts()
            .cmp(&b.ts())
            .then_with(|| a.kind_label().cmp(b.kind_label()))
            .then_with(|| a.id_for_sort().cmp(b.id_for_sort()))
    });
}
