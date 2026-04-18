//! Remargin CLI binary.

#[cfg(feature = "obsidian")]
mod obsidian;

use std::env;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::Parser;
use os_shim::System;
use os_shim::real::RealSystem;
use serde_json::{Value, json};

use remargin_core::config::{self, CliOverrides, ResolvedConfig};
use remargin_core::display;
use remargin_core::document;
use remargin_core::linter;
use remargin_core::mcp;
use remargin_core::operations;
use remargin_core::operations::batch::BatchCommentOp;
use remargin_core::operations::migrate;
use remargin_core::operations::plan as plan_ops;
use remargin_core::operations::projections;
use remargin_core::operations::purge;
use remargin_core::operations::query;
use remargin_core::operations::sandbox as sandbox_ops;
use remargin_core::operations::search;
use remargin_core::parser;
use remargin_core::skill;
use remargin_core::writer::InsertPosition;

const EXIT_ERROR: u8 = 1;
const EXIT_LINT: u8 = 2;
const EXIT_INTEGRITY: u8 = 3;
const EXIT_ATTACHMENT: u8 = 4;
const EXIT_PRESERVATION: u8 = 5;
const EXIT_SKILL: u8 = 6;
const EXIT_NOT_FOUND: u8 = 7;
const EXIT_AMBIGUOUS: u8 = 8;

static START_TIME: OnceLock<Instant> = OnceLock::new();

#[derive(Parser)]
#[command(
    name = "remargin",
    version,
    about = "Enhanced inline review protocol for markdown"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[command(flatten)]
    global: GlobalFlags,
}

#[derive(clap::Args)]
#[cfg_attr(
    feature = "unrestricted",
    expect(
        clippy::struct_excessive_bools,
        reason = "CLI flags are naturally boolean; struct_excessive_bools is not relevant here"
    )
)]
struct GlobalFlags {
    /// Path to assets directory.
    #[arg(long)]
    assets_dir: Option<String>,

    /// Path to the config file.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Dry-run mode: preview changes without writing.
    #[arg(long)]
    dry_run: bool,

    /// Identity (author name) for this operation.
    #[arg(long)]
    identity: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    json: bool,

    /// Path to signing key.
    #[arg(long)]
    key: Option<String>,

    /// Enforcement mode: open, registered, or strict.
    #[arg(long, value_name = "open|registered|strict")]
    mode: Option<String>,

    /// Author type: human or agent.
    #[arg(long, value_name = "human|agent")]
    r#type: Option<String>,

    /// Bypass path sandbox checks (requires compile-time feature).
    #[cfg(feature = "unrestricted")]
    #[arg(long)]
    unrestricted: bool,

    /// Enable verbose/tracing output.
    #[arg(long)]
    verbose: bool,
}

