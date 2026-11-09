//! The MCP server of remargin: the tool service tixschema turns into a dispatcher, its handlers,
//! the guards each call runs first, the stdio host, and the paging helpers the listings share.

#[macro_use]
pub mod service;

pub mod answers;
pub mod args;
pub mod context;
pub mod dispatcher;
pub mod errors;
pub mod handlers;
pub mod host;

#[cfg(test)]
mod server_tests;
#[cfg(test)]
mod tests;

use core::mem;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Serialize;

use crate::activity::{self, CompactFileChanges};
use crate::config::ResolvedConfig;
use crate::config::identity::IdentityFlags;
use crate::config::permissions::resolve::ResolvedPermissions;
use crate::document;
use crate::operations;
use crate::operations::query::{self, QueryResult};
use crate::operations::search::{self, SearchResults};
use crate::parser;
use crate::permissions::op_guard::{CallerInfo, check_against_resolved_for_caller};
use crate::wait::Wait as _;

use self::answers::{
    CommentRow, GetAnswer, GetContent, GetLines, MatchRow, QueryFile, ReportSpillAnswer,
    SearchFile, SearchPage,
};

/// In UTF-8 bytes of emitted tool-result text: a proxy for the client's token limit, seeded low
/// so a fresh session rarely spills. The cap only ratchets down from here.
const DEFAULT_SPILL_CAP: usize = 40_000;

/// One part in this many of the cap is held back for the response envelope and indentation.
const SPILL_MARGIN_DIVISOR: usize = 10;

/// Callable under a human identity, which the MCP surface otherwise refuses: `whoami` to
/// diagnose the refusal and `identity_create` to render the agent identity it asks for.
pub(crate) const HUMAN_IDENTITY_EXEMPT_TOOLS: &[&str] = &["identity_create", "whoami"];

/// A group of positional rows that a page cuts to fit, such as one file of
/// a `query` answer.
pub(crate) trait RowGroup: Serialize {
    type Row: Serialize;

    fn push_row(&mut self, row: Self::Row);

    /// Takes the rows out, leaving the group empty; `None` when the group
    /// has no rows to take.
    fn take_rows(&mut self) -> Option<Vec<Self::Row>>;
}

/// One page of a grouped columnar payload.
pub(crate) struct GroupedPage<G> {
    effective_limit: Option<usize>,
    /// The groups that have at least one item on this page, in their
    /// original order, each holding only its rows on this page.
    groups: Vec<G>,
    /// Items across every group before paging: one per row, or one for a
    /// group that has no rows.
    total: usize,
}

impl<G> GroupedPage<G> {
    /// The effective limit when the size cap trimmed the page, the groups kept, and the total.
    pub(crate) fn into_parts(self) -> (Option<usize>, Vec<G>, usize) {
        (self.effective_limit, self.groups, self.total)
    }
}

/// Per-session adaptive state owned by the stdio run loop.
///
/// `spill_cap` is a learned size ceiling in remargin's own unit (UTF-8 bytes
/// of emitted tool-result text) — a self-consistent proxy for the client's
/// token limit, not an equivalent. It only ratchets DOWN (from an
/// agent-reported spill) and is seeded fresh each session with no disk
/// persistence, so a session re-learns rather than carrying a stale ceiling.
/// `last_response_size` is the byte size of the most recently emitted tool
/// result — what `report_spill` infers the offending size from.
#[derive(Debug)]
pub(crate) struct SessionState {
    last_response_size: usize,
    spill_cap: usize,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            last_response_size: 0,
            spill_cap: DEFAULT_SPILL_CAP,
        }
    }
}

impl SessionState {
    pub(crate) const fn record_response_size(&mut self, size: usize) {
        self.last_response_size = size;
    }

    /// Lowers the cap to the size the client reported, or to the last answer's size when none was
    /// given; the cap never rises and a size of 0 changes nothing.
    pub(crate) fn report_spill(&mut self, explicit: Option<usize>) -> ReportSpillAnswer {
        let reported = explicit.unwrap_or(self.last_response_size);
        let previous_cap = self.spill_cap;
        if reported > 0 {
            self.spill_cap = self.spill_cap.min(reported);
        }
        ReportSpillAnswer {
            inferred: explicit.is_none(),
            previous_cap,
            reported_size: reported,
            spill_cap: self.spill_cap,
        }
    }

    pub(crate) const fn spill_cap(&self) -> usize {
        self.spill_cap
    }
}

impl RowGroup for CompactFileChanges {
    type Row = activity::CompactChangeRow;

