//! Embedded structural markdown linter.
//!
//! Validates that a markdown document is structurally sound before and after
//! every write operation. The linter is embedded (no external dependency),
//! structural-only (no style rules), and not configurable.

#[cfg(test)]
mod tests;

use core::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use os_shim::System;
use serde::Serialize;

use crate::config::permissions::resolve::lint_permissions_in_parents;
use crate::config::{Mode, ResolvedConfig, load_registry, resolve_mode};

const REQUIRED_REMARGIN_FIELDS: &[&str] = &["id", "author", "type", "ts", "checksum"];

/// A single lint error with its line number and message.
#[derive(Debug)]
#[non_exhaustive]
pub struct LintError {
    /// 1-indexed.
    pub line: usize,
    pub message: String,
}

/// Per-doc serialised view of a [`LintError`].
///
/// Shared by the CLI's `--json` mode and the MCP `lint` tool result.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct LintErrorView {
    /// 1-indexed.
    pub line: usize,
    pub message: String,
}

/// Combined lint result for a single doc.
///
/// Structural findings AND permissions-config findings discovered by
/// walking parents from the doc's directory. Consumers can render to
/// text or JSON; see [`LintReport::to_json`] and
/// [`LintReport::format_text`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LintReport {
    pub errors: Vec<LintErrorView>,

    /// Found in any `.remargin.yaml` on the parent walk from the doc's directory.
    pub permissions: Vec<PermissionsLintErrorView>,

    /// `to:` entries naming an absent or revoked participant; empty in open mode or with no
    /// registry.
    pub recipients: Vec<LintErrorView>,
}

/// Per-config serialised view of a [`crate::config::permissions::resolve::PermissionsLintError`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionsLintErrorView {
    /// 1-indexed; `None` when `serde_yaml` gave no location.
    pub column: Option<usize>,
    /// 1-indexed; `None` when `serde_yaml` gave no location.
    pub line: Option<usize>,
    pub message: String,
    pub source_file: PathBuf,
}

impl LintReport {
    /// Render the report as the human-facing CLI text.
    ///
    /// Returns the "no errors" placeholder when clean.
    #[must_use]
    pub fn format_text(&self) -> String {
        if self.is_clean() {
            return String::from("No lint errors.\n");
        }
        let mut buf = String::new();
        for err in &self.errors {
            let _ = writeln!(buf, "line {}: {}", err.line, err.message);
        }
        for finding in &self.permissions {
            let location = match (finding.line, finding.column) {
                (Some(line), Some(col)) => {
                    format!("{} (line {line}:{col})", finding.source_file.display())
                }
                (Some(line), None) => {
                    format!("{} (line {line})", finding.source_file.display())
                }
                _ => finding.source_file.display().to_string(),
            };
            let _ = writeln!(buf, "permissions: {location}: {}", finding.message);
        }
        for err in &self.recipients {
            let _ = writeln!(buf, "line {}: recipients: {}", err.line, err.message);
        }
        buf
    }

    /// `true` when neither structural, permissions, nor recipient findings exist.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.permissions.is_empty() && self.recipients.is_empty()
    }

    /// Render the report as the canonical `lint` JSON payload.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "errors": self.errors,
            "ok": self.is_clean(),
            "permissions": self.permissions,
            "recipients": self.recipients,
        })
    }
}

/// Run structural lint checks on a markdown document.
///
/// Returns a (possibly empty) list of structural issues found.
///
/// # Errors
///
/// Returns an error only on internal failures (not lint violations).
pub fn lint(content: &str) -> Result<Vec<LintError>> {
    let mut errors = Vec::new();

    check_unclosed_fences(content, &mut errors);
    check_yaml_frontmatter(content, &mut errors);
    check_remargin_blocks(content, &mut errors);

    Ok(errors)
}