/// Available subcommands.
#[derive(clap::Subcommand)]
enum Commands {
    /// Acknowledge one or more comments.
    Ack {
        /// Path to the document (use - for stdin). Omit to resolve by ID across the folder tree.
        #[arg(long)]
        file: Option<String>,
        /// Comment IDs to acknowledge.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Base directory to search when resolving by ID (default: .).
        #[arg(long, default_value = ".")]
        path: String,
        /// Remove this identity's ack instead of adding one.
        #[arg(long)]
        remove: bool,
    },
    /// Create multiple comments atomically (JSON ops via --ops).
    Batch {
        /// Path to the document.
        file: String,
        /// JSON array of operations.
        #[arg(long)]
        ops: String,
    },
    /// Create a comment in a document.
    Comment {
        /// Path to the document (use - for stdin).
        file: String,
        /// Comment body text (mutually exclusive with --comment-file).
        content: Option<String>,
        /// Insert after this comment ID.
        #[arg(long)]
        after_comment: Option<String>,
        /// Insert after this line number (1-indexed).
        #[arg(long)]
        after_line: Option<usize>,
        /// Attachments to include.
        #[arg(long)]
        attach: Vec<PathBuf>,
        /// Automatically acknowledge the parent comment when replying.
        #[arg(long)]
        auto_ack: bool,
        /// Read comment body from a file (use - for stdin).
        #[arg(long, short = 'F', conflicts_with = "content")]
        comment_file: Option<PathBuf>,
        /// ID of the comment to reply to.
        #[arg(long)]
        reply_to: Option<String>,
        /// Atomically stage the file in the caller's sandbox in the same write.
        #[arg(long)]
        sandbox: bool,
        /// Addressees of the comment.
        #[arg(long)]
        to: Vec<String>,
    },
    /// List comments in a document.
    Comments {
        /// Path to the document (use - for stdin).
        file: String,
        /// Pretty-print comments as a threaded tree.
        #[arg(long)]
        pretty: bool,
    },
    /// Delete one or more comments.
    Delete {
        /// Path to the document.
        file: String,
        /// Comment IDs to delete.
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Edit a comment (cascading ack clear).
    Edit {
        /// Path to the document.
        file: String,
        /// Comment ID to edit.
        id: String,
        /// New comment body.
        content: String,
    },
    /// Read a file's contents. Add `--binary` to fetch non-markdown files as
    /// bytes (base64 in `--json` mode, raw bytes to stdout otherwise, or
    /// written to `--out <path>`). Run `remargin metadata <path>` first to
    /// check `size_bytes` and `mime` before pulling large blobs.
    Get {
        /// Path to the file.
        path: String,
        /// Fetch as bytes. Rejects `.md` (use the text path for markdown).
        /// Mime is derived from the file extension.
        #[arg(long)]
        binary: bool,
        /// End line (1-indexed, inclusive). Text mode only.
        #[arg(long)]
        end: Option<usize>,
        /// Show line numbers in output. Text mode only.
        #[arg(short = 'n', long = "line-numbers")]
        line_numbers: bool,
        /// Write the fetched bytes to this path (binary mode only). Stdout
        /// receives a summary instead of the bytes. The caller names the
        /// target path — no auto-cleanup.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Start line (1-indexed). Text mode only.
        #[arg(long)]
        start: Option<usize>,
    },
    /// Resolve and print the identity config for a given type.
    ///
    /// Walks up from the current directory to find the first matching
    /// `.remargin.yaml`, filtered by `--type` (e.g. `human` or `agent`).
    /// Prints JSON to stdout. Useful for tooling that wants to detect the
    /// human's personal config without passing through all CLI flags.
    Identity {
        /// Author type filter: `human`, `agent`, etc. If omitted, returns the first config found.
        #[arg(long = "type")]
        author_type: Option<String>,
    },
    /// Generate a new Ed25519 signing key pair.
    Keygen {
        /// Output path for the private key (public key gets .pub suffix).
        #[arg(default_value = "remargin_key")]
        output: PathBuf,
    },
    /// Run structural lint checks.
    Lint {
        /// Path to the document (use - for stdin).
        file: String,
    },
    /// List files and directories.
    Ls {
        /// Directory path to list.
        #[arg(default_value = ".")]
        path: String,
    },
    /// MCP server management and execution.
    Mcp {
        /// Subcommand: run, install, uninstall, test.
        #[command(subcommand)]
        action: Option<McpAction>,
    },
    /// Get document metadata.
    Metadata {
        /// Path to the document.
        path: String,
    },
    /// Convert old-format comments to remargin format.
    Migrate {
        /// Path to the document.
        file: String,
        /// Create a .bak backup before modifying.
        #[arg(long)]
        backup: bool,
    },
    /// Install or uninstall the embedded Obsidian plugin in a vault.
    #[cfg(feature = "obsidian")]
    Obsidian {
        #[command(subcommand)]
        action: ObsidianAction,
    },
    /// Structured pre-commit prediction for a mutating op (rem-bhk).
    ///
    /// Per-op subcommand routing wires this to the in-memory projection
    /// of each mutating op. This crate ships the shared shape +
    /// subcommand tree (rem-2qr); individual op wiring lands in
    /// rem-imc, rem-3uo, rem-qll.
    Plan {
        /// Which mutating op to plan.
        #[command(subcommand)]
        action: PlanAction,
    },
    /// Strip all comments from a document.
    Purge {
        /// Path to the document.
        file: String,
    },
    /// Search across documents for comments.
    Query {
        /// Base directory to search.
        #[arg(default_value = ".")]
        path: String,
        /// Only documents with comments by this author.
        #[arg(long)]
        author: Option<String>,
        /// Only documents containing a comment with this structural ID.
        #[arg(long)]
        comment_id: Option<String>,
        /// Regex applied to comment content; composes with metadata filters.
        #[arg(long)]
        content_regex: Option<String>,
        /// Include individual matching comments in each result (default behavior).
        #[arg(long)]
        expanded: bool,
        /// Case-insensitive match for `--content-regex`.
        #[arg(long, short = 'i')]
        ignore_case: bool,
        /// Only documents with pending (unacked) comments.
        #[arg(long)]
        pending: bool,
        /// Only pending for this recipient.
        #[arg(long)]
        pending_for: Option<String>,
        /// Pretty-print results grouped by file.
        #[arg(long)]
        pretty: bool,
        /// Only activity after this ISO 8601 timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Return only counts/summary, suppress comment data.
        #[arg(long)]
        summary: bool,
    },
    /// Add or remove an emoji reaction.
    React {
        /// Path to the document.
        file: String,
        /// Comment ID.
        id: String,
        /// Emoji to add/remove.
        emoji: String,
        /// Remove instead of add.
        #[arg(long)]
        remove: bool,
    },
    /// Manage the registry file.
    Registry {
        /// Subcommand: show.
        #[command(subcommand)]
        action: RegistryAction,
    },
    /// Resolve the effective enforcement mode for a directory.
    ///
    /// Walks up from `--cwd` (or the current directory) looking for the
    /// nearest `.remargin.yaml` and returns its `mode:` field. Unlike
    /// `identity`, this does NOT filter by author type — mode is a
    /// directory-tree property. Falls back to `open` when no config is found.
    ///
    /// Prints a JSON object like `{"mode":"strict","source":"/path/to/.remargin.yaml"}`
    /// under `--json`; prints a short human-readable summary otherwise.
    ResolveMode {
        /// Starting directory for the walk-up. Defaults to the process's
        /// current directory.
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Remove a file from the managed document tree.
    Rm {
        /// Path to the file.
        file: String,
    },
    /// Manage per-identity sandbox staging for markdown files.
    Sandbox {
        /// Subcommand: add, list, or remove.
        #[command(subcommand)]
        action: SandboxAction,
    },
    /// Search across documents for text matches.
    Search {
        /// Text or regex pattern to search for.
        pattern: String,
        /// Base directory to search.
        #[arg(long, default_value = ".")]
        path: String,
        /// Treat pattern as a regex.
        #[arg(long)]
        regex: bool,
        /// Search scope: all, body, or comments.
        #[arg(long, default_value = "all")]
        scope: String,
        /// Lines of context around matches.
        #[arg(long, short = 'C', default_value = "0")]
        context: usize,
        /// Case-insensitive matching.
        #[arg(long, short = 'i')]
        ignore_case: bool,
    },
    /// Manage the Claude Code skill.
    Skill {
        /// Subcommand: install, uninstall, test.
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Verify comment integrity (checksums and signatures) against the
    /// participant registry.
    ///
    /// No flags: the registry is the single source of truth for pubkeys.
    /// Per-comment resolution runs unconditionally and the aggregate
    /// pass/fail follows the mode-driven severity table (see
    /// `operations::verify`).
    Verify {
        /// Path to the document.
        file: String,
    },
    /// Print version information.
    Version,
    /// Write document contents (comment-preserving).
    Write {
        /// Path to the file.
        path: String,
        /// File content to write (read from stdin if omitted).
        content: Option<String>,
        /// Content is base64-encoded binary data (implies --raw).
        /// Not supported for markdown (.md) files.
        #[arg(long)]
        binary: bool,
        /// Create a new file (parent directory must exist, file must not).
        #[arg(long)]
        create: bool,
        /// Replace only lines `START-END` (1-indexed, inclusive) and leave
        /// every other line byte-identical. Comment blocks inside the
        /// range must be reincluded (by id) in the replacement; writes
        /// that would destroy a comment are rejected. Incompatible with
        /// --create, --raw, and --binary.
        #[arg(long, value_name = "START-END")]
        lines: Option<String>,
        /// Write content exactly as provided, skipping frontmatter and comment
        /// preservation. Not supported for markdown (.md) files.
        #[arg(long)]
        raw: bool,
    },
}

/// Registry subcommands.
/// Plan subcommands (rem-bhk). One variant per mutating op; per-op
/// wiring is tracked under rem-imc / rem-3uo / rem-qll.
#[derive(clap::Subcommand)]
enum PlanAction {
    /// Project an `ack` op (rem-3uo).
    Ack {
        /// Path to the document.
        path: String,
        /// Comment IDs to ack (or un-ack with `--remove`).
        #[arg(required = true)]
        ids: Vec<String>,
        /// Remove the current identity's ack from each comment.
        #[arg(long)]
        remove: bool,
    },
    /// Project a `batch` op.
    Batch,
    /// Project a `comment` creation op.
    Comment,
    /// Project a `delete` op (rem-3uo).
    Delete {
        /// Path to the document.
        path: String,
        /// Comment IDs to delete.
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Project an `edit` op.
    Edit,
    /// Project a `migrate` op.
    Migrate,
    /// Project a `purge` op.
    Purge,
    /// Project a `react` op (rem-3uo).
    React {
        /// Path to the document.
        path: String,
        /// Comment ID to react to.
        id: String,
        /// Emoji to add (or remove with `--remove`).
        emoji: String,
        /// Remove the current identity's reaction with this emoji.
        #[arg(long)]
        remove: bool,
    },
    /// Project a `sandbox add` op.
    SandboxAdd,
    /// Project a `sandbox remove` op.
    SandboxRemove,
    /// Project a `write` op (rem-imc).
    Write {
        /// Path to the file.
        path: String,
        /// File content to write (read from stdin if omitted).
        content: Option<String>,
        /// Content is base64-encoded binary data (implies --raw). Not
        /// supported for markdown (.md) files and not representable as
        /// a structured plan — the report will carry a `reject_reason`.
        #[arg(long)]
        binary: bool,
        /// Create a new file (parent directory must exist, file must not).
        #[arg(long)]
        create: bool,
        /// Replace only lines `START-END` (1-indexed, inclusive) and
        /// leave every other line byte-identical. See `write --lines`
        /// for the full semantics.
        #[arg(long, value_name = "START-END")]
        lines: Option<String>,
        /// Write content exactly as provided, skipping frontmatter and
        /// comment preservation. Not supported for markdown (.md) files
        /// and not representable as a structured plan — the report will
        /// carry a `reject_reason`.
        #[arg(long)]
        raw: bool,
    },
}

#[derive(clap::Subcommand)]
enum RegistryAction {
    /// Show the current registry.
    Show,
}

/// Sandbox subcommands.
#[derive(clap::Subcommand)]
enum SandboxAction {
    /// Stage one or more markdown files in the caller's sandbox.
    Add {
        /// Markdown files to stage.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// List every markdown file staged for the caller.
    List {
        /// Emit absolute paths instead of paths relative to `--path`.
        #[arg(long)]
        absolute: bool,
        /// Base directory to walk (defaults to current directory).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Remove the caller's sandbox entry from one or more markdown files.
    Remove {
        /// Markdown files to unstage.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

/// Skill subcommands.
#[derive(clap::Subcommand)]
enum SkillAction {
    /// Install the Claude Code skill.
    Install {
        /// Install globally to ~/.claude/skills/remargin/.
        #[arg(long)]
        global: bool,
    },
    /// Check skill installation status.
    Test {
        /// Check global installation.
        #[arg(long)]
        global: bool,
    },
    /// Uninstall the skill.
    Uninstall {
        /// Uninstall from global location.
        #[arg(long)]
        global: bool,
    },
}

/// MCP subcommands.
#[derive(clap::Subcommand)]
enum McpAction {
    /// Register remargin as an MCP server in Claude Code.
    Install {
        /// Install at user scope (default is project scope).
        #[arg(long)]
        user: bool,
    },
    /// Start the MCP server (stdio transport). This is the default.
    Run,
    /// Check MCP registration status.
    Test,
    /// Remove remargin MCP server from Claude Code.
    Uninstall,
}

/// Obsidian plugin install/uninstall actions.
#[cfg(feature = "obsidian")]
#[derive(clap::Subcommand)]
enum ObsidianAction {
    /// Install or upgrade the plugin in the current vault.
    Install {
        /// Vault directory. Defaults to the current working directory.
        #[arg(long)]
        vault_path: Option<PathBuf>,
    },
    /// Remove the plugin from the current vault.
    Uninstall {
        /// Vault directory. Defaults to the current working directory.
        #[arg(long)]
        vault_path: Option<PathBuf>,
    },
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI flags are naturally boolean"
)]
struct CommentParams<'cmd> {
    after_comment: Option<&'cmd str>,
    after_line: Option<usize>,
    attachments: &'cmd [PathBuf],
    auto_ack: bool,
    content: &'cmd str,
    dry_run: bool,
    file: &'cmd str,
    json_mode: bool,
    reply_to: Option<&'cmd str>,
    sandbox: bool,
    to: &'cmd [String],
}

struct GetParams<'cmd> {
    binary: bool,
    end: Option<usize>,
    json_mode: bool,
    line_numbers: bool,
    out: Option<&'cmd Path>,
    path: &'cmd str,
    start: Option<usize>,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI flags are naturally boolean"
)]
struct QueryParams<'cmd> {
    author: Option<&'cmd str>,
    comment_id: Option<&'cmd str>,
    content_regex: Option<&'cmd str>,
    expanded: bool,
    ignore_case: bool,
    json_mode: bool,
    path: &'cmd str,
    pending: bool,
    pending_for: Option<&'cmd str>,
    pretty: bool,
    since: Option<&'cmd str>,
    summary: bool,
}

struct SearchParams<'cmd> {
    context: usize,
    ignore_case: bool,
    json_mode: bool,
    path: &'cmd str,
    pattern: &'cmd str,
    regex: bool,
    scope: &'cmd str,
}

struct AckParams<'cmd> {
    file: Option<&'cmd str>,
    ids: &'cmd [String],
    json_mode: bool,
    remove: bool,
    search_path: &'cmd str,
}

struct ReactParams<'cmd> {
    emoji: &'cmd str,
    file: &'cmd str,
    id: &'cmd str,
    json_mode: bool,
    remove: bool,
}