    fn push_row(&mut self, row: Self::Row) {
        self.changes.push(row);
    }

    fn take_rows(&mut self) -> Option<Vec<Self::Row>> {
        Some(mem::take(&mut self.changes))
    }
}

impl RowGroup for QueryFile {
    type Row = CommentRow;

    fn push_row(&mut self, row: Self::Row) {
        if let Some(comments) = &mut self.comments {
            comments.push(row);
        }
    }

    fn take_rows(&mut self) -> Option<Vec<Self::Row>> {
        self.comments.as_mut().map(mem::take)
    }
}

/// Boundary check for a single path. In an unconstrained session the path must canonicalise
/// under `base_dir`; a constrained or locked session goes through `op_guard`, so a violation
/// carries the canonical `trusted_roots` / `deny_ops` wording.
pub(crate) fn check_one_path(
    system: &dyn System,
    base_dir: &Path,
    permissions: &ResolvedPermissions,
    tool_name: &str,
    raw: &str,
) -> Result<()> {
    let candidate = Path::new(raw);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    };
    let canonical = canonicalize_or_lexical(system, &absolute)?;

    if permissions.trusted_roots_unconstrained() {
        let canonical_base = system
            .canonicalize(base_dir)
            .wait()
            .unwrap_or_else(|_err| base_dir.to_path_buf());
        if canonical == canonical_base || canonical.starts_with(&canonical_base) {
            return Ok(());
        }
        anyhow::bail!("path escapes MCP sandbox: {raw}");
    }

    check_against_resolved_for_caller(
        system,
        tool_name,
        &canonical,
        permissions,
        &CallerInfo::default(),
    )
}

/// Canonicalise `target` if it exists; otherwise walk parents until one
/// canonicalises and append the missing tail. Lets the boundary admit
/// writes to not-yet-existing files inside a covered root while still
/// rejecting paths outside it.
fn canonicalize_or_lexical(system: &dyn System, target: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = system.canonicalize(target).wait() {
        return Ok(canonical);
    }
    let mut suffix = PathBuf::new();
    let mut cursor = target.to_path_buf();
    while let Some(parent) = cursor.parent().map(Path::to_path_buf) {
        if parent.as_os_str().is_empty() {
            break;
        }
        let tail = cursor
            .file_name()
            .map(PathBuf::from)
            .with_context(|| format!("path missing file component: {}", target.display()))?;
        suffix = tail.join(&suffix);
        if let Ok(canonical_parent) = system.canonicalize(&parent).wait() {
            return Ok(canonical_parent.join(suffix));
        }
        cursor = parent;
    }
    Ok(target.to_path_buf())
}

/// MCP-only gate: a reply that explicitly opts out of acking *another author's*
/// comment (`auto_ack: false`) must justify it via `ack_skip_reason`. The smart
/// default and self-replies are exempt; the reason is validated, never stored.
fn check_ack_skip_reason(
    doc: &parser::ParsedDocument,
    identity: &str,
    parent_id: &str,
    auto_ack: Option<bool>,
    reason: Option<&str>,
) -> Result<()> {
    if auto_ack != Some(false) {
        return Ok(());
    }
    let Some(parent) = doc.find_comment(parent_id) else {
        return Ok(());
    };
    if parent.author == identity {
        return Ok(());
    }
    if reason.is_none_or(|r| r.trim().is_empty()) {
        bail!(
            "reply to comment {parent_id:?} sets auto_ack:false; provide ack_skip_reason explaining why you are not acknowledging it"
        );
    }
    Ok(())
}

/// Run [`check_ack_skip_reason`] over every reply op against a single file.
/// `ops` is `(reply_to, auto_ack, ack_skip_reason)` per op. The document is
/// parsed once, and only when at least one op opts out, so the common path
/// stays free. Parse/identity failures defer to the live op's canonical error.
pub(crate) fn enforce_ack_skip_reasons(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    file: &str,
    ops: &[(Option<&str>, Option<bool>, Option<&str>)],
) -> Result<()> {
    if !ops.iter().any(|(_, auto_ack, _)| *auto_ack == Some(false)) {
        return Ok(());
    }
    let Some(identity) = config.identity.as_deref() else {
        return Ok(());
    };
    let path = base_dir.join(file);
    let Ok(doc) = parser::parse_file(system, &path) else {
        return Ok(());
    };
    for (reply_to, auto_ack, reason) in ops {
        if let Some(parent_id) = reply_to {
            check_ack_skip_reason(&doc, identity, parent_id, *auto_ack, *reason)?;
        }
    }
    Ok(())
}