/// Run the full lint pipeline for `doc_path`.
///
/// Structural lint over the file content + permissions-config lint over
/// every `.remargin.yaml` from the doc's directory up to `/`, plus
/// recipient registry validation for registered/strict mode realms.
///
/// # Errors
///
/// I/O failure reading the doc or walking the parent chain, or a refusal
/// from the realm's read gate.
pub fn lint_doc(
    system: &dyn System,
    doc_path: &Path,
    config: &ResolvedConfig,
) -> Result<LintReport> {
    use crate::parser;

    // A realm that fails to resolve is what the permissions pass below reports, so that failure
    // must not propagate here. A realm that resolves and refuses the caller still denies the read.
    if config
        .read_gate()
        .admits(system, doc_path)
        .is_ok_and(|admitted| !admitted)
    {
        config.ensure_can_read(system, doc_path)?;
    }

    let content = system
        .read_to_string(doc_path)
        .with_context(|| format!("reading {}", doc_path.display()))?;
    let structural = lint(&content)?;
    let walk_anchor = doc_path
        .parent()
        .map_or_else(|| doc_path.to_path_buf(), Path::to_path_buf);
    let permissions = lint_permissions_in_parents(system, &walk_anchor)?;

    // Only in registered/strict mode with a registry. Config errors are skipped here: the
    // permissions pass already reports them.
    let mut recipient_findings: Vec<LintErrorView> = Vec::new();
    if let Ok(resolved_mode) = resolve_mode(system, &walk_anchor)
        && matches!(resolved_mode.mode, Mode::Registered | Mode::Strict)
        && let Ok(Some(registry)) = load_registry(system, &walk_anchor)
    {
        let doc = parser::parse(&content)?;
        for cm in doc.comments() {
            for recipient in &cm.to {
                if !registry.is_active(recipient) {
                    recipient_findings.push(LintErrorView {
                        line: cm.line,
                        message: format!(
                            "recipient {recipient:?} is not an active registry participant"
                        ),
                    });
                }
            }
        }
    }

    Ok(LintReport {
        errors: structural
            .into_iter()
            .map(|err| LintErrorView {
                line: err.line,
                message: err.message,
            })
            .collect(),
        permissions: permissions
            .into_iter()
            .map(|finding| PermissionsLintErrorView {
                column: finding.column,
                line: finding.line,
                message: finding.message,
                source_file: finding.source_file,
            })
            .collect(),
        recipients: recipient_findings,
    })
}

/// Convenience: lint and fail if any errors found.
///
/// # Errors
///
/// Returns an error if any lint issues are detected, with all issues
/// formatted in the error message.
pub fn lint_or_fail(content: &str) -> Result<()> {
    let errors = lint(content)?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("Lint errors:\n{}", format_errors(&errors)))
    }
}

/// Check for unclosed fenced code blocks and fence depth mismatches.
///
/// A fence opener is a line starting with 3+ backticks. It is closed by a
/// line with exactly the same number of backticks (and nothing else on the
/// line besides optional whitespace). If a closer has fewer backticks than
/// the opener, it does not close the block.
fn check_unclosed_fences(content: &str, errors: &mut Vec<LintError>) {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut idx = 0;

    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim_start();
        let backtick_count = count_leading_backticks(trimmed);

        if backtick_count >= 3 && is_fence_opener(trimmed, backtick_count) {
            let opener_line = idx + 1;
            let opener_depth = backtick_count;

            let mut found_close = false;
            let mut inner_idx = idx + 1;
            while inner_idx < lines.len() {
                let inner_line = lines[inner_idx];
                let inner_trimmed = inner_line.trim_start();
                let inner_ticks = count_leading_backticks(inner_trimmed);

                if inner_ticks == opener_depth && is_fence_closer(inner_trimmed, inner_ticks) {
                    found_close = true;
                    idx = inner_idx + 1;
                    break;
                }
                inner_idx += 1;
            }

            if !found_close {
                errors.push(LintError {
                    line: opener_line,
                    message: format!(
                        "unclosed fenced code block (opened with {opener_depth} backticks)"
                    ),
                });
                idx += 1;
            }
        } else {
            idx += 1;
        }
    }
}

/// Check remargin blocks for valid YAML headers and required fields.
fn check_remargin_blocks(content: &str, errors: &mut Vec<LintError>) {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut idx = 0;

    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim_start();
        let backtick_count = count_leading_backticks(trimmed);

        if backtick_count >= 3 && is_fence_opener(trimmed, backtick_count) {
            let tag = trimmed[backtick_count..].trim();
            let opener_line = idx + 1;

            if tag == "remargin" {
                let mut close_idx = None;
                let mut inner_idx = idx + 1;
                while inner_idx < lines.len() {
                    let inner_trimmed = lines[inner_idx].trim_start();
                    let inner_ticks = count_leading_backticks(inner_trimmed);
                    if inner_ticks == backtick_count && is_fence_closer(inner_trimmed, inner_ticks)
                    {
                        close_idx = Some(inner_idx);
                        break;
                    }
                    inner_idx += 1;
                }

                if let Some(close) = close_idx {
                    let inner_lines = &lines[idx + 1..close];
                    validate_remargin_inner(inner_lines, opener_line, errors);
                    idx = close + 1;
                } else {
                    // Unclosed remargin block -- already caught by check_unclosed_fences.
                    idx += 1;
                }
            } else {
                let mut inner_idx = idx + 1;
                while inner_idx < lines.len() {
                    let inner_trimmed = lines[inner_idx].trim_start();
                    let inner_ticks = count_leading_backticks(inner_trimmed);
                    if inner_ticks == backtick_count && is_fence_closer(inner_trimmed, inner_ticks)
                    {
                        break;
                    }
                    inner_idx += 1;
                }
                idx = if inner_idx < lines.len() {
                    inner_idx + 1
                } else {
                    inner_idx
                };
            }
        } else {
            idx += 1;
        }
    }
}