struct WriteParams<'cmd> {
    content: Option<&'cmd str>,
    json_mode: bool,
    opts: document::WriteOptions,
    path: &'cmd str,
}

fn out(msg: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{msg}").context("writing to stdout")
}

fn out_raw(msg: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "{msg}").context("writing to stdout")
}

/// Decorates object payloads with an `elapsed_ms` field so every `--json`
/// response carries timing info.
fn out_json(value: &Value) -> Result<()> {
    let decorated = inject_elapsed_ms(value);
    out(&serde_json::to_string_pretty(&decorated).unwrap_or_default())
}

fn elapsed_ms() -> u64 {
    START_TIME.get().map_or(0, |t| {
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    })
}

/// Non-object values pass through unchanged so future non-object top-level
/// outputs are not silently corrupted.
fn inject_elapsed_ms(value: &Value) -> Value {
    if let Value::Object(map) = value {
        let mut new_map = map.clone();
        new_map.insert(String::from("elapsed_ms"), json!(elapsed_ms()));
        return Value::Object(new_map);
    }
    value.clone()
}

fn print_output(json_mode: bool, value: &Value) -> Result<()> {
    if json_mode {
        out_json(value)
    } else {
        print_text_output(value)
    }
}

fn print_text_output(value: &Value) -> Result<()> {
    match value {
        Value::String(s) => out(s),
        Value::Object(map) => {
            for (key, val) in map {
                if let Value::Array(arr) = val {
                    out(&format!("{key}:"))?;
                    for item in arr {
                        out(&format!("  {item}"))?;
                    }
                } else {
                    out(&format!("{key}: {val}"))?;
                }
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {
            out(&value.to_string())
        }
    }
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("reading from stdin")?;
    Ok(buf)
}

/// Exactly one of `content` or `comment_file` must be provided. When
/// `comment_file` is `"-"`, the body is read from stdin.
fn resolve_comment_content(
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

fn resolve_doc_path(system: &dyn System, cwd: &Path, file: &str) -> Result<PathBuf> {
    if file == "-" {
        let input = read_stdin()?;
        let temp_path = env::temp_dir().join("remargin-stdin.md");
        system
            .write(&temp_path, input.as_bytes())
            .context("writing stdin to temp file")?;
        Ok(temp_path)
    } else {
        Ok(cwd.join(file))
    }
}

const fn author_type_str(at: &parser::AuthorType) -> &'static str {
    match at {
        parser::AuthorType::Human => "human",
        parser::AuthorType::Agent => "agent",
        _ => "unknown",
    }
}

fn truncate_content(content: &str, max_len: usize) -> String {
    let first_line = content.lines().next().unwrap_or("");
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        String::from(first_line)
    }
}

/// Parse the `--lines START-END` argument used by `remargin write` (rem-24p).
///
/// Accepts `START-END` with 1-indexed inclusive bounds, both required.
/// Returns `(start, end)`; further validation (start <= end, start >= 1)
/// happens in `document::write` so CLI and MCP callers hit the same
/// diagnostics.
fn parse_line_range(raw: &str) -> Result<(usize, usize)> {
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

fn main() -> ExitCode {
    // Capture the start time before parsing so `elapsed_ms` includes clap's
    // argument-parsing overhead.
    let _: Result<_, _> = START_TIME.set(Instant::now());

    let cli = Cli::parse();

    if cli.global.verbose {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive(tracing::Level::DEBUG.into()),
            )
            .with_writer(io::stderr)
            .init();
    }

    let system = RealSystem::new();
    let cwd = match system.current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("error: could not determine current directory: {err}");
            return ExitCode::from(EXIT_ERROR);
        }
    };

    let exit = match run(&cli, &system, &cwd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let exit_code = classify_error(&err);
            if cli.global.json {
                let error_json = inject_elapsed_ms(&json!({ "error": format!("{err:#}") }));
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&error_json).unwrap_or_default()
                );
            } else {
                eprintln!("error: {err:#}");
            }
            ExitCode::from(exit_code)
        }
    };

    // Non-JSON mode does not emit a timing footer on any stream (rem-26w):
    // stdout stays pure command output and stderr stays clean. The timing
    // value survives as `elapsed_ms` inside the JSON payload (rem-4ay).

    exit
}

fn classify_error(err: &anyhow::Error) -> u8 {
    let msg = format!("{err:#}");
    if msg.contains("Lint error") {
        EXIT_LINT
    } else if msg.contains("checksum") || msg.contains("signature") || msg.contains("integrity") {
        EXIT_INTEGRITY
    } else if msg.contains("attachment not found") {
        EXIT_ATTACHMENT
    } else if msg.contains("was removed") || msg.contains("preservation") {
        EXIT_PRESERVATION
    } else if msg.contains("skill") && msg.contains("not installed") {
        EXIT_SKILL
    } else if msg.contains("ambiguous: comment") {
        EXIT_AMBIGUOUS
    } else if msg.contains("not found") {
        EXIT_NOT_FOUND
    } else {
        EXIT_ERROR
    }
}

fn build_overrides(global: &GlobalFlags) -> CliOverrides<'_> {
    let mut overrides = CliOverrides::default();
    overrides.assets_dir = global.assets_dir.as_deref();
    overrides.author_type = global.r#type.as_deref();
    overrides.identity = global.identity.as_deref();
    overrides.key = global.key.as_deref();
    overrides.mode = global.mode.as_deref();
    overrides
}

