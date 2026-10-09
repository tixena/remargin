//! Style checks on comment bodies, with a severity per check.
//!
//! [`crate::linter`] always rejects and [`crate::advice`] never does. This
//! module sits between them, and the severity is a property of the check
//! rather than of the module: a mechanical, unambiguous fault is refused,
//! a heuristic one is only reported.
//!
//! The refusal applies to non-human authors alone. An agent that is turned
//! away reads the message, fixes the body and retries — a clean repair loop
//! that costs nothing. Refusing a person is a tool arguing with its user,
//! and they will route around it.
//!
//! There is deliberately no override: a check that needs one is a check
//! that is wrong, and the fix is to demote it.

#[cfg(test)]
mod tests;

use core::fmt::Write as _;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;

use crate::advice::{self, Advice};
use crate::parser::AuthorType;

/// Generous on purpose: the check should fire only on a body that is unarguably a slab.
const DENSE_BODY_CHARS: usize = 600;

/// Matched only at the start of the body's last block, so the same words mid-comment pass.
const TRAILING_METADATA_OPENERS: &[&str] = &[
    "as an aside",
    "for context",
    "fyi",
    "heads-up",
    "just so you know",
    "not your concern",
    "note that",
    "one observation",
    "one thing",
    "worth noting",
];

/// Capture 1 is the opening backtick, if any; capture 2 is the candidate id.
static BARE_ID_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i:\b(?:comments|comment|replies|reply|thread|as in|see|per)\s+(?:to |at |on |the )?)(`?)([a-z0-9]{3,4})\b",
    )
    .unwrap()
});

/// Whether a finding refuses the comment or is only reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Reject,
    Warn,
}

/// Which check produced a finding. Severity hangs off the kind rather than
/// off the individual finding, so two findings of one kind can never
/// disagree about whether that kind refuses — which is what the edit
/// comparison counts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FindingKind {
    BareIdReference,
    DenseBody,
    HardWrap,
    TrailingMetadata,
}

impl FindingKind {
    const fn severity(self) -> Severity {
        match self {
            Self::HardWrap | Self::TrailingMetadata => Severity::Reject,
            Self::BareIdReference | Self::DenseBody => Severity::Warn,
        }
    }
}

/// One style fault found in a comment body, with the line it sits on and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StyleFinding {
    kind: FindingKind,
    line: usize,
    message: String,
}

/// Refuse a comment body whose reject-tier checks fail.
///
/// # Errors
///
/// Returns an error naming every offending line and what to do about it.
pub fn gate(content: &str, author_type: &AuthorType) -> Result<()> {
    if matches!(*author_type, AuthorType::Human) {
        return Ok(());
    }

    refuse(
        review(content)
            .into_iter()
            .filter(|finding| finding.kind.severity() == Severity::Reject),
    )
}

/// Refuse an edit that leaves the body worse than it found it: a reject-tier
/// kind occurring more often in `new_content` than in `old_content`.
///
/// A repair, full or partial, passes because the counts fall; an inherited
/// fault the editor did not touch passes because its count holds. Gating the
/// new body outright would instead strand an editor mid-repair on a body
/// somebody else wrote.
///
/// # Errors
///
/// Returns an error naming every offending line and what to do about it.
pub fn gate_edit(old_content: &str, new_content: &str, author_type: &AuthorType) -> Result<()> {
    if matches!(*author_type, AuthorType::Human) {
        return Ok(());
    }

    let before = reject_counts(old_content);
    let worsened: HashSet<FindingKind> = reject_counts(new_content)
        .into_iter()
        .filter(|(kind, count)| *count > before.get(kind).copied().unwrap_or(0))
        .map(|(kind, _)| kind)
        .collect();

    // Every occurrence of a worsened kind is named, inherited ones included:
    // the editor has to bring the count back down and picks which one goes.
    refuse(
        review(new_content)
            .into_iter()
            .filter(|finding| worsened.contains(&finding.kind)),
    )
}

/// Occurrences per reject-tier kind in `content`.
fn reject_counts(content: &str) -> HashMap<FindingKind, usize> {
    let mut counts = HashMap::new();
    for finding in review(content) {
        if finding.kind.severity() == Severity::Reject {
            *counts.entry(finding.kind).or_insert(0) += 1;
        }
    }
    counts
}

fn refuse(findings: impl IntoIterator<Item = StyleFinding>) -> Result<()> {
    let mut buf = String::new();
    for finding in findings {
        let _ = writeln!(buf, "  line {}: {}", finding.line, finding.message);
    }
    if buf.is_empty() {
        return Ok(());
    }
    bail!("comment body rejected:\n{buf}")
}

/// Every advisory note a written comment body earns: the general
/// [`crate::advice`] pass plus this module's warn tier, in line order.
#[must_use]
pub fn notes(content: &str) -> Vec<Advice> {
    let mut notes = advice::review(content);
    notes.extend(review(content).into_iter().filter_map(|finding| {
        (finding.kind.severity() == Severity::Warn).then_some(Advice {
            line: finding.line,
            message: finding.message,
        })
    }));
    notes.sort_by_key(|note| note.line);
    notes
}

/// Every finding on `content`, both tiers, in line order.
fn review(content: &str) -> Vec<StyleFinding> {
    let mut findings = Vec::new();
    check_hard_wraps(content, &mut findings);
    check_trailing_metadata(content, &mut findings);
    check_bare_id_references(content, &mut findings);
    check_dense_body(content, &mut findings);
    findings.sort_by_key(|finding| finding.line);
    findings
}

/// Flag a reference that names a comment by its id instead of saying what
/// that comment said. A regex over prose, so it carries real false-positive
/// risk and only ever warns.
fn check_bare_id_references(content: &str, findings: &mut Vec<StyleFinding>) {
    for block in advice::prose_blocks(content) {
        for (offset, line) in block.lines.iter().enumerate() {
            for caps in BARE_ID_REFERENCE.captures_iter(line) {
                let backticked = caps.get(1).is_some_and(|tick| !tick.as_str().is_empty());
                let Some(token) = caps.get(2).map(|id| id.as_str()) else {
                    continue;
                };
                if !backticked && !looks_like_id(token) {
                    continue;
                }
                findings.push(StyleFinding {
                    kind: FindingKind::BareIdReference,
                    line: block.line + offset,
                    message: format!(
                        "{token:?} reads as a comment id; quote or paraphrase what that comment said instead, so the reference stands on its own"
                    ),
                });
            }
        }
    }
}

/// Flag a body that runs long with no blank line anywhere. The threshold is
/// a guess, so this only ever warns.
fn check_dense_body(content: &str, findings: &mut Vec<StyleFinding>) {
    if content.lines().any(|line| line.trim().is_empty()) {
        return;
    }
    let width: usize = advice::prose_blocks(content)
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| line.chars().count())
        .sum();
    if width < DENSE_BODY_CHARS {
        return;
    }
    findings.push(StyleFinding {
        kind: FindingKind::DenseBody,
        line: 1,
        message: format!(
            "the body runs {width} characters with no blank line anywhere; split it into blocks separated by blank lines so a reader can scan it"
        ),
    });
}

/// Promote the hard-wrap advice to a rejection. Whether a paragraph is
/// split across lines is mechanical and unambiguous, which is what earns
/// this check the reject tier.
fn check_hard_wraps(content: &str, findings: &mut Vec<StyleFinding>) {
    findings.extend(
        advice::hard_wrapped_paragraphs(content)
            .into_iter()
            .map(|note| StyleFinding {
                kind: FindingKind::HardWrap,
                line: note.line,
                message: note.message,
            }),
    );
}

/// Flag a body whose last block opens with a closing addendum. The phrase
/// list is closed and matched only at the start of the final block, which
/// is what keeps it precise enough to reject on.
///
/// When the body ends on a fence or a blockquote there is nothing to match:
/// a phrase inside one of those is quoted, not asserted, and a prose line
/// above one is introducing it rather than trailing off after it.
fn check_trailing_metadata(content: &str, findings: &mut Vec<StyleFinding>) {
    let blocks = advice::prose_blocks(content);
    let Some(last) = blocks.last() else {
        return;
    };
    if Some(last.line + last.lines.len() - 1) != last_content_line(content) {
        return;
    }
    let Some(opener) = last.lines.first() else {
        return;
    };
    let text = strip_emphasis(opener.trim_start()).to_lowercase();
    let Some(phrase) = TRAILING_METADATA_OPENERS
        .iter()
        .find(|opening| text.starts_with(**opening))
    else {
        return;
    };
    findings.push(StyleFinding {
        kind: FindingKind::TrailingMetadata,
        line: last.line,
        message: format!(
            "the comment's last block opens with {phrase:?}; move the point inline where it is relevant, or cut it"
        ),
    });
}

/// 1-indexed line of the last line in `content` that carries anything.
fn last_content_line(content: &str) -> Option<usize> {
    content
        .split('\n')
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(idx, _)| idx + 1)
        .last()
}

/// Whether a bare token is id-shaped: short, and mixing letters with digits
/// the way a generated id does but an English word does not.
fn looks_like_id(token: &str) -> bool {
    token.chars().any(|ch| ch.is_ascii_digit()) && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

/// `text` past one leading bold or italic marker.
fn strip_emphasis(text: &str) -> &str {
    text.strip_prefix("**")
        .or_else(|| text.strip_prefix("__"))
        .or_else(|| text.strip_prefix('*'))
        .or_else(|| text.strip_prefix('_'))
        .unwrap_or(text)
        .trim_start()
}