/// Check YAML frontmatter validity.
///
/// If the document starts with `---`, the closing `---` must exist and
/// the content between must be valid YAML.
fn check_yaml_frontmatter(content: &str, errors: &mut Vec<LintError>) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return;
    }

    let lines: Vec<&str> = content.split('\n').collect();
    let first_line_idx = lines
        .iter()
        .position(|line| line.trim() == "---")
        .unwrap_or(0);

    let mut closing_idx = None;
    for (i, line) in lines.iter().enumerate().skip(first_line_idx + 1) {
        if line.trim() == "---" {
            closing_idx = Some(i);
            break;
        }
    }

    let Some(close_idx) = closing_idx else {
        errors.push(LintError {
            line: first_line_idx + 1,
            message: String::from("unclosed YAML frontmatter (no closing --- found)"),
        });
        return;
    };

    let yaml_lines: Vec<&str> = lines[first_line_idx + 1..close_idx].to_vec();
    let yaml_str = yaml_lines.join("\n");

    if let Err(err) = serde_yaml::from_str::<serde_yaml::Value>(&yaml_str) {
        errors.push(LintError {
            line: first_line_idx + 2,
            message: format!("invalid YAML in frontmatter: {err}"),
        });
    }
}

fn count_leading_backticks(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b'`').count()
}

fn format_errors(errors: &[LintError]) -> String {
    let mut out = String::new();
    for err in errors {
        let _ = writeln!(out, "  line {}: {}", err.line, err.message);
    }
    out
}

/// Determine if a line is a fence closer (only backticks and optional whitespace).
fn is_fence_closer(trimmed: &str, backtick_count: usize) -> bool {
    let rest = &trimmed[backtick_count..];
    rest.trim().is_empty()
}

/// Determine if a line is a fence opener (backticks followed by optional tag).
fn is_fence_opener(trimmed: &str, backtick_count: usize) -> bool {
    let rest = &trimmed[backtick_count..];
    !rest.contains('`')
}

/// Validate the inner content of a remargin block.
///
/// The inner content must have a `---` / `---` delimited YAML header with
/// all required fields.
fn validate_remargin_inner(inner_lines: &[&str], opener_line: usize, errors: &mut Vec<LintError>) {
    let yaml_start = inner_lines.iter().position(|line| line.trim() == "---");
    let Some(start) = yaml_start else {
        errors.push(LintError {
            line: opener_line,
            message: String::from("remargin block missing YAML header (no opening --- found)"),
        });
        return;
    };

    let yaml_end = inner_lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i);

    let Some(end) = yaml_end else {
        errors.push(LintError {
            line: opener_line + start + 1,
            message: String::from("remargin block YAML header not closed (no closing --- found)"),
        });
        return;
    };

    let yaml_lines: Vec<&str> = inner_lines[start + 1..end].to_vec();
    let yaml_str = yaml_lines.join("\n");

    let parsed: serde_yaml::Value = match serde_yaml::from_str(&yaml_str) {
        Ok(val) => val,
        Err(err) => {
            errors.push(LintError {
                line: opener_line + start + 2,
                message: format!("invalid YAML in remargin block header: {err}"),
            });
            return;
        }
    };

    let Some(mapping) = parsed.as_mapping() else {
        errors.push(LintError {
            line: opener_line + start + 2,
            message: String::from("remargin block YAML header is not a mapping"),
        });
        return;
    };

    for field in REQUIRED_REMARGIN_FIELDS {
        let key = serde_yaml::Value::String(String::from(*field));
        if !mapping.contains_key(&key) {
            errors.push(LintError {
                line: opener_line + start + 2,
                message: format!("remargin block missing required field: {field}"),
            });
        }
    }
}