fn run(cli: &Cli, system: &dyn System, cwd: &Path) -> Result<()> {
    let overrides = build_overrides(&cli.global);

    // Commands that do not need config.
    match &cli.command {
        Commands::Version => {
            eprintln!("remargin {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Commands::Mcp { action } => {
            return cmd_mcp(system, cwd, &overrides, action.as_ref(), cli.global.json);
        }
        Commands::Identity { author_type } => {
            return cmd_identity(system, cwd, author_type.as_deref(), cli.global.json);
        }
        Commands::ResolveMode { cwd: override_cwd } => {
            let start_dir = override_cwd.as_deref().unwrap_or(cwd);
            return cmd_resolve_mode(system, start_dir, cli.global.json);
        }
        Commands::Keygen { output } => return cmd_keygen(system, output),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { action } => {
            return cmd_obsidian(system, cwd, action, cli.global.json);
        }
        Commands::Skill { action } => return cmd_skill(system, action, cli.global.json),
        Commands::Ack { .. }
        | Commands::Batch { .. }
        | Commands::Comment { .. }
        | Commands::Comments { .. }
        | Commands::Delete { .. }
        | Commands::Edit { .. }
        | Commands::Get { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Metadata { .. }
        | Commands::Migrate { .. }
        | Commands::Plan { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::Rm { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Verify { .. }
        | Commands::Write { .. } => {}
    }

    // Load config and registry.
    // When --type is given without --identity, use it as a config selector:
    // walk up skips .remargin.yaml files whose type does not match.
    let type_filter = if cli.global.identity.is_none() {
        cli.global.r#type.as_deref()
    } else {
        None
    };
    let cfg = config::load_config_filtered(system, cwd, type_filter)?;
    if let Some(filter) = type_filter
        && cfg.is_none()
    {
        anyhow::bail!(
            "no .remargin.yaml with type {filter:?} found (searched from {} to /)",
            cwd.display()
        );
    }
    let registry = config::load_registry(system, cwd)?;
    #[cfg(not(feature = "unrestricted"))]
    let final_config = ResolvedConfig::resolve(system, cfg, registry, &overrides)?;

    #[cfg(feature = "unrestricted")]
    let final_config = {
        let mut c = ResolvedConfig::resolve(system, cfg, registry, &overrides)?;
        c.unrestricted = cli.global.unrestricted;
        c
    };

    dispatch_with_config(cli, system, cwd, &final_config)
}

#[expect(
    clippy::too_many_lines,
    reason = "dispatch function maps all CLI subcommands"
)]
fn dispatch_with_config(
    cli: &Cli,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let json_mode = cli.global.json;
    let dry_run = cli.global.dry_run;

    match &cli.command {
        Commands::Ack {
            file,
            ids,
            path,
            remove,
        } => {
            let ap = AckParams {
                file: file.as_deref(),
                ids,
                json_mode,
                remove: *remove,
                search_path: path,
            };
            cmd_ack(system, cwd, config, &ap)
        }
        Commands::Batch { file, ops } => cmd_batch(system, cwd, config, file, ops, json_mode),
        Commands::Comment {
            file,
            content,
            after_comment,
            after_line,
            attach,
            auto_ack,
            comment_file,
            reply_to,
            sandbox,
            to,
        } => {
            let resolved_content =
                resolve_comment_content(system, cwd, content.as_ref(), comment_file.as_ref())?;
            let cp = CommentParams {
                after_comment: after_comment.as_deref(),
                after_line: *after_line,
                attachments: attach,
                auto_ack: *auto_ack,
                content: &resolved_content,
                dry_run,
                file,
                json_mode,
                reply_to: reply_to.as_deref(),
                sandbox: *sandbox,
                to,
            };
            cmd_comment(system, cwd, config, &cp)
        }
        Commands::Comments { file, pretty } => cmd_comments(system, cwd, file, json_mode, *pretty),
        Commands::Delete { file, ids } => cmd_delete(system, cwd, config, file, ids, json_mode),
        Commands::Edit { file, id, content } => {
            cmd_edit(system, cwd, config, file, id, content, json_mode)
        }
        Commands::Get {
            path,
            binary,
            start,
            end,
            line_numbers,
            out,
        } => {
            let gp = GetParams {
                binary: *binary,
                end: *end,
                json_mode,
                line_numbers: *line_numbers,
                out: out.as_deref(),
                path,
                start: *start,
            };
            cmd_get(system, cwd, config, &gp)
        }
        Commands::Lint { file } => cmd_lint(system, cwd, file, json_mode),
        Commands::Ls { path } => cmd_ls(system, cwd, config, path, json_mode),
        Commands::Metadata { path } => cmd_metadata(system, cwd, config, path, json_mode),
        Commands::Migrate { file, backup } => {
            cmd_migrate(system, cwd, config, file, dry_run, *backup, json_mode)
        }
        Commands::Plan { action } => cmd_plan(system, cwd, config, action, json_mode),
        Commands::Purge { file } => cmd_purge(system, cwd, config, file, dry_run, json_mode),
        Commands::Query {
            path,
            author,
            comment_id,
            content_regex,
            expanded,
            ignore_case,
            pending,
            pending_for,
            pretty,
            since,
            summary,
        } => {
            let q = QueryParams {
                author: author.as_deref(),
                comment_id: comment_id.as_deref(),
                content_regex: content_regex.as_deref(),
                expanded: *expanded,
                ignore_case: *ignore_case,
                json_mode,
                path: path.as_str(),
                pending: *pending,
                pending_for: pending_for.as_deref(),
                pretty: *pretty,
                since: since.as_deref(),
                summary: *summary,
            };
            cmd_query(system, cwd, &q)
        }
        Commands::React {
            file,
            id,
            emoji,
            remove,
        } => {
            let r = ReactParams {
                emoji: emoji.as_str(),
                file: file.as_str(),
                id: id.as_str(),
                json_mode,
                remove: *remove,
            };
            cmd_react(system, cwd, config, &r)
        }
        Commands::Registry { action } => cmd_registry(system, cwd, action, json_mode),
        Commands::Rm { file } => cmd_rm(system, cwd, config, file, json_mode),
        Commands::Sandbox { action } => cmd_sandbox(system, cwd, config, action, json_mode),
        Commands::Search {
            pattern,
            path,
            regex,
            scope,
            context,
            ignore_case,
        } => {
            let s = SearchParams {
                context: *context,
                ignore_case: *ignore_case,
                json_mode,
                path: path.as_str(),
                pattern: pattern.as_str(),
                regex: *regex,
                scope: scope.as_str(),
            };
            cmd_search(system, cwd, &s)
        }
        Commands::Verify { file } => cmd_verify(system, cwd, file, config, json_mode),
        Commands::Write {
            path,
            content,
            binary,
            create,
            lines,
            raw,
        } => {
            let line_range = lines.as_deref().map(parse_line_range).transpose()?;
            cmd_write(
                system,
                cwd,
                config,
                &WriteParams {
                    content: content.as_deref(),
                    json_mode,
                    opts: document::WriteOptions::new()
                        .binary(*binary)
                        .create(*create)
                        .lines(line_range)
                        .raw(*raw),
                    path,
                },
            )
        }
        Commands::Version
        | Commands::Identity { .. }
        | Commands::Mcp { .. }
        | Commands::Keygen { .. }
        | Commands::ResolveMode { .. }
        | Commands::Skill { .. } => Ok(()),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => Ok(()),
    }
}

fn cmd_ack(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &AckParams<'_>,
) -> Result<()> {
    let AckParams {
        file,
        ids,
        json_mode,
        remove,
        search_path,
    } = *params;
    if let Some(doc_file) = file {
        let path = resolve_doc_path(system, cwd, doc_file)?;
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        operations::ack_comments(system, &path, config, &id_refs, remove)?;
    } else {
        let base_dir = cwd.join(search_path);
        for comment_id in ids {
            let matches = query::resolve_comment_id(system, &base_dir, comment_id)?;
            match matches.len() {
                0 => {
                    bail!("comment {comment_id:?} not found");
                }
                1 => {
                    let id_refs: Vec<&str> = vec![comment_id.as_str()];
                    operations::ack_comments(system, &matches[0], config, &id_refs, remove)?;
                }
                n => {
                    let file_list: Vec<String> =
                        matches.iter().map(|p| p.display().to_string()).collect();
                    bail!(
                        "ambiguous: comment {comment_id:?} found in {n} files: {}",
                        file_list.join(", ")
                    );
                }
            }
        }
    }
    let key = if remove {
        "unacknowledged"
    } else {
        "acknowledged"
    };
    print_output(json_mode, &json!({ key: ids }))
}

fn cmd_batch(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    ops_json: &str,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let ops_value: Vec<Value> =
        serde_json::from_str(ops_json).context("parsing batch operations JSON")?;

    let mut batch_ops = Vec::new();
    for (idx, op_value) in ops_value.iter().enumerate() {
        let op_obj = op_value
            .as_object()
            .with_context(|| format!("batch operation {idx}: expected object"))?;

        let content = op_obj
            .get("content")
            .and_then(Value::as_str)
            .with_context(|| format!("batch operation {idx}: missing content"))?;

        let to: Vec<String> = op_obj
            .get("to")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let reply_to = op_obj
            .get("reply_to")
            .and_then(Value::as_str)
            .map(String::from);
        let after_comment = op_obj
            .get("after_comment")
            .and_then(Value::as_str)
            .map(String::from);
        let after_line = op_obj
            .get("after_line")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok());
        let auto_ack = op_obj
            .get("auto_ack")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut op = BatchCommentOp::new(String::from(content));
        op.after_comment = after_comment;
        op.after_line = after_line;
        op.auto_ack = auto_ack;
        op.reply_to = reply_to;
        op.to = to;
        batch_ops.push(op);
    }

    let created_ids = operations::batch::batch_comment(system, &path, config, &batch_ops)?;
    print_output(json_mode, &json!({ "ids": created_ids }))
}

fn resolve_comment_position(
    reply_to: Option<&str>,
    after_comment: Option<&str>,
    after_line: Option<usize>,
) -> InsertPosition {
    reply_to.map_or_else(
        || {
            after_comment.map_or_else(
                || after_line.map_or(InsertPosition::Append, InsertPosition::AfterLine),
                |ac| InsertPosition::AfterComment(String::from(ac)),
            )
        },
        |parent_id| InsertPosition::AfterComment(String::from(parent_id)),
    )
}

fn cmd_comment(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    cp: &CommentParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, cp.file)?;

    // Replies always go after their parent — explicit placement is ignored.
    let position = resolve_comment_position(cp.reply_to, cp.after_comment, cp.after_line);

    if cp.dry_run {
        return print_output(cp.json_mode, &json!({ "dry_run": true, "file": cp.file }));
    }

    let mut params = operations::CreateCommentParams::new(cp.content, &position);
    params.attachments = cp.attachments;
    params.auto_ack = cp.auto_ack;
    params.reply_to = cp.reply_to;
    params.sandbox = cp.sandbox;
    params.to = cp.to;

    let new_id = operations::create_comment(system, &path, config, &params)?;

    // Write to stdout if stdin mode.
    if cp.file == "-" {
        let updated = system.read_to_string(&path)?;
        out_raw(&updated)?;
    }

    print_output(cp.json_mode, &json!({ "id": new_id }))
}

