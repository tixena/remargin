//! Find/replace engine — the mutating sibling of [`search`].
//!
//! Substitutes a pattern across **document body text only**, over a
//! single file or a whole directory tree, reusing remargin's
//! comment-preservation and post-verify subset gate so it can never
//! corrupt a comment or introduce an integrity anomaly.
//!
//! Comment blocks are never in scope — not their content, not their
//! frontmatter. This is a structural safety property, not a default
//! that can be flipped: there is no scope selector. The substitution is
//! applied to [`Segment::Body`] payloads exclusively; every
//! [`Segment::Comment`] is copied through untouched, and the rebuilt
//! document is committed through the same integrity tail [`crate::document::write`]
//! uses.
//!
//! [`search`]: crate::operations::search

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use regex::{NoExpand, Regex, RegexBuilder};
use serde::Serialize;

use tixschema::model_schema;

use crate::config::ResolvedConfig;
use crate::document::{self, allowlist};
use crate::parser::{self, ParsedDocument, Segment};
use crate::permissions::op_guard::pre_mutate_check_for_caller;
use crate::writer::ensure_not_forbidden_target;

/// Options for a replace operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct ReplaceOptions {
    /// Find what would change; write nothing.
    pub dry_run: bool,
    /// Case-insensitive matching.
    pub ignore_case: bool,
    /// The find pattern (literal or regex).
    pub pattern: String,
    /// Treat the pattern as a regex (default: literal). In literal mode
    /// the replacement is also literal (a `$` is inserted verbatim).
    pub regex: bool,
    /// The replacement text. In regex mode, `$1` / `${name}` expand to
    /// capture groups; in literal mode the text is inserted verbatim.
    pub replacement: String,
}

impl ReplaceOptions {
    /// Enable dry-run (report only; write nothing).
    #[must_use]
    pub const fn dry_run(mut self, yes: bool) -> Self {
        self.dry_run = yes;
        self
    }

    /// Enable case-insensitive matching.
    #[must_use]
    pub const fn ignore_case(mut self, yes: bool) -> Self {
        self.ignore_case = yes;
        self
    }

    /// Create a new set of replace options (literal pattern, literal
    /// replacement, case-sensitive, real write).
    #[must_use]
    pub const fn new(pattern: String, replacement: String) -> Self {
        Self {
            dry_run: false,
            ignore_case: false,
            pattern,
            regex: false,
            replacement,
        }
    }

    /// Enable regex mode.
    #[must_use]
    pub const fn regex(mut self, yes: bool) -> Self {
        self.regex = yes;
        self
    }
}

/// Per-file outcome of a replace operation.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct ReplaceFileOutcome {
    /// `true` when this file's body was (or would be) modified.
    pub changed: bool,
    /// Per-file failure (gate refusal, parse error, read error). In
    /// folder mode the run continues with the remaining files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Relative file path.
    pub path: PathBuf,
    /// Count of substitutions applied (or that would be applied) in the
    /// document body.
    pub replacements: usize,
}

/// Aggregate report for a replace operation over a file or folder.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct ReplaceReport {
    /// `true` when no byte was written (a `--dry-run` preview).
    pub dry_run: bool,
    /// Per-file outcomes, in walk order.
    pub files: Vec<ReplaceFileOutcome>,
    /// Number of files whose body changed (or would change).
    pub files_changed: usize,
    /// Number of files that could not be replaced (recorded in `files`).
    pub files_failed: usize,
    /// Sum of `replacements` across every file.
    pub total_replacements: usize,
}

/// A compiled find/replace matcher.
struct Matcher {
    regex: Regex,
}

impl Matcher {
    /// Apply the substitution to one body segment, returning the new
    /// text and the number of replacements made. In literal mode the
    /// replacement is inserted verbatim via [`NoExpand`] (a `$` is not a
    /// capture reference); in regex mode `$1` / `${name}` expand.
    fn apply(&self, text: &str, replacement: &str, literal: bool) -> (String, usize) {
        let count = self.regex.find_iter(text).count();
        if count == 0 {
            return (String::from(text), 0);
        }
        let replaced = if literal {
            self.regex.replace_all(text, NoExpand(replacement))
        } else {
            self.regex.replace_all(text, replacement)
        };
        (replaced.into_owned(), count)
    }
}

