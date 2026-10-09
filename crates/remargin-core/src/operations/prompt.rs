//! Typed mutation surface for the `system_prompt:` block.
//!
//! Lives outside the document access layer so it can bypass
//! `writer::ensure_not_forbidden_target` (which blocks every other
//! writer from touching `.remargin.yaml`). The bypass is earned by a
//! narrow contract: only the `system_prompt:` mapping is added,
//! replaced, or removed. A post-splice diff parses both pre- and
//! post-write YAML and refuses the write when any other top-level
//! field would have changed — defence-in-depth against the splice
//! string transform doing something the regex didn't anticipate.
//!
//! The pre-mutate op-guard (`pre_mutate_check_for_caller`) still
//! runs. `restrict` policies that deny `write` on the target folder
//! therefore deny prompt-set / prompt-delete too, since the new ops
//! identify themselves as `write` to the guard — finer-grained op
//! names would require enlarging the permission model, which is out
//! of scope here.

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::config::ResolvedConfig;
use crate::permissions::op_guard::pre_mutate_check_for_caller;

/// Result of a successful `delete` write.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PromptDeleteOutcome {
    /// The file had no `system_prompt:` block, so the call was a no-op.
    pub absent: bool,
    /// The strip left the file with no meaningful content; it stays in place regardless.
    pub left_empty: bool,
    pub source: PathBuf,
}

/// One entry from a recursive `list` walk.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PromptListEntry {
    pub folder: PathBuf,
    /// `None` leaves the display name to the caller, which falls back to the folder basename.
    pub name: Option<String>,
    pub prompt: String,
    /// `None` means the caller's default runner.
    pub runner: Option<String>,
    pub source: PathBuf,
}

/// Result of a successful `set` write.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PromptSetOutcome {
    pub created: bool,
    /// The splice produced byte-identical content, so nothing was written.
    pub noop: bool,
    pub source: PathBuf,
}

/// Byte range of the `system_prompt:` block inside a `.remargin.yaml`, end exclusive.
pub(crate) struct BlockRange {
    pub end: usize,
    pub start: usize,
}

/// The YAML text after a splice, and whether it equals the text before it.
pub(crate) struct SpliceResult {
    pub content: String,
    pub noop: bool,
}

/// The fields rendered into a `system_prompt:` block.
pub(crate) struct SystemPromptBlock<'block> {
    pub name: &'block str,
    pub prompt: &'block str,
    /// Empty = absent (no `runner:` line rendered).
    pub runner: &'block str,
}

/// Strip the `system_prompt:` block from `<folder>/.remargin.yaml`.
/// Idempotent: a missing block (or missing file) succeeds.
///
/// # Errors
///
/// Returns an error when `folder` is not a directory, the op-guard refuses, the splice changed
/// a field other than `system_prompt:`, or the write fails.
pub fn delete(
    system: &dyn System,
    folder: &Path,
    config: &ResolvedConfig,
) -> Result<PromptDeleteOutcome> {
    ensure_folder(system, folder)?;
    let target = folder.join(".remargin.yaml");

    let file_present = system
        .exists(&target)
        .with_context(|| format!("checking existence of {}", target.display()))?;
    if !file_present {
        return Ok(PromptDeleteOutcome {
            absent: true,
            left_empty: false,
            source: target,
        });
    }

    pre_mutate_check_for_caller(system, "write", &target, &config.caller_info())
        .with_context(|| format!("op-guard refused write to {}", target.display()))?;

    let existing = system
        .read_to_string(&target)
        .with_context(|| format!("reading {}", target.display()))?;
    let result = remove_system_prompt(&existing);
    if result.noop {
        return Ok(PromptDeleteOutcome {
            absent: true,
            left_empty: existing.trim().is_empty(),
            source: target,
        });
    }
    diff_only_system_prompt(&existing, &result.content)
        .context("post-splice diff: refusing write that changes fields other than system_prompt")?;
    system
        .write(&target, result.content.as_bytes())
        .with_context(|| format!("writing {}", target.display()))?;
    Ok(PromptDeleteOutcome {
        absent: false,
        left_empty: result.content.trim().is_empty(),
        source: target,
    })
}