fn cmd_comments(
    system: &dyn System,
    cwd: &Path,
    file: &str,
    json_mode: bool,
    pretty: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let doc = parser::parse_file(system, &path)?;
    let comments = doc.comments();

    if pretty {
        let formatted = display::format_comments_pretty(file, &comments);
        out(&formatted)
    } else if json_mode {
        out_json(&json!({ "comments": comments }))
    } else {
        for cm in &comments {
            let ack_status = if cm.ack.is_empty() {
                "pending"
            } else {
                "acked"
            };
            out(&format!(
                "{} {} ({}) [{}] {}",
                cm.id,
                cm.author,
                author_type_str(&cm.author_type),
                ack_status,
                truncate_content(&cm.content, 60_usize),
            ))?;
        }
        Ok(())
    }
}

fn cmd_delete(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    ids: &[String],
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    operations::delete_comments(system, &path, config, &id_refs)?;
    print_output(json_mode, &json!({ "deleted": ids }))
}

fn cmd_edit(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    id: &str,
    content: &str,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    operations::edit_comment(system, &path, config, id, content)?;
    print_output(json_mode, &json!({ "edited": id }))
}

fn cmd_get(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    gp: &GetParams<'_>,
) -> Result<()> {
    let target = Path::new(gp.path);

    if gp.binary {
        return cmd_get_binary(system, cwd, config, gp, target);
    }

    if gp.out.is_some() {
        bail!("--out requires --binary");
    }

    let lines = match (gp.start, gp.end) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    };

    if gp.json_mode && gp.line_numbers {
        let content = document::get(system, cwd, target, lines, false, config.unrestricted)?;
        let start_num = lines.map_or(1, |(s, _)| s);
        let json_lines: Vec<Value> = content
            .split('\n')
            .enumerate()
            .map(|(i, text)| json!({ "line": start_num + i, "text": text }))
            .collect();
        print_output(true, &json!({ "lines": json_lines }))
    } else {
        let content = document::get(
            system,
            cwd,
            target,
            lines,
            gp.line_numbers,
            config.unrestricted,
        )?;
        if gp.json_mode {
            print_output(true, &json!({ "content": content }))
        } else {
            out_raw(&content)
        }
    }
}

/// Binary-mode `get` dispatch (rem-cdr). Reads bytes once through the shared
/// core helper, then surfaces them in the caller's chosen shape:
/// - `--out <path>` — write bytes to disk, stdout shows `{path, size_bytes, mime}`.
/// - `--json` — base64-encoded `content` in the payload alongside mime / size.
/// - default — raw bytes to stdout (so `remargin get --binary x.png > out.png` works).
///
/// Incompatible flags (`--start`, `--end`, `-n`) are rejected up front so
/// binary requests never silently drop text-mode options.
fn cmd_get_binary(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    gp: &GetParams<'_>,
    target: &Path,
) -> Result<()> {
    if gp.start.is_some() || gp.end.is_some() {
        bail!("--start / --end are not supported with --binary");
    }
    if gp.line_numbers {
        bail!("--line-numbers is not supported with --binary");
    }

    let payload = document::read_binary(system, cwd, target, config.unrestricted)?;

    if let Some(out_path) = gp.out {
        fs::write(out_path, &payload.bytes)
            .with_context(|| format!("writing {}", out_path.display()))?;
        let summary = json!({
            "mime": payload.mime,
            "out": out_path,
            "path": payload.path,
            "size_bytes": payload.size_bytes,
        });
        return print_output(gp.json_mode, &summary);
    }

    if gp.json_mode {
        let encoded = BASE64_STANDARD.encode(&payload.bytes);
        return print_output(
            true,
            &json!({
                "binary": true,
                "content": encoded,
                "mime": payload.mime,
                "path": payload.path,
                "size_bytes": payload.size_bytes,
            }),
        );
    }

    // Non-JSON, no --out: raw bytes to stdout so shell redirection works.
    io::stdout()
        .write_all(&payload.bytes)
        .context("writing bytes to stdout")
}

fn cmd_identity(
    system: &dyn System,
    cwd: &Path,
    author_type: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let found = config::load_config_filtered_with_path(system, cwd, author_type)?;
    let value = match &found {
        Some((path, cfg)) => json!({
            "found": true,
            "path": path.display().to_string(),
            "identity": cfg.identity,
            "author_type": cfg.author_type,
            "key": cfg.key,
            "mode": format!("{:?}", cfg.mode).to_lowercase(),
        }),
        None => json!({ "found": false }),
    };
    if json_mode {
        print_output(true, &value)?;
    } else {
        match &found {
            Some((path, cfg)) => {
                eprintln!("Found config: {}", path.display());
                if let Some(id) = &cfg.identity {
                    eprintln!("Identity:     {id}");
                }
                if let Some(t) = &cfg.author_type {
                    eprintln!("Type:         {t}");
                }
                if let Some(k) = &cfg.key {
                    eprintln!("Key:          {k}");
                }
            }
            None => eprintln!("No identity config found."),
        }
    }
    Ok(())
}

fn cmd_resolve_mode(system: &dyn System, cwd: &Path, json_mode: bool) -> Result<()> {
    let resolved = config::resolve_mode(system, cwd)?;
    let source = resolved.source.as_ref().map(|p| p.display().to_string());
    let value = json!({
        "mode": resolved.mode.as_str(),
        "source": source,
    });
    if json_mode {
        print_output(true, &value)?;
    } else {
        eprintln!("Mode:   {}", resolved.mode.as_str());
        match &source {
            Some(path) => eprintln!("Source: {path}"),
            None => eprintln!("Source: <default>"),
        }
    }
    Ok(())
}

fn cmd_keygen(system: &dyn System, output: &Path) -> Result<()> {
    use ssh_key::{Algorithm, LineEnding, PrivateKey, rand_core::OsRng};

    let private_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|err| anyhow::anyhow!("key generation failed: {err}"))?;

    let private_pem = private_key
        .to_openssh(LineEnding::LF)
        .map_err(|err| anyhow::anyhow!("encoding private key: {err}"))?;

    let public_key = private_key.public_key();
    let public_openssh = public_key
        .to_openssh()
        .map_err(|err| anyhow::anyhow!("encoding public key: {err}"))?;

    system
        .write(output, private_pem.as_bytes())
        .with_context(|| format!("writing private key to {}", output.display()))?;

    let pub_path = output.with_extension("pub");
    system
        .write(&pub_path, public_openssh.as_bytes())
        .with_context(|| format!("writing public key to {}", pub_path.display()))?;

    eprintln!("Private key: {}", output.display());
    eprintln!("Public key:  {}", pub_path.display());

    Ok(())
}

fn cmd_lint(system: &dyn System, cwd: &Path, file: &str, json_mode: bool) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let content = system
        .read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;

    let errors = linter::lint(&content)?;

    if json_mode {
        let results: Vec<Value> = errors
            .iter()
            .map(|err| json!({ "line": err.line, "message": err.message }))
            .collect();
        print_output(
            true,
            &json!({ "errors": results, "ok": results.is_empty() }),
        )?;
    } else if errors.is_empty() {
        eprintln!("No lint errors.");
    } else {
        for err in &errors {
            eprintln!("line {}: {}", err.line, err.message);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("Lint errors found");
    }
}

fn cmd_ls(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    path_str: &str,
    json_mode: bool,
) -> Result<()> {
    let target = Path::new(path_str);
    let entries = document::ls(system, cwd, target, config)?;

    if json_mode {
        print_output(true, &json!({ "entries": entries }))
    } else {
        for entry in &entries {
            let kind = if entry.is_dir { "d" } else { "-" };
            let size_str = entry
                .size
                .map_or_else(|| String::from("-"), |s| format!("{s}"));
            let pending_str = entry
                .remargin_pending
                .map(|p| format!(" [{p} pending]"))
                .unwrap_or_default();
            out(&format!(
                "{kind} {size_str:>8} {}{}",
                entry.path.display(),
                pending_str,
            ))?;
        }
        Ok(())
    }
}

fn cmd_metadata(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    path_str: &str,
    json_mode: bool,
) -> Result<()> {
    let target = Path::new(path_str);
    let meta = document::metadata(system, cwd, target, config.unrestricted)?;

    // File-level fields are always present. Markdown fields are only emitted
    // when the file was parsed (rem-lqz).
    let mut result = json!({
        "binary": meta.binary,
        "mime": meta.mime,
        "path": meta.path,
        "size_bytes": meta.size_bytes,
    });
    let map = result.as_object_mut().unwrap();
    if let Some(count) = meta.comment_count {
        map.insert("comment_count".into(), json!(count));
    }
    if let Some(count) = meta.line_count {
        map.insert("line_count".into(), json!(count));
    }
    if let Some(count) = meta.pending_count {
        map.insert("pending_count".into(), json!(count));
    }
    if !meta.pending_for.is_empty() {
        map.insert("pending_for".into(), json!(meta.pending_for));
    }
    if let Some(last) = &meta.last_activity {
        map.insert("last_activity".into(), json!(last));
    }

    print_output(json_mode, &result)
}