/// Build a [`Matcher`] from the replace options.
///
/// In literal mode the pattern is regex-escaped (matching `search`); the
/// literal-vs-capture behaviour of the *replacement* is decided at
/// substitution time via [`NoExpand`].
fn build_matcher(options: &ReplaceOptions) -> Result<Matcher> {
    if options.pattern.is_empty() {
        bail!("replace pattern cannot be empty");
    }
    let pattern = if options.regex {
        options.pattern.clone()
    } else {
        regex::escape(&options.pattern)
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(options.ignore_case)
        .build()
        .with_context(|| format!("invalid regex pattern: {}", options.pattern))?;
    Ok(Matcher { regex })
}

/// Find/replace across document body text.
///
/// `target` is a file or a directory. A directory walks the tree (the
/// same walk [`search`](crate::operations::search) uses) and applies the
/// replacement to every visible `.md` file; a per-file failure is
/// captured in that file's [`ReplaceFileOutcome::error`] and the run
/// continues. A file target runs the per-file algorithm once.
///
/// # Errors
///
/// Returns an error if the pattern is empty or an invalid regex, if the
/// directory cannot be walked, or if a single-file target cannot be
/// resolved within the sandbox. Per-file replace failures in folder mode
/// do **not** abort the run — they are recorded in the report.
pub fn replace(
    system: &dyn System,
    base_dir: &Path,
    target: &Path,
    options: &ReplaceOptions,
    config: &ResolvedConfig,
) -> Result<ReplaceReport> {
    let matcher = build_matcher(options)?;

    let resolved_target = allowlist::resolve_sandboxed(
        system,
        base_dir,
        target,
        config.unrestricted,
        &config.trusted_roots,
    )?;

    let mut files: Vec<ReplaceFileOutcome> = Vec::new();

    if system.is_dir(&resolved_target).unwrap_or(false) {
        let entries = system
            .walk_dir(&resolved_target, false, false)
            .with_context(|| format!("walking directory {}", resolved_target.display()))?;
        for entry in &entries {
            if !entry.is_file {
                continue;
            }
            let has_md_ext = entry
                .path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
            if !has_md_ext || !allowlist::is_visible(&entry.path, false) {
                continue;
            }
            let relative = entry
                .path
                .strip_prefix(base_dir)
                .unwrap_or(&entry.path)
                .to_path_buf();
            files.push(replace_one(
                system,
                &entry.path,
                &relative,
                &matcher,
                options,
                config,
            ));
        }
    } else {
        let relative = resolved_target
            .strip_prefix(base_dir)
            .unwrap_or(target)
            .to_path_buf();
        files.push(replace_one(
            system,
            &resolved_target,
            &relative,
            &matcher,
            options,
            config,
        ));
    }

    let total_replacements = files.iter().map(|f| f.replacements).sum();
    let files_changed = files.iter().filter(|f| f.changed).count();
    let files_failed = files.iter().filter(|f| f.error.is_some()).count();

    Ok(ReplaceReport {
        dry_run: options.dry_run,
        files,
        files_changed,
        files_failed,
        total_replacements,
    })
}

/// Run the per-file replace algorithm, capturing any failure into the
/// returned outcome's `error` field so folder-mode runs continue.
fn replace_one(
    system: &dyn System,
    resolved: &Path,
    relative: &Path,
    matcher: &Matcher,
    options: &ReplaceOptions,
    config: &ResolvedConfig,
) -> ReplaceFileOutcome {
    match try_replace_one(system, resolved, matcher, options, config) {
        Ok((replacements, changed)) => ReplaceFileOutcome {
            changed,
            error: None,
            path: relative.to_path_buf(),
            replacements,
        },
        Err(err) => ReplaceFileOutcome {
            changed: false,
            error: Some(format!("{err:#}")),
            path: relative.to_path_buf(),
            replacements: 0,
        },
    }
}

/// The per-file pipeline: gate, read, parse, body-only substitute,
/// reassemble, and commit through the shared integrity tail. Returns the
/// substitution count and whether the file was (or would be) changed.
fn try_replace_one(
    system: &dyn System,
    resolved: &Path,
    matcher: &Matcher,
    options: &ReplaceOptions,
    config: &ResolvedConfig,
) -> Result<(usize, bool)> {
    ensure_not_forbidden_target(resolved)?;
    pre_mutate_check_for_caller(system, "replace", resolved, &config.caller_info())?;

    if !allowlist::is_visible(resolved, false) {
        bail!("file not visible: {}", resolved.display());
    }

    let content = system
        .read_to_string(resolved)
        .with_context(|| format!("reading {}", resolved.display()))?;
    let doc = parser::parse(&content).context("parsing document")?;

    // Body-only substitution: rewrite every `Body` payload, copy every
    // `Comment` through untouched. Reassembly is via `to_markdown()`,
    // consistent with every other mutating command (Design Decision 4).
    let mut segments: Vec<Segment> = Vec::with_capacity(doc.segments.len());
    let mut replacements = 0_usize;
    for seg in doc.segments {
        match seg {
            Segment::Body(text) => {
                let (rewritten, count) = matcher.apply(&text, &options.replacement, !options.regex);
                replacements += count;
                segments.push(Segment::Body(rewritten));
            }
            Segment::Comment(cm) => segments.push(Segment::Comment(cm)),
        }
    }

    // No body match: the file is untouched (a comment-only match can
    // never reach the body substitution above), so report a no-op
    // without invoking the commit tail.
    if replacements == 0 {
        return Ok((0, false));
    }

    let new_content = ParsedDocument { segments }
        .to_markdown()
        .context("reassembling document after replace")?;

    if options.dry_run {
        // Project the commit without writing: parse + preservation +
        // frontmatter + subset gate, surfacing a gate refusal as an
        // error, but never touching disk.
        let changed = document::project_commit_markdown(system, config, resolved, &new_content)?;
        return Ok((replacements, changed));
    }

    let outcome = document::commit_markdown(system, config, resolved, &new_content, false)?;
    Ok((replacements, !outcome.noop))
}