/// Walk `root` recursively and return every `.remargin.yaml` that
/// declares a `system_prompt:` block. Files that fail to read or
/// parse are skipped silently — same posture as `sandbox list`.
///
/// # Errors
///
/// Returns an error only if `root` cannot be walked.
pub fn list(system: &dyn System, root: &Path) -> Result<Vec<PromptListEntry>> {
    // hidden=true: targets are `.remargin.yaml` dotfiles.
    let entries = system
        .walk_dir(root, false, true)
        .with_context(|| format!("walking directory {}", root.display()))?;

    let mut out = Vec::new();
    for entry in &entries {
        if !entry.is_file {
            continue;
        }
        if entry.path.file_name().and_then(|n| n.to_str()) != Some(".remargin.yaml") {
            continue;
        }
        let Ok(content) = system.read_to_string(&entry.path) else {
            continue;
        };
        let Ok(parsed) = serde_yaml::from_str::<serde_yaml::Value>(&content) else {
            continue;
        };
        let Some(map) = parsed.as_mapping() else {
            continue;
        };
        let Some(sp) = map.get(serde_yaml::Value::String(String::from("system_prompt"))) else {
            continue;
        };
        let Some(sp_map) = sp.as_mapping() else {
            continue;
        };
        let name = sp_map
            .get(serde_yaml::Value::String(String::from("name")))
            .and_then(|v| v.as_str())
            .map(String::from);
        let prompt = sp_map
            .get(serde_yaml::Value::String(String::from("prompt")))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();
        let runner = sp_map
            .get(serde_yaml::Value::String(String::from("runner")))
            .and_then(|v| v.as_str())
            .map(String::from);
        let folder = entry
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        out.push(PromptListEntry {
            folder,
            name,
            prompt,
            runner,
            source: entry.path.clone(),
        });
    }
    out.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(out)
}

/// Set or replace the `system_prompt:` block in `<folder>/.remargin.yaml`.
///
/// `folder` MUST be a directory; passing a file is an error. The
/// op-guard runs against the resolved `.remargin.yaml` path; the
/// `FORBIDDEN_TARGETS` guard is bypassed because this is the typed
/// surface designed to make exactly that write safe.
///
/// # Errors
///
/// Returns an error when `folder` is not a directory, the op-guard refuses, the existing YAML
/// does not parse, the splice changed a field other than `system_prompt:`, or the write fails.
pub fn set(
    system: &dyn System,
    folder: &Path,
    name: Option<&str>,
    body: &str,
    runner: Option<&str>,
    config: &ResolvedConfig,
) -> Result<PromptSetOutcome> {
    ensure_folder(system, folder)?;
    if runner.is_some_and(|r| r.contains('\n')) {
        bail!("runner must be a single line");
    }
    let target = folder.join(".remargin.yaml");

    pre_mutate_check_for_caller(system, "write", &target, &config.caller_info())
        .with_context(|| format!("op-guard refused write to {}", target.display()))?;

    let created = !system
        .exists(&target)
        .with_context(|| format!("checking existence of {}", target.display()))?;
    let existing = read_or_empty(system, &target)?;
    let result = splice_system_prompt(
        &existing,
        &SystemPromptBlock {
            name: name.unwrap_or(""),
            prompt: body,
            runner: runner.unwrap_or(""),
        },
    );
    if result.noop {
        return Ok(PromptSetOutcome {
            created: false,
            noop: true,
            source: target,
        });
    }
    diff_only_system_prompt(&existing, &result.content)
        .context("post-splice diff: refusing write that changes fields other than system_prompt")?;
    system
        .write(&target, result.content.as_bytes())
        .with_context(|| format!("writing {}", target.display()))?;
    Ok(PromptSetOutcome {
        created,
        noop: false,
        source: target,
    })
}

fn collapse_blank_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines: usize = 0;
    for ch in s.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push(ch);
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out
}

/// Refuses a rewrite that changed any top-level field other than `system_prompt`.
pub(crate) fn diff_only_system_prompt(old: &str, new: &str) -> Result<()> {
    let old_map = parse_top_mapping(old, "old")?;
    let new_map = parse_top_mapping(new, "new")?;
    let key = serde_yaml::Value::String(String::from("system_prompt"));
    let mut old_others = old_map;
    old_others.remove(&key);
    let mut new_others = new_map;
    new_others.remove(&key);
    if old_others != new_others {
        bail!("non-prompt field changed");
    }
    Ok(())
}

fn ensure_folder(system: &dyn System, folder: &Path) -> Result<()> {
    let exists = system
        .exists(folder)
        .with_context(|| format!("checking existence of {}", folder.display()))?;
    if !exists {
        bail!("folder does not exist: {}", folder.display());
    }
    if !system
        .is_dir(folder)
        .with_context(|| format!("checking directory: {}", folder.display()))?
    {
        bail!("path is not a directory: {}", folder.display());
    }
    Ok(())
}