fn cmd_migrate(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    dry_run: bool,
    backup: bool,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let migrated = migrate::migrate(system, &path, config, dry_run, backup)?;

    if json_mode {
        let results: Vec<Value> = migrated
            .iter()
            .map(|m| json!({ "new_id": m.new_id, "original_role": m.original_role }))
            .collect();
        print_output(true, &json!({ "migrated": results, "dry_run": dry_run }))
    } else if migrated.is_empty() {
        eprintln!("No legacy comments found.");
        Ok(())
    } else {
        let label = if dry_run { "dry-run" } else { "migrated" };
        for m in &migrated {
            eprintln!("{} -> {} ({label})", m.original_role, m.new_id);
        }
        Ok(())
    }
}

/// Route a `plan` subcommand to the correct per-op projection.
///
/// Lightweight ops that have not yet been wired (tracked under rem-3uo /
/// rem-qll) surface a deliberate "not yet landed" error so callers
/// discover the subcommand tree and failures are loud. `plan write` is
/// fully wired per rem-imc.
fn cmd_plan(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    action: &PlanAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        PlanAction::Write {
            path,
            content,
            binary,
            create,
            lines,
            raw,
        } => {
            let body = match content {
                Some(s) => String::from(s),
                None => read_stdin()?,
            };
            let line_range = lines.as_deref().map(parse_line_range).transpose()?;
            let opts = document::WriteOptions::new()
                .binary(*binary)
                .create(*create)
                .lines(line_range)
                .raw(*raw);
            let projection = document::project_write(
                system,
                cwd,
                Path::new(path.as_str()),
                &body,
                config,
                opts,
            )?;
            let identity = build_plan_identity(config);
            let report = match projection {
                document::WriteProjection::Markdown {
                    before,
                    after,
                    noop,
                } => {
                    let mut report =
                        plan_ops::project_report("write", &before, &after, config, identity);
                    // `project_write` already performed the byte-identical
                    // shortcut; carry the flag through in case the caller
                    // wants the richer diagnostic than `checksum_before ==
                    // checksum_after` alone implies.
                    report.noop = report.noop || noop;
                    report
                }
                document::WriteProjection::Unsupported { reason } => {
                    // `--raw` / `--binary` cannot produce a structured
                    // markdown plan; return a degraded report with an
                    // explicit `reject_reason`.
                    let empty = parser::parse("").context("parsing empty document")?;
                    let mut report =
                        plan_ops::project_report("write", &empty, &empty, config, identity);
                    report.reject_reason = Some(reason);
                    report.would_commit = false;
                    report
                }
                _ => anyhow::bail!("unhandled WriteProjection variant"),
            };
            let value = serde_json::to_value(&report).context("serializing plan report")?;
            print_output(json_mode, &value)
        }
        PlanAction::Ack { path, ids, remove } => {
            let doc_path = resolve_doc_path(system, cwd, path)?;
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let (before, after) =
                projections::project_ack(system, &doc_path, config, &id_refs, *remove)?;
            emit_plan_report("ack", &before, &after, config, json_mode)
        }
        PlanAction::Delete { path, ids } => {
            let doc_path = resolve_doc_path(system, cwd, path)?;
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let (before, after) = projections::project_delete(system, &doc_path, config, &id_refs)?;
            emit_plan_report("delete", &before, &after, config, json_mode)
        }
        PlanAction::React {
            path,
            id,
            emoji,
            remove,
        } => {
            let doc_path = resolve_doc_path(system, cwd, path)?;
            let (before, after) =
                projections::project_react(system, &doc_path, config, id, emoji, *remove)?;
            emit_plan_report("react", &before, &after, config, json_mode)
        }
        PlanAction::Batch => bail_plan_not_yet_wired("batch"),
        PlanAction::Comment => bail_plan_not_yet_wired("comment"),
        PlanAction::Edit => bail_plan_not_yet_wired("edit"),
        PlanAction::Migrate => bail_plan_not_yet_wired("migrate"),
        PlanAction::Purge => bail_plan_not_yet_wired("purge"),
        PlanAction::SandboxAdd => bail_plan_not_yet_wired("sandbox-add"),
        PlanAction::SandboxRemove => bail_plan_not_yet_wired("sandbox-remove"),
    }
}

/// Shared helper: build a [`plan_ops::PlanReport`] from a `(before,
/// after)` pair and emit it through [`print_output`]. Used by every
/// lightweight plan op (rem-3uo).
fn emit_plan_report(
    op_label: &str,
    before: &parser::ParsedDocument,
    after: &parser::ParsedDocument,
    config: &ResolvedConfig,
    json_mode: bool,
) -> Result<()> {
    let identity = build_plan_identity(config);
    let report = plan_ops::project_report(op_label, before, after, config, identity);
    let value = serde_json::to_value(&report).context("serializing plan report")?;
    print_output(json_mode, &value)
}

/// Shared bail for the not-yet-wired plan actions (rem-bhk follow-ups).
fn bail_plan_not_yet_wired(op_label: &str) -> Result<()> {
    anyhow::bail!("plan {op_label}: per-op wiring not yet landed (tracked under rem-bhk)")
}

/// Build a [`plan_ops::PlanIdentity`] from the active resolved config.
///
/// `would_sign` is `true` when a key path is configured. We do not load
/// the key here — that would cost a disk read and/or a password prompt,
/// and per rem-bhk `plan` must stay side-effect-free.
fn build_plan_identity(config: &ResolvedConfig) -> plan_ops::PlanIdentity {
    let author_type = config.author_type.as_ref().map(|t| match t {
        parser::AuthorType::Agent => String::from("agent"),
        parser::AuthorType::Human => String::from("human"),
        _ => String::from("unknown"),
    });
    plan_ops::PlanIdentity::new(
        config.identity.clone(),
        author_type,
        config.key_path.is_some(),
    )
}

fn cmd_purge(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    dry_run: bool,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let result = purge::purge(system, &path, config, dry_run)?;

    print_output(
        json_mode,
        &json!({
            "comments_removed": result.comments_removed,
            "attachments_cleaned": result.attachments_cleaned,
            "dry_run": dry_run
        }),
    )
}

fn cmd_query(system: &dyn System, cwd: &Path, params: &QueryParams<'_>) -> Result<()> {
    let target = cwd.join(params.path);

    let since_dt = params
        .since
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .with_context(|| format!("invalid timestamp: {s}"))
        })
        .transpose()?;

    let mut filter = query::QueryFilter::default();
    filter.author = params.author.map(String::from);
    filter.comment_id = params.comment_id.map(String::from);
    filter.expanded = params.expanded;
    filter.pending = params.pending;
    filter.pending_for = params.pending_for.map(String::from);
    filter.since = since_dt;
    filter.summary = params.summary;
    if let Some(pattern) = params.content_regex {
        filter = filter.with_content_regex(pattern, params.ignore_case)?;
    }

    let results = query::query(system, &target, &filter)?;

    if params.json_mode {
        print_output(
            true,
            &json!({
                "base_path": format!("{}/", params.path.trim_end_matches('/')),
                "results": results,
            }),
        )
    } else if params.pretty {
        let filter_name = params.pending_for;
        let output = display::format_query_pretty(&results, filter_name);
        out_raw(&output)
    } else {
        for r in &results {
            out(&format!(
                "{} ({} comments, {} pending)",
                r.path.display(),
                r.comment_count,
                r.pending_count,
            ))?;
            for cm in &r.comments {
                let status = if cm.ack.is_empty() {
                    "pending"
                } else {
                    "acked"
                };
                let author_type = match cm.author_type {
                    parser::AuthorType::Agent => "agent",
                    parser::AuthorType::Human => "human",
                    _ => "unknown",
                };
                out(&format!(
                    "  {} {} ({}) [{}] {}",
                    cm.id, cm.author, author_type, status, cm.content,
                ))?;
            }
        }
        Ok(())
    }
}

fn cmd_search(system: &dyn System, cwd: &Path, params: &SearchParams<'_>) -> Result<()> {
    let target = cwd.join(params.path);

    let scope = match params.scope {
        "body" => search::SearchScope::Body,
        "comments" => search::SearchScope::Comments,
        _ => search::SearchScope::All,
    };

    let options = search::SearchOptions::new(String::from(params.pattern))
        .context_lines(params.context)
        .ignore_case(params.ignore_case)
        .regex(params.regex)
        .scope(scope);

    let results = search::search(system, cwd, &target, &options)?;

    if params.json_mode {
        print_output(true, &json!({ "matches": results }))
    } else {
        for m in &results {
            let loc = match m.location {
                search::MatchLocation::Body => "body",
                search::MatchLocation::Comment => "comment",
                _ => "unknown",
            };
            for line in &m.before {
                out(&format!("  {line}"))?;
            }
            out(&format!(
                "{}:{}  [{}]  {}",
                m.path.display(),
                m.line,
                loc,
                m.text
            ))?;
            for line in &m.after {
                out(&format!("  {line}"))?;
            }
        }
        Ok(())
    }
}