/// Read the requested window, narrowed to what fits under the size cap and
/// read again when narrowed, so the links describe exactly the lines
/// returned. The second value is the last line returned when the cap cut
/// the window.
pub(crate) fn read_get_window(
    system: &dyn System,
    base_dir: &Path,
    target: &Path,
    lines: Option<(usize, usize)>,
    spill_cap: usize,
    config: &ResolvedConfig,
) -> Result<(document::GetResult, Option<usize>)> {
    let result = document::get_with_links(system, base_dir, target, lines, false, config)?;
    let start_num = lines.map_or(1, |(s, _)| s);
    let line_values: Vec<&str> = result.content.split('\n').collect();
    let kept = rows_within_cap(&line_values, spill_cap);
    if kept >= line_values.len() {
        return Ok((result, None));
    }
    let end = start_num + kept - 1;
    let narrowed = document::get_with_links(
        system,
        base_dir,
        target,
        Some((start_num, end)),
        false,
        config,
    )?;
    Ok((narrowed, Some(end)))
}

/// The compact `get` text payload for one window.
pub(crate) fn get_envelope(
    result: document::GetResult,
    start_num: usize,
    line_numbers: bool,
    effective_end_line: Option<usize>,
) -> GetAnswer {
    // Link rows are positional `[alias, lines, target, title]`; `count` and `path` are derivable.
    let links = operations::links::to_compact_rows(result.links);
    let links_cols = column_names(&operations::links::LINK_COLS);
    if line_numbers {
        // Line numbers are contiguous, so state the start once and drop the
        // per-line objects: line i's number is `start_line + i`.
        GetAnswer::Lines(GetLines {
            effective_end_line,
            lines: result.content.split('\n').map(String::from).collect(),
            links,
            links_cols,
            start_line: start_num,
            total_lines: result.total_lines,
        })
    } else {
        GetAnswer::Content(GetContent {
            content: result.content,
            effective_end_line,
            links,
            links_cols,
            total_lines: result.total_lines,
        })
    }
}

/// Assemble the compact grouped `search` envelope from a page of results.
///
/// Builds the flat compact rows, clamps them against `spill_cap` (measuring
/// the emitted minified bytes), then groups the survivors by file. The
/// caller's own `limit` already bounded the window; the cap only narrows it
/// further. `effective_limit` is added only when the clamp trimmed the page —
/// its presence tells the agent to advance `offset` for the rest, and `total`
/// says how many remain.
pub(crate) fn search_compact_envelope(
    results: &SearchResults,
    spill_cap: usize,
    with_context: bool,
) -> SearchPage {
    let rows: Vec<MatchRow> = results
        .matches
        .iter()
        .map(|m| {
            if with_context {
                MatchRow::WithContext(search::compact_row_with_context(m))
            } else {
                MatchRow::Plain(search::compact_row(m))
            }
        })
        .collect();
    let (kept, effective_limit) = size_page(rows, spill_cap);

    let mut files: Vec<SearchFile> = Vec::new();
    for (m, row) in results.matches.iter().zip(kept) {
        let path = m.path.display().to_string();
        match files.last_mut() {
            Some(file) if file.path == path => file.matches.push(row),
            _ => files.push(SearchFile {
                matches: vec![row],
                path,
            }),
        }
    }
    SearchPage {
        effective_limit,
        files,
        match_cols: column_names(search::match_cols(with_context)),
        total: results.total,
    }
}

/// Trim a page so its emitted size stays under `spill_cap`.
///
/// Greedily keeps rows while the running minified size stays within the cap
/// (minus a margin for the envelope). Always keeps at least one row when the
/// window is non-empty, so a single oversized row can never livelock paging.
/// Returns `Some(effective_limit)` only when the cap forced fewer rows than
/// the window carried — the signal to advance `offset` for the rest.
/// `spill_cap` is a self-consistent byte proxy for the client's token limit,
/// not an equivalent.
pub(crate) fn size_page<T>(window: Vec<T>, spill_cap: usize) -> (Vec<T>, Option<usize>)
where
    T: Serialize,
{
    let kept = rows_within_cap(&window, spill_cap);
    if kept >= window.len() {
        (window, None)
    } else {
        let mut page = window;
        page.truncate(kept);
        (page, Some(kept))
    }
}

/// How many leading `rows` fit under `spill_cap`, admitting the first row
/// unconditionally.
fn rows_within_cap<T>(rows: &[T], spill_cap: usize) -> usize
where
    T: Serialize,
{
    kept_within_cap(rows.iter().map(emitted_len), spill_cap)
}