/// The `system_prompt:` block: from its key line to the next unindented line, or to the end of
/// the text.
pub(crate) fn find_block_range(existing: &str) -> Option<BlockRange> {
    let lines: Vec<&str> = existing.split('\n').collect();
    let start_line = lines
        .iter()
        .position(|l| starts_with_system_prompt_key(l))?;
    let mut end_line = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(start_line + 1) {
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            end_line = idx;
            break;
        }
    }
    let mut start: usize = 0;
    for line in &lines[..start_line] {
        start += line.len() + 1;
    }
    let mut end = start;
    for line in &lines[start_line..end_line] {
        end += line.len() + 1;
    }
    if end > existing.len() {
        end = existing.len();
    }
    Some(BlockRange { end, start })
}

/// `true` when `s` holds a YAML-significant character or leading or trailing whitespace.
fn needs_block_style(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '#' | ':' | '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%' | '@' | '`'
        )
    }) || s.starts_with(char::is_whitespace)
        || s.ends_with(char::is_whitespace)
}

fn parse_top_mapping(content: &str, label: &str) -> Result<serde_yaml::Mapping> {
    if content.trim().is_empty() {
        return Ok(serde_yaml::Mapping::new());
    }
    let value: serde_yaml::Value = serde_yaml::from_str(content)
        .with_context(|| format!("parsing {label} .remargin.yaml as YAML"))?;
    match value {
        serde_yaml::Value::Null => Ok(serde_yaml::Mapping::new()),
        serde_yaml::Value::Mapping(m) => Ok(m),
        serde_yaml::Value::Bool(_)
        | serde_yaml::Value::Number(_)
        | serde_yaml::Value::String(_)
        | serde_yaml::Value::Sequence(_)
        | serde_yaml::Value::Tagged(_) => {
            bail!("{label} .remargin.yaml top level is not a mapping")
        }
    }
}

fn quote_scalar(s: &str) -> String {
    if !needs_block_style(s) && !s.contains('\n') {
        return String::from(s);
    }
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn read_or_empty(system: &dyn System, target: &Path) -> Result<String> {
    if !system
        .exists(target)
        .with_context(|| format!("checking existence of {}", target.display()))?
    {
        return Ok(String::new());
    }
    system
        .read_to_string(target)
        .with_context(|| format!("reading {}", target.display()))
}

pub(crate) fn remove_system_prompt(existing: &str) -> SpliceResult {
    let Some(range) = find_block_range(existing) else {
        return SpliceResult {
            content: String::from(existing),
            noop: true,
        };
    };
    let mut next = String::with_capacity(existing.len());
    next.push_str(&existing[..range.start]);
    next.push_str(&existing[range.end..]);
    // Collapse three or more newlines to two, so a removal leaves at most one blank line.
    let collapsed = collapse_blank_runs(&next);
    let trimmed = trim_trailing_blank_lines(&collapsed);
    let noop = trimmed == existing;
    SpliceResult {
        content: trimmed,
        noop,
    }
}

pub(crate) fn render_block(block: &SystemPromptBlock<'_>) -> String {
    let mut lines: Vec<String> = vec![String::from("system_prompt:")];
    if !block.name.is_empty() {
        lines.push(format!("  name: {}", quote_scalar(block.name)));
    }
    lines.extend(render_prompt_field(block.prompt));
    if !block.runner.is_empty() {
        lines.push(format!("  runner: {}", quote_scalar(block.runner)));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn render_prompt_field(prompt: &str) -> Vec<String> {
    if prompt.is_empty() {
        return vec![String::from(r#"  prompt: """#)];
    }
    if prompt.contains('\n') || needs_block_style(prompt) {
        let body = prompt.strip_suffix('\n').unwrap_or(prompt);
        let indented = body
            .split('\n')
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        return vec![String::from("  prompt: |"), indented];
    }
    vec![format!("  prompt: {}", quote_scalar(prompt))]
}

/// Replaces the existing `system_prompt:` block in place, or appends one after a blank line
/// when there is none.
pub(crate) fn splice_system_prompt(existing: &str, block: &SystemPromptBlock<'_>) -> SpliceResult {
    let rendered = render_block(block);
    if let Some(range) = find_block_range(existing) {
        let mut next = String::with_capacity(existing.len() + rendered.len());
        next.push_str(&existing[..range.start]);
        next.push_str(&rendered);
        next.push_str(&existing[range.end..]);
        let noop = next == existing;
        return SpliceResult {
            content: next,
            noop,
        };
    }
    let trimmed = existing.trim_end();
    let next = if trimmed.is_empty() {
        rendered
    } else {
        format!("{trimmed}\n\n{rendered}")
    };
    let noop = next == existing;
    SpliceResult {
        content: next,
        noop,
    }
}

pub(crate) fn starts_with_system_prompt_key(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("system_prompt") else {
        return false;
    };
    let after_ws = rest.trim_start_matches([' ', '\t']);
    after_ws.starts_with(':')
}

fn trim_trailing_blank_lines(s: &str) -> String {
    let mut out = String::from(s);
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests;