fn cmd_react(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &ReactParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, params.file)?;
    operations::react(
        system,
        &path,
        config,
        params.id,
        params.emoji,
        params.remove,
    )?;
    let action = if params.remove { "removed" } else { "added" };
    print_output(
        params.json_mode,
        &json!({ "action": action, "emoji": params.emoji, "comment_id": params.id }),
    )
}

/// Render a single registry participant as a JSON object for
/// `remargin registry show --json`. `display_name` always appears;
/// when absent in the registry it falls back to the participant id
/// so clients never have to handle a null value.
fn registry_participant_json(
    name: &str,
    participant: &config::registry::RegistryParticipant,
) -> Value {
    let status = match participant.status {
        config::registry::RegistryParticipantStatus::Active => "active",
        config::registry::RegistryParticipantStatus::Revoked => "revoked",
        _ => "unknown",
    };
    let display_name = participant
        .display_name
        .clone()
        .unwrap_or_else(|| String::from(name));
    json!({
        "name": name,
        "display_name": display_name,
        "type": participant.author_type,
        "status": status,
        "pubkeys": participant.pubkeys.len(),
    })
}

/// Render a single registry participant as a one-line string for
/// `remargin registry show`. When a display name is set, the prefix
/// is `"Display Name" (id)`; otherwise it's the bare id.
fn registry_participant_pretty(
    name: &str,
    participant: &config::registry::RegistryParticipant,
) -> String {
    let status = match participant.status {
        config::registry::RegistryParticipantStatus::Active => "active",
        config::registry::RegistryParticipantStatus::Revoked => "revoked",
        _ => "unknown",
    };
    let prefix = participant.display_name.as_ref().map_or_else(
        || String::from(name),
        |display| format!("\"{display}\" ({name})"),
    );
    format!(
        "{prefix} ({}) [{status}] {} key(s)",
        participant.author_type,
        participant.pubkeys.len(),
    )
}

fn cmd_registry(
    system: &dyn System,
    cwd: &Path,
    action: &RegistryAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        RegistryAction::Show => {
            let registry = config::load_registry(system, cwd)?.context("no registry found")?;

            if json_mode {
                let participants: Vec<Value> = registry
                    .participants
                    .iter()
                    .map(|(name, participant)| registry_participant_json(name, participant))
                    .collect();
                print_output(true, &json!({ "participants": participants }))
            } else {
                for (name, participant) in &registry.participants {
                    out(&registry_participant_pretty(name, participant))?;
                }
                Ok(())
            }
        }
    }
}

fn cmd_sandbox(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    action: &SandboxAction,
    json_mode: bool,
) -> Result<()> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required for sandbox operations")?;

    match action {
        SandboxAction::Add { files } => {
            let absolute: Vec<PathBuf> = files
                .iter()
                .map(|f| {
                    if f.is_absolute() {
                        f.clone()
                    } else {
                        cwd.join(f)
                    }
                })
                .collect();
            let result = sandbox_ops::add_to_files(system, &absolute, identity, config)?;
            emit_sandbox_bulk_result(&result, cwd, "added", json_mode)?;
            if result.failed.is_empty() {
                Ok(())
            } else {
                bail!("sandbox add: {} file(s) failed", result.failed.len())
            }
        }
        SandboxAction::Remove { files } => {
            let absolute: Vec<PathBuf> = files
                .iter()
                .map(|f| {
                    if f.is_absolute() {
                        f.clone()
                    } else {
                        cwd.join(f)
                    }
                })
                .collect();
            let result = sandbox_ops::remove_from_files(system, &absolute, identity, config)?;
            emit_sandbox_bulk_result(&result, cwd, "removed", json_mode)?;
            if result.failed.is_empty() {
                Ok(())
            } else {
                bail!("sandbox remove: {} file(s) failed", result.failed.len())
            }
        }
        SandboxAction::List { absolute, path } => {
            let root = path
                .as_ref()
                .map_or_else(|| cwd.to_path_buf(), |p| cwd.join(p));
            let listings = sandbox_ops::list_for_identity(system, &root, identity)?;

            if json_mode {
                let items: Vec<Value> = listings
                    .iter()
                    .map(|l| {
                        let display_path = if *absolute {
                            l.path.display().to_string()
                        } else {
                            l.path
                                .strip_prefix(&root)
                                .unwrap_or(&l.path)
                                .display()
                                .to_string()
                        };
                        json!({
                            "path": display_path,
                            "since": l.since.to_rfc3339(),
                        })
                    })
                    .collect();
                out_json(&json!({ "files": items }))
            } else {
                for l in &listings {
                    let display_path = if *absolute {
                        l.path.display().to_string()
                    } else {
                        l.path
                            .strip_prefix(&root)
                            .unwrap_or(&l.path)
                            .display()
                            .to_string()
                    };
                    out(&display_path)?;
                }
                Ok(())
            }
        }
    }
}

fn emit_sandbox_bulk_result(
    result: &sandbox_ops::SandboxBulkResult,
    cwd: &Path,
    changed_key: &str,
    json_mode: bool,
) -> Result<()> {
    if json_mode {
        let changed: Vec<String> = result
            .changed
            .iter()
            .map(|p| strip_prefix_display(p, cwd))
            .collect();
        let skipped: Vec<String> = result
            .skipped
            .iter()
            .map(|p| strip_prefix_display(p, cwd))
            .collect();
        let failed: Vec<Value> = result
            .failed
            .iter()
            .map(|f| {
                json!({
                    "path": strip_prefix_display(&f.path, cwd),
                    "reason": f.reason,
                })
            })
            .collect();
        out_json(&json!({
            changed_key: changed,
            "skipped": skipped,
            "failed": failed,
        }))?;
    } else {
        for p in &result.changed {
            out(&strip_prefix_display(p, cwd))?;
        }
        for failure in &result.failed {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(
                stderr,
                "{}: {}",
                strip_prefix_display(&failure.path, cwd),
                failure.reason,
            );
        }
    }
    Ok(())
}

fn strip_prefix_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn cmd_rm(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let target = Path::new(file);
    let result = document::rm(system, cwd, target, config)?;

    if json_mode {
        out_json(&json!({
            "deleted": file,
            "existed": result.existed,
        }))
    } else if result.existed {
        out(&format!("deleted: {file}"))
    } else {
        out(&format!("already absent: {file}"))
    }
}

