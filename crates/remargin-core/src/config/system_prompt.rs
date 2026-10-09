//! System-prompt walk-up resolver.
//!
//! Mirrors the identity walk in [`crate::config::identity::resolve_from_walk`]
//! but anchors on the `system_prompt:` block of a `.remargin.yaml` rather than
//! the identity fields. The walk is identity-free on purpose: a folder's
//! prompt is a property of the directory tree, not the caller.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Serialize;

use crate::config::{CONFIG_FILENAME, Config};

/// The `<files>` token is substituted by the caller; the resolver returns the body verbatim.
pub const DEFAULT_PROMPT_BODY: &str =
    "Please process the comments in <files> using the remargin skill";

/// Resolved `system_prompt:` answer for one walk.
///
/// `source` is `None` and `is_default` is `true` only when the walk found no `system_prompt:`
/// block at all.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ResolvedSystemPrompt {
    pub is_default: bool,
    /// The YAML `name:`, else the owning folder's basename; `"default"` for the fallback.
    pub name: String,
    pub prompt: String,
    /// `None` means the caller's default runner.
    pub runner: Option<String>,
    /// `None` for the fallback.
    pub source: Option<PathBuf>,
}

/// Walk upward from `file_path`'s parent directory looking for the
/// nearest `.remargin.yaml` that declares a `system_prompt:` block.
/// Falls through to [`DEFAULT_PROMPT_BODY`] when the walk exhausts.
///
/// `file_path` itself need not exist on disk; only its parent chain is
/// walked. Callers wanting to resolve for a directory can pass the
/// directory path directly — the function treats an extension-less path
/// as the starting directory.
///
/// # Errors
///
/// Returns an error only when a candidate `.remargin.yaml` exists but
/// cannot be read or parsed. A missing file is not an error; an
/// exhausted walk is not an error.
pub fn resolve_system_prompt(
    system: &dyn System,
    file_path: &Path,
) -> Result<ResolvedSystemPrompt> {
    let start_dir = if file_path.extension().is_some() {
        file_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    } else {
        file_path.to_path_buf()
    };

    let mut current = start_dir;
    loop {
        let candidate = current.join(CONFIG_FILENAME);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            let content = system
                .read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let config: Config = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", candidate.display()))?;

            if let Some(sp) = config.system_prompt {
                let name = sp
                    .name
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| folder_name_for(&current));
                if sp.runner.as_deref().is_some_and(|r| r.contains('\n')) {
                    bail!("runner must be a single line (in {})", candidate.display());
                }
                return Ok(ResolvedSystemPrompt {
                    is_default: false,
                    name,
                    prompt: sp.prompt,
                    runner: sp.runner,
                    source: Some(candidate),
                });
            }
        }
        if !current.pop() {
            break;
        }
    }

    Ok(ResolvedSystemPrompt {
        is_default: true,
        name: "default".to_owned(),
        prompt: DEFAULT_PROMPT_BODY.to_owned(),
        runner: None,
        source: None,
    })
}

/// Derive a display name from the owning folder when `name:` is absent.
/// Empty / `.` / root all fall back to `"prompt"`.
fn folder_name_for(dir: &Path) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != ".")
        .map_or_else(|| "prompt".to_owned(), str::to_owned)
}

/// Render a [`ResolvedSystemPrompt`] as human-readable text.
///
/// Output goes to stderr in the CLI; the function returns a `String`
/// so callers can route it to any sink. `target` is the file path
/// the resolution was run for (for the header line).
#[must_use]
pub fn render_resolved_prompt(target: &Path, resolved: &ResolvedSystemPrompt) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Resolved prompt for: {}", target.display());
    let _ = writeln!(out, "  Name:    {}", resolved.name);
    if let Some(runner) = &resolved.runner {
        let _ = writeln!(out, "  Runner:  {runner}");
    }
    match &resolved.source {
        Some(path) => {
            let _ = writeln!(out, "  Source:  {}", path.display());
        }
        None => {
            let _ = writeln!(out, "  Source:  (walk exhausted)");
        }
    }
    let _ = writeln!(
        out,
        "  Default: {}",
        if resolved.is_default { "yes" } else { "no" },
    );
    let _ = writeln!(out, "  Body ({} chars):", resolved.prompt.chars().count());
    if resolved.prompt.is_empty() {
        let _ = writeln!(out, "    (empty)");
    } else {
        for line in resolved.prompt.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    out
}

#[cfg(test)]
mod tests;