fn kept_within_cap(sizes: impl Iterator<Item = usize>, spill_cap: usize) -> usize {
    let budget = spill_cap.saturating_sub(spill_cap / SPILL_MARGIN_DIVISOR);
    let mut running = 0_usize;
    let mut kept = 0_usize;
    for size in sizes {
        if kept > 0 && running.saturating_add(size) > budget {
            break;
        }
        running = running.saturating_add(size);
        kept = kept.saturating_add(1);
    }
    kept
}

fn emitted_len<T>(item: &T) -> usize
where
    T: Serialize,
{
    serde_json::to_string(item).map_or(usize::MAX, |text| text.len())
}

/// Apply the caller's `offset` / `limit` window to `items`.
pub(crate) fn page_window<T>(items: Vec<T>, offset: usize, max_items: Option<usize>) -> Vec<T> {
    let rest = items.into_iter().skip(offset);
    match max_items {
        Some(limit) => rest.take(limit).collect(),
        None => rest.collect(),
    }
}

/// Page a grouped columnar payload (per-file groups holding positional rows)
/// by rows across all groups, then regroup the survivors.
pub(crate) fn page_grouped<G>(
    mut heads: Vec<G>,
    offset: usize,
    limit: Option<usize>,
    spill_cap: usize,
) -> GroupedPage<G>
where
    G: RowGroup,
{
    // Each item is (group index, row); a group without rows is one item
    // so it still lists.
    let mut items: Vec<(usize, Option<G::Row>)> = Vec::new();
    for (index, head) in heads.iter_mut().enumerate() {
        match head.take_rows() {
            Some(rows) if !rows.is_empty() => {
                items.extend(rows.into_iter().map(|row| (index, Some(row))));
            }
            _ => items.push((index, None)),
        }
    }
    let total = items.len();
    let window = page_window(items, offset, limit);
    let sizes = window.iter().map(|(index, row)| {
        row.as_ref()
            .map_or_else(|| emitted_len(&heads[*index]), emitted_len)
    });
    let kept = kept_within_cap(sizes, spill_cap);
    let effective_limit = (kept < window.len()).then_some(kept);

    let mut slots: Vec<Option<G>> = heads.into_iter().map(Some).collect();
    let mut paged: Vec<G> = Vec::new();
    let mut current: Option<usize> = None;
    for (index, row) in window.into_iter().take(kept) {
        if current != Some(index) {
            paged.extend(slots[index].take());
            current = Some(index);
        }
        if let Some(cells) = row
            && let Some(group) = paged.last_mut()
        {
            group.push_row(cells);
        }
    }
    GroupedPage {
        effective_limit,
        groups: paged,
        total,
    }
}

pub(crate) fn column_names(cols: &[&str]) -> Vec<String> {
    cols.iter().map(|col| String::from(*col)).collect()
}

pub(crate) fn comment_row(comment: &query::ExpandedComment, include_integrity: bool) -> CommentRow {
    if include_integrity {
        CommentRow::WithIntegrity(query::compact_row_with_integrity(comment))
    } else {
        CommentRow::Plain(query::compact_row(comment))
    }
}

/// One file of a `query` answer: the summary fields stay named, comments
/// become positional rows, and a summary query carries none.
pub(crate) fn query_file(result: &QueryResult, include_integrity: bool) -> QueryFile {
    QueryFile {
        comment_count: result.comment_count,
        comments: result.comments.as_ref().map(|comments| {
            comments
                .iter()
                .map(|comment| comment_row(comment, include_integrity))
                .collect()
        }),
        last_activity: result.last_activity,
        matched_count: result.matched_count,
        path: result.path.clone(),
        pending_count: result.pending_count,
        pending_for: result.pending_for.clone(),
    }
}

/// Answers one request against a configuration the caller resolved, over a server with no
/// session state: the seam the CLI tests drive.
///
/// # Errors
///
/// Returns an error if the permissions of `base_dir` cannot be resolved.
pub fn process_request(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    request_json: &str,
) -> Result<Option<String>> {
    host::process_request(
        &handlers::Server::new(),
        system,
        base_dir,
        config.clone(),
        request_json,
    )
}

/// Runs the server over stdin and stdout until stdin closes; see [`host::run`].
///
/// # Errors
///
/// Returns an error if the configuration or the permissions cannot be resolved, or stdin or
/// stdout fail.
pub fn run(
    system: &dyn System,
    base_dir: &Path,
    startup_flags: &IdentityFlags,
    startup_assets_dir: Option<&str>,
) -> Result<()> {
    host::run(system, base_dir, startup_flags, startup_assets_dir)
}