#[cfg(feature = "obsidian")]
fn cmd_obsidian(
    system: &dyn System,
    cwd: &Path,
    action: &ObsidianAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        ObsidianAction::Install { vault_path } => {
            if !json_mode {
                eprintln!(
                    "Downloading remargin plugin v{} from GitHub Releases...",
                    obsidian::plugin_version()
                );
            }
            let report = obsidian::install(system, cwd, vault_path.as_deref())?;
            if json_mode {
                print_output(true, &report.to_json())
            } else {
                eprintln!("{}", report.to_text());
                Ok(())
            }
        }
        ObsidianAction::Uninstall { vault_path } => {
            let status = obsidian::uninstall(system, cwd, vault_path.as_deref())?;
            match status {
                obsidian::UninstallStatus::Removed { plugin_dir } => {
                    if json_mode {
                        print_output(
                            true,
                            &json!({
                                "uninstalled": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        eprintln!("Uninstalled remargin plugin from {}", plugin_dir.display());
                        Ok(())
                    }
                }
                obsidian::UninstallStatus::NotInstalled { plugin_dir } => {
                    if json_mode {
                        print_output(
                            true,
                            &json!({
                                "not_installed": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        eprintln!("remargin plugin not installed at {}", plugin_dir.display());
                        Ok(())
                    }
                }
            }
        }
    }
}

fn cmd_skill(system: &dyn System, action: &SkillAction, json_mode: bool) -> Result<()> {
    match action {
        SkillAction::Install { global } => {
            let path = skill::install(system, *global)?;
            if json_mode {
                print_output(true, &json!({ "installed": path.display().to_string() }))
            } else {
                eprintln!("Skill installed to {}", path.display());
                Ok(())
            }
        }
        SkillAction::Test { global } => {
            let status = skill::test_status(system, *global)?;
            let status_str = match status {
                skill::SkillStatus::NotInstalled => "not_installed",
                skill::SkillStatus::Outdated => "outdated",
                skill::SkillStatus::UpToDate => "up_to_date",
                _ => "unknown",
            };
            if json_mode {
                print_output(true, &json!({ "status": status_str }))
            } else {
                eprintln!("Skill status: {status_str}");
                Ok(())
            }
        }
        SkillAction::Uninstall { global } => {
            skill::uninstall(system, *global)?;
            if json_mode {
                print_output(true, &json!({ "uninstalled": true }))
            } else {
                eprintln!("Skill uninstalled.");
                Ok(())
            }
        }
    }
}

fn cmd_mcp(
    system: &dyn System,
    cwd: &Path,
    overrides: &config::CliOverrides<'_>,
    mcp_action: Option<&McpAction>,
    json_mode: bool,
) -> Result<()> {
    use std::process::Command;

    // Default to Run when no subcommand given (bare `remargin mcp`).
    match mcp_action {
        None | Some(McpAction::Run) => mcp::run(system, cwd, overrides),
        Some(McpAction::Install { user }) => {
            let bin = env::current_exe().context("resolving remargin binary path")?;
            let bin_str = bin.display().to_string();
            let scope = if *user { "user" } else { "project" };

            // Remove first to make the operation idempotent.
            let _: Result<_, _> = Command::new("claude")
                .args(["mcp", "remove", "remargin"])
                .output()
                .map(drop);

            let output = Command::new("claude")
                .args(["mcp", "add", "remargin", "-s", scope, "--", &bin_str, "mcp"])
                .output()
                .context("running 'claude mcp add' -- is Claude Code CLI installed?")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("claude mcp add failed: {stderr}");
            }

            if json_mode {
                print_output(
                    true,
                    &json!({
                        "installed": true,
                        "scope": scope,
                        "binary": bin_str,
                    }),
                )
            } else {
                eprintln!("MCP server registered ({scope} scope): {bin_str}");
                Ok(())
            }
        }
        Some(McpAction::Uninstall) => {
            let output = Command::new("claude")
                .args(["mcp", "remove", "remargin"])
                .output()
                .context("running 'claude mcp remove' -- is Claude Code CLI installed?")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("claude mcp remove failed: {stderr}");
            }

            if json_mode {
                print_output(true, &json!({ "uninstalled": true }))
            } else {
                eprintln!("MCP server unregistered.");
                Ok(())
            }
        }
        Some(McpAction::Test) => {
            let output = Command::new("claude")
                .args(["mcp", "list"])
                .output()
                .context("running 'claude mcp list' -- is Claude Code CLI installed?")?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let registered = stdout.lines().any(|l| l.contains("remargin"));
            let status_str = if registered {
                "registered"
            } else {
                "not_registered"
            };

            if json_mode {
                print_output(true, &json!({ "status": status_str }))
            } else {
                eprintln!("MCP status: {status_str}");
                Ok(())
            }
        }
    }
}

fn cmd_verify(
    system: &dyn System,
    cwd: &Path,
    file: &str,
    config: &ResolvedConfig,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let doc = parser::parse_file(system, &path)?;

    let report = operations::verify::verify_document(&doc, config);
    let results: Vec<Value> = report
        .results
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "checksum_ok": row.checksum_ok,
                "signature": row.signature.as_str(),
            })
        })
        .collect();

    if json_mode {
        print_output(true, &json!({ "results": results, "ok": report.ok }))?;
    } else {
        for row in &report.results {
            let chk = if row.checksum_ok { "ok" } else { "FAIL" };
            out(&format!(
                "{}: checksum={} signature={}",
                row.id,
                chk,
                row.signature.as_str(),
            ))?;
        }
    }

    if report.ok {
        Ok(())
    } else {
        anyhow::bail!("integrity check failed");
    }
}

fn cmd_write(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    wp: &WriteParams<'_>,
) -> Result<()> {
    let target = Path::new(wp.path);

    let body = match wp.content {
        Some(s) => String::from(s),
        None => read_stdin()?,
    };

    let outcome = document::write(system, cwd, target, &body, config, wp.opts)?;

    // A no-op prints a one-line human message in text mode instead of
    // the usual "written: ... / binary: ... / raw: ..." block; JSON mode
    // still returns a single payload, now with `noop: true` alongside
    // the existing fields so callers can branch on it (rem-1f2).
    if outcome.noop && !wp.json_mode {
        return out(&format!("{}: no changes (already up to date)", wp.path));
    }

    print_output(
        wp.json_mode,
        &json!({
            "written": wp.path,
            "binary": wp.opts.binary,
            "raw": wp.opts.raw || wp.opts.binary,
            "noop": outcome.noop,
        }),
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use os_shim::mock::MockSystem;
    use remargin_core::config;
    use remargin_core::config::registry::Registry;
    use serde_json::json;

    use super::{
        parse_line_range, registry_participant_json, registry_participant_pretty,
        resolve_comment_content,
    };

    #[test]
    fn parse_line_range_accepts_simple_pair() {
        let (s, e) = parse_line_range("10-20").unwrap();
        assert_eq!((s, e), (10, 20));
    }

    #[test]
    fn parse_line_range_accepts_single_line_range() {
        let (s, e) = parse_line_range("7-7").unwrap();
        assert_eq!((s, e), (7, 7));
    }

    #[test]
    fn parse_line_range_rejects_missing_dash() {
        let err = parse_line_range("100").unwrap_err();
        assert!(err.to_string().contains("START-END"));
    }

    #[test]
    fn parse_line_range_rejects_non_numeric() {
        let err = parse_line_range("a-b").unwrap_err();
        assert!(err.to_string().contains("invalid start value"));
    }

    #[test]
    fn parse_line_range_rejects_non_numeric_end() {
        let err = parse_line_range("1-b").unwrap_err();
        assert!(err.to_string().contains("invalid end value"));
    }

    #[test]
    fn content_from_positional_arg() {
        let system = MockSystem::new();
        let cwd = Path::new("/project");
        let content = String::from("Hello from arg");

        let result = resolve_comment_content(&system, cwd, Some(&content), None).unwrap();
        assert_eq!(result, "Hello from arg");
    }

    #[test]
    fn content_from_file() {
        let system = MockSystem::new()
            .with_file(Path::new("/project/comment.txt"), b"Hello from file")
            .unwrap();
        let cwd = Path::new("/project");
        let path = PathBuf::from("comment.txt");

        let result = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap();
        assert_eq!(result, "Hello from file");
    }

    #[test]
    fn content_from_absolute_file_path() {
        let system = MockSystem::new()
            .with_file(Path::new("/elsewhere/note.md"), b"Absolute path content")
            .unwrap();
        let cwd = Path::new("/project");
        let path = PathBuf::from("/elsewhere/note.md");

        let result = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap();
        assert_eq!(result, "Absolute path content");
    }

    #[test]
    fn error_when_neither_content_nor_file() {
        let system = MockSystem::new();
        let cwd = Path::new("/project");

        let err = resolve_comment_content(&system, cwd, None, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("comment body required"),
            "unexpected error: {msg}",
        );
    }

    #[test]
    fn error_when_file_not_found() {
        let system = MockSystem::new();
        let cwd = Path::new("/project");
        let path = PathBuf::from("missing.txt");

        let err = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("reading comment body from"),
            "unexpected error: {msg}",
        );
    }

    fn registry_with_yaml(yaml: &str) -> Registry {
        let system = MockSystem::new()
            .with_file(
                Path::new("/project/.remargin-registry.yaml"),
                yaml.as_bytes(),
            )
            .unwrap();
        config::load_registry(&system, Path::new("/project"))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn registry_json_includes_display_name_when_set() {
        let registry = registry_with_yaml(
            "\
participants:
  alice:
    display_name: \"Alice Doe\"
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
        );
        let alice = &registry.participants["alice"];
        let value = registry_participant_json("alice", alice);
        assert_eq!(
            value,
            json!({
                "name": "alice",
                "display_name": "Alice Doe",
                "type": "human",
                "status": "active",
                "pubkeys": 1_u64,
            })
        );
    }

    #[test]
    fn registry_json_falls_back_to_name_when_display_name_absent() {
        let registry = registry_with_yaml(
            "\
participants:
  ci-bot:
    type: agent
    status: active
    pubkeys: []
",
        );
        let bot = &registry.participants["ci-bot"];
        let value = registry_participant_json("ci-bot", bot);
        assert_eq!(
            value,
            json!({
                "name": "ci-bot",
                "display_name": "ci-bot",
                "type": "agent",
                "status": "active",
                "pubkeys": 0_u64,
            })
        );
    }

    #[test]
    fn registry_pretty_with_display_name() {
        let registry = registry_with_yaml(
            "\
participants:
  alice:
    display_name: \"Alice Doe\"
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
        );
        let alice = &registry.participants["alice"];
        assert_eq!(
            registry_participant_pretty("alice", alice),
            "\"Alice Doe\" (alice) (human) [active] 1 key(s)",
        );
    }

    #[test]
    fn registry_pretty_without_display_name() {
        let registry = registry_with_yaml(
            "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
        );
        let alice = &registry.participants["alice"];
        assert_eq!(
            registry_participant_pretty("alice", alice),
            "alice (human) [active] 1 key(s)",
        );
    }
}
