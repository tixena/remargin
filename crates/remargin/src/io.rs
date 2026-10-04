//! IO plumbing: sink writers, JSON output helpers, stdin reading, path helpers.
//!
//! Extracted from `main.rs` so these leaf utilities can live (and eventually be
//! unit-tested) in isolation without bringing in the full CLI grammar.

use std::io::{self, Read as _, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_json::{Value, json};

use remargin_core::path::expand_path;

/// Global start time captured before argument parsing in `main`.
pub static START_TIME: OnceLock<Instant> = OnceLock::new();

/// Bundle of writers for the CLI's stdout / stderr streams.
///
/// Allows the `cmd_*` functions and `run()` to be exercised in-process by
/// tests with captured `Vec<u8>` buffers instead of writing to the real
/// process streams.
#[non_exhaustive]
pub struct IoSinks<'sinks> {
    pub stderr: &'sinks mut dyn Write,
    pub stdout: &'sinks mut dyn Write,
}

impl<'sinks> IoSinks<'sinks> {
    pub fn new(stdout: &'sinks mut dyn Write, stderr: &'sinks mut dyn Write) -> Self {
        Self { stderr, stdout }
    }
}

pub fn out(sinks: &mut IoSinks<'_>, msg: &str) -> Result<()> {
    writeln!(sinks.stdout, "{msg}").context("writing to stdout")
}

pub fn out_raw(sinks: &mut IoSinks<'_>, msg: &str) -> Result<()> {
    write!(sinks.stdout, "{msg}").context("writing to stdout")
}

/// Decorates object payloads with an `elapsed_ms` field so every `--json`
/// response carries timing info.
pub fn out_json(sinks: &mut IoSinks<'_>, value: &Value) -> Result<()> {
    let decorated = inject_elapsed_ms(value);
    out(
        sinks,
        &serde_json::to_string_pretty(&decorated).unwrap_or_default(),
    )
}

pub fn elapsed_ms() -> u64 {
    START_TIME.get().map_or(0, |t| {
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    })
}

/// Non-object values pass through unchanged so future non-object top-level
/// outputs are not silently corrupted.
pub fn inject_elapsed_ms(value: &Value) -> Value {
    if let Value::Object(map) = value {
        let mut new_map = map.clone();
        new_map.insert(String::from("elapsed_ms"), json!(elapsed_ms()));
        return Value::Object(new_map);
    }
    value.clone()
}

pub fn print_output(sinks: &mut IoSinks<'_>, json_mode: bool, value: &Value) -> Result<()> {
    if json_mode {
        out_json(sinks, value)
    } else {
        print_text_output(sinks, value)
    }
}

pub fn print_text_output(sinks: &mut IoSinks<'_>, value: &Value) -> Result<()> {
    match value {
        Value::String(s) => out(sinks, s),
        Value::Object(map) => {
            for (key, val) in map {
                if let Value::Array(arr) = val {
                    out(sinks, &format!("{key}:"))?;
                    for item in arr {
                        out(sinks, &format!("  {item}"))?;
                    }
                } else {
                    out(sinks, &format!("{key}: {val}"))?;
                }
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {
            out(sinks, &value.to_string())
        }
    }
}

pub fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("reading from stdin")?;
    Ok(buf)
}

/// Exactly one of `content` or `comment_file` must be provided. When
/// `comment_file` is `"-"`, the body is read from stdin.
pub fn resolve_comment_content(
    system: &dyn System,
    cwd: &Path,
    content: Option<&String>,
    comment_file: Option<&PathBuf>,
) -> Result<String> {
    match (content, comment_file) {
        (Some(text), None) => Ok(text.clone()),
        (None, Some(path)) => {
            let path_str = path.to_string_lossy();
            if path_str == "-" {
                read_stdin().context("reading comment body from stdin")
            } else {
                system
                    .read_to_string(&cwd.join(path))
                    .with_context(|| format!("reading comment body from {path_str}"))
            }
        }
        (None, None) => {
            anyhow::bail!("comment body required: provide as argument or via --comment-file")
        }
        (Some(_), Some(_)) => {
            anyhow::bail!("cannot use both positional content and --comment-file")
        }
    }
}

pub fn resolve_doc_path(system: &dyn System, cwd: &Path, file: &str) -> Result<PathBuf> {
    if file == "-" {
        let input = read_stdin()?;
        let temp_root = system
            .env_var("TMPDIR")
            .unwrap_or_else(|_err| String::from("/tmp"));
        let temp_path = PathBuf::from(temp_root).join("remargin-stdin.md");
        system
            .write(&temp_path, input.as_bytes())
            .context("writing stdin to temp file")?;
        Ok(temp_path)
    } else {
        let expanded = expand_cli_path(system, file)?;
        Ok(cwd.join(expanded))
    }
}

/// Expand a string-typed CLI path argument through [`expand_path`] and
/// surface a clear error naming the offending input. Downstream callers
/// layer their own path semantics (joining against `cwd`, validating that
/// the file exists, etc.) on top of the expanded `PathBuf`.
pub fn expand_cli_path(system: &dyn System, raw: &str) -> Result<PathBuf> {
    expand_path(system, raw).with_context(|| format!("expanding path argument {raw:?}"))
}

/// Resolve a path argument for the `purge` subcommand. In single-file
/// mode this funnels through [`resolve_doc_path`] (which honours stdin
/// `-`); in `--recursive` mode the path is treated as a directory, so
/// stdin redirection makes no sense and we just expand `~` / `$VAR`
/// before joining onto `cwd`.
pub fn resolve_purge_path(
    system: &dyn System,
    cwd: &Path,
    raw: &str,
    recursive: bool,
) -> Result<PathBuf> {
    if recursive {
        let expanded = expand_cli_path(system, raw)?;
        Ok(if expanded.is_absolute() {
            expanded
        } else {
            cwd.join(expanded)
        })
    } else {
        resolve_doc_path(system, cwd, raw)
    }
}

/// Same as [`expand_cli_path`] but for a `&Path`. Used by flags that clap
/// already parsed as [`PathBuf`] — we round-trip through `to_string_lossy`
/// so `~`, `$VAR`, etc. in the original arg still get expanded.
pub fn expand_cli_pathbuf(system: &dyn System, raw: &Path) -> Result<PathBuf> {
    let raw_str = raw.to_string_lossy();
    expand_cli_path(system, raw_str.as_ref())
}

pub fn truncate_content(content: &str, max_len: usize) -> String {
    let first_line = content.lines().next().unwrap_or("");
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        String::from(first_line)
    }
}

/// Parse the `--lines START-END` argument used by `remargin write`.
///
/// Accepts `START-END` with 1-indexed inclusive bounds, both required.
/// Returns `(start, end)`; further validation (start <= end, start >= 1)
/// happens in `document::write` so CLI and MCP callers hit the same
/// diagnostics.
pub fn parse_line_range(raw: &str) -> Result<(usize, usize)> {
    let (start_str, end_str) = raw
        .split_once('-')
        .with_context(|| format!("--lines expects START-END, got {raw:?}"))?;
    let start: usize = start_str
        .parse()
        .with_context(|| format!("--lines: invalid start value {start_str:?}"))?;
    let end: usize = end_str
        .parse()
        .with_context(|| format!("--lines: invalid end value {end_str:?}"))?;
    Ok((start, end))
}

#[cfg(test)]
mod tests;
