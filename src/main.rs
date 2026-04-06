//! # `Remargin`
//!
//! `Remargin` is a command-line tool for enhanced inline review of markdown documents.
//! It provides comment parsing, writing, threading, signatures, and cross-document queries.

use std::env;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::Parser;
use os_shim::System;
use os_shim::real::RealSystem;
use serde_json::{Value, json};

use remargin::config::{self, CliOverrides, ResolvedConfig};
use remargin::crypto;
use remargin::document;
use remargin::linter;
use remargin::mcp;
use remargin::operations;
use remargin::operations::batch::BatchCommentOp;
use remargin::operations::migrate;
use remargin::operations::purge;
use remargin::operations::query;
use remargin::parser;
use remargin::skill;
use remargin::writer::InsertPosition;

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

/// General error.
const EXIT_ERROR: u8 = 1;

/// Lint failure.
const EXIT_LINT: u8 = 2;

/// Integrity failure.
const EXIT_INTEGRITY: u8 = 3;

/// Missing attachment.
const EXIT_ATTACHMENT: u8 = 4;

/// Comment preservation violation.
const EXIT_PRESERVATION: u8 = 5;

/// Skill error.
const EXIT_SKILL: u8 = 6;

// ---------------------------------------------------------------------------
// CLI structure
// ---------------------------------------------------------------------------

/// Enhanced inline review protocol for markdown.
#[derive(Parser)]
#[command(
    name = "remargin",
    version,
    about = "Enhanced inline review protocol for markdown"
)]
struct Cli {
    /// Subcommand to execute.
    #[command(subcommand)]
    command: Commands,

    /// Global flags shared by all subcommands.
    #[command(flatten)]
    global: GlobalFlags,
}

/// Global flags that override config file values.
#[derive(clap::Args)]
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

    /// Enable verbose/tracing output.
    #[arg(long)]
    verbose: bool,
}

/// Available subcommands.
#[derive(clap::Subcommand)]
enum Commands {
    /// Acknowledge one or more comments.
    Ack {
        /// Path to the document (use - for stdin).
        file: String,
        /// Comment IDs to acknowledge.
        #[arg(required = true)]
        ids: Vec<String>,
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
        /// Comment body text.
        content: String,
        /// Insert after this comment ID.
        #[arg(long)]
        after_comment: Option<String>,
        /// Insert after this line number (1-indexed).
        #[arg(long)]
        after_line: Option<usize>,
        /// Attachments to include.
        #[arg(long)]
        attach: Vec<PathBuf>,
        /// ID of the comment to reply to.
        #[arg(long)]
        reply_to: Option<String>,
        /// Addressees of the comment.
        #[arg(long)]
        to: Vec<String>,
    },
    /// List comments in a document.
    Comments {
        /// Path to the document (use - for stdin).
        file: String,
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
    /// Read a file's contents.
    Get {
        /// Path to the file.
        path: String,
        /// End line (1-indexed, inclusive).
        #[arg(long)]
        end: Option<usize>,
        /// Start line (1-indexed).
        #[arg(long)]
        start: Option<usize>,
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
    /// Start the MCP server (stdio transport).
    Mcp,
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
        /// Only documents with pending (unacked) comments.
        #[arg(long)]
        pending: bool,
        /// Only pending for this recipient.
        #[arg(long)]
        pending_for: Option<String>,
        /// Only activity after this ISO 8601 timestamp.
        #[arg(long)]
        since: Option<String>,
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
    /// Manage the Claude Code skill.
    Skill {
        /// Subcommand: install, uninstall, test.
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Verify comment integrity (checksums and signatures).
    Verify {
        /// Path to the document.
        file: String,
        /// OpenSSH public key for signature verification.
        #[arg(long)]
        public_key: Option<String>,
    },
    /// Print version information.
    Version,
    /// Write document contents (comment-preserving).
    Write {
        /// Path to the file.
        path: String,
        /// File content to write (read from stdin if omitted).
        content: Option<String>,
    },
}

/// Registry subcommands.
#[derive(clap::Subcommand)]
enum RegistryAction {
    /// Show the current registry.
    Show,
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

// ---------------------------------------------------------------------------
// Parameter structs (to reduce argument count)
// ---------------------------------------------------------------------------

/// Parameters for the comment command.
struct CommentParams<'cmd> {
    /// Insert after this comment ID.
    after_comment: Option<&'cmd str>,
    /// Insert after this line number.
    after_line: Option<usize>,
    /// Attachments to include.
    attachments: &'cmd [PathBuf],
    /// Comment body text.
    content: &'cmd str,
    /// Dry-run mode.
    dry_run: bool,
    /// Path to the document.
    file: &'cmd str,
    /// JSON output mode.
    json_mode: bool,
    /// ID of the comment to reply to.
    reply_to: Option<&'cmd str>,
    /// Addressees.
    to: &'cmd [String],
}

/// Parameters for the query command.
struct QueryParams<'cmd> {
    /// Author filter.
    author: Option<&'cmd str>,
    /// JSON output mode.
    json_mode: bool,
    /// Base path to search.
    path: &'cmd str,
    /// Pending filter.
    pending: bool,
    /// Pending-for filter.
    pending_for: Option<&'cmd str>,
    /// Since timestamp filter.
    since: Option<&'cmd str>,
}

/// Parameters for the react command.
struct ReactParams<'cmd> {
    /// Emoji to add/remove.
    emoji: &'cmd str,
    /// Path to the document.
    file: &'cmd str,
    /// Comment ID.
    id: &'cmd str,
    /// JSON output mode.
    json_mode: bool,
    /// Remove instead of add.
    remove: bool,
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

/// Write a line to stdout.
fn out(msg: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{msg}").context("writing to stdout")
}

/// Write raw content to stdout (no trailing newline).
fn out_raw(msg: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "{msg}").context("writing to stdout")
}

/// Write JSON output to stdout.
fn out_json(value: &Value) -> Result<()> {
    out(&serde_json::to_string_pretty(value).unwrap_or_default())
}

/// Print output as JSON or text.
fn print_output(json_mode: bool, value: &Value) -> Result<()> {
    if json_mode {
        out_json(value)
    } else {
        print_text_output(value)
    }
}

/// Print a JSON value as human-readable text.
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

/// Read all of stdin to a string.
fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("reading from stdin")?;
    Ok(buf)
}

/// Resolve document path, supporting "-" for stdin.
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

/// Get the author type string from a comment.
const fn author_type_str(at: &parser::AuthorType) -> &'static str {
    match at {
        parser::AuthorType::Human => "human",
        parser::AuthorType::Agent => "agent",
        _ => "unknown",
    }
}

/// Convert a comment to a JSON value.
fn comment_to_json(cm: &parser::Comment) -> Value {
    let mut obj = json!({
        "id": cm.id,
        "author": cm.author,
        "type": author_type_str(&cm.author_type),
        "ts": cm.ts.to_rfc3339(),
        "checksum": cm.checksum,
        "content": cm.content,
    });
    let map = obj.as_object_mut().unwrap();
    if !cm.to.is_empty() {
        map.insert("to".into(), json!(cm.to));
    }
    if let Some(reply_to) = &cm.reply_to {
        map.insert("reply_to".into(), json!(reply_to));
    }
    if let Some(thread) = &cm.thread {
        map.insert("thread".into(), json!(thread));
    }
    if !cm.ack.is_empty() {
        let acks: Vec<Value> = cm
            .ack
            .iter()
            .map(|a| json!({ "author": a.author, "ts": a.ts.to_rfc3339() }))
            .collect();
        map.insert("ack".into(), json!(acks));
    }
    if !cm.reactions.is_empty() {
        map.insert("reactions".into(), json!(cm.reactions));
    }
    if !cm.attachments.is_empty() {
        map.insert("attachments".into(), json!(cm.attachments));
    }
    if let Some(sig) = &cm.signature {
        map.insert("signature".into(), json!(sig));
    }
    obj
}

/// Truncate content for display.
fn truncate_content(content: &str, max_len: usize) -> String {
    let first_line = content.lines().next().unwrap_or("");
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        String::from(first_line)
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
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

    match run(&cli, &system, &cwd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let exit_code = classify_error(&err);
            if cli.global.json {
                let error_json = json!({ "error": format!("{err:#}") });
                eprintln!("{error_json}");
            } else {
                eprintln!("error: {err:#}");
            }
            ExitCode::from(exit_code)
        }
    }
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// Classify an error into an exit code by inspecting the error chain.
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
    } else {
        EXIT_ERROR
    }
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

/// Build CLI overrides from global flags.
fn build_overrides(global: &GlobalFlags) -> CliOverrides<'_> {
    let mut overrides = CliOverrides::default();
    overrides.assets_dir = global.assets_dir.as_deref();
    overrides.author_type = global.r#type.as_deref();
    overrides.identity = global.identity.as_deref();
    overrides.key = global.key.as_deref();
    overrides.mode = global.mode.as_deref();
    overrides
}

/// Run the selected command.
fn run(cli: &Cli, system: &dyn System, cwd: &Path) -> Result<()> {
    let overrides = build_overrides(&cli.global);

    // Commands that do not need config.
    match &cli.command {
        Commands::Version => {
            eprintln!("remargin {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Commands::Mcp => return mcp::run(system, cwd, &overrides),
        Commands::Keygen { output } => return cmd_keygen(system, output),
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
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::Verify { .. }
        | Commands::Write { .. } => {}
    }

    // Load config and registry.
    let cfg = config::load_config(system, cwd)?;
    let registry = config::load_registry(system, cwd)?;
    let resolved = ResolvedConfig::resolve(system, cfg, registry, &overrides)?;

    dispatch_with_config(cli, system, cwd, &resolved)
}

/// Dispatch commands that require a loaded config.
fn dispatch_with_config(
    cli: &Cli,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let json_mode = cli.global.json;
    let dry_run = cli.global.dry_run;

    match &cli.command {
        Commands::Ack { file, ids } => cmd_ack(system, cwd, config, file, ids, json_mode),
        Commands::Batch { file, ops } => cmd_batch(system, cwd, config, file, ops, json_mode),
        Commands::Comment {
            file,
            content,
            after_comment,
            after_line,
            attach,
            reply_to,
            to,
        } => {
            let cp = CommentParams {
                after_comment: after_comment.as_deref(),
                after_line: *after_line,
                attachments: attach,
                content,
                dry_run,
                file,
                json_mode,
                reply_to: reply_to.as_deref(),
                to,
            };
            cmd_comment(system, cwd, config, &cp)
        }
        Commands::Comments { file } => cmd_comments(system, cwd, file, json_mode),
        Commands::Delete { file, ids } => cmd_delete(system, cwd, config, file, ids, json_mode),
        Commands::Edit { file, id, content } => {
            cmd_edit(system, cwd, config, file, id, content, json_mode)
        }
        Commands::Get { path, start, end } => cmd_get(system, cwd, path, *start, *end, json_mode),
        Commands::Lint { file } => cmd_lint(system, cwd, file, json_mode),
        Commands::Ls { path } => cmd_ls(system, cwd, config, path, json_mode),
        Commands::Metadata { path } => cmd_metadata(system, cwd, path, json_mode),
        Commands::Migrate { file, backup } => {
            cmd_migrate(system, cwd, config, file, dry_run, *backup, json_mode)
        }
        Commands::Purge { file } => cmd_purge(system, cwd, config, file, dry_run, json_mode),
        Commands::Query {
            path,
            author,
            pending,
            pending_for,
            since,
        } => {
            let q = QueryParams {
                author: author.as_deref(),
                json_mode,
                path: path.as_str(),
                pending: *pending,
                pending_for: pending_for.as_deref(),
                since: since.as_deref(),
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
        Commands::Verify { file, public_key } => {
            cmd_verify(system, cwd, file, public_key.as_deref(), json_mode)
        }
        Commands::Write { path, content } => {
            cmd_write(system, cwd, config, path, content.as_deref(), json_mode)
        }
        // Already handled in `run()`.
        Commands::Version | Commands::Mcp | Commands::Keygen { .. } | Commands::Skill { .. } => {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

fn cmd_ack(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    ids: &[String],
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    operations::ack_comments(system, &path, config, &id_refs)?;
    print_output(json_mode, &json!({ "acknowledged": ids }))
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

        let mut op = BatchCommentOp::new(String::from(content));
        op.after_comment = after_comment;
        op.after_line = after_line;
        op.reply_to = reply_to;
        op.to = to;
        batch_ops.push(op);
    }

    let created_ids = operations::batch::batch_comment(system, &path, config, &batch_ops)?;
    print_output(json_mode, &json!({ "ids": created_ids }))
}

fn cmd_comment(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    cp: &CommentParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, cp.file)?;

    let position = cp.after_comment.map_or_else(
        || {
            cp.after_line
                .map_or(InsertPosition::Append, InsertPosition::AfterLine)
        },
        |ac| InsertPosition::AfterComment(String::from(ac)),
    );

    if cp.dry_run {
        return print_output(cp.json_mode, &json!({ "dry_run": true, "file": cp.file }));
    }

    let mut params = operations::CreateCommentParams::new(cp.content, &position);
    params.attachments = cp.attachments;
    params.reply_to = cp.reply_to;
    params.to = cp.to;

    let new_id = operations::create_comment(system, &path, config, &params)?;

    // Write to stdout if stdin mode.
    if cp.file == "-" {
        let updated = system.read_to_string(&path)?;
        out_raw(&updated)?;
    }

    print_output(cp.json_mode, &json!({ "id": new_id }))
}

fn cmd_comments(system: &dyn System, cwd: &Path, file: &str, json_mode: bool) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let doc = parser::parse_file(system, &path)?;
    let comments = doc.comments();

    if json_mode {
        let result: Vec<Value> = comments.iter().map(|cm| comment_to_json(cm)).collect();
        out_json(&json!({ "comments": result }))
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
    path_str: &str,
    start: Option<usize>,
    end: Option<usize>,
    json_mode: bool,
) -> Result<()> {
    let target = Path::new(path_str);
    let lines = match (start, end) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    };
    let content = document::get(system, cwd, target, lines)?;
    if json_mode {
        print_output(true, &json!({ "content": content }))
    } else {
        out_raw(&content)
    }
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
        let results: Vec<Value> = entries
            .iter()
            .map(|entry| {
                let mut obj = json!({
                    "path": entry.path.display().to_string(),
                    "is_dir": entry.is_dir,
                });
                let map = obj.as_object_mut().unwrap();
                if let Some(size) = entry.size {
                    map.insert("size".into(), json!(size));
                }
                if let Some(pending) = entry.remargin_pending {
                    map.insert("remargin_pending".into(), json!(pending));
                }
                if let Some(last) = &entry.remargin_last_activity {
                    map.insert("remargin_last_activity".into(), json!(last));
                }
                obj
            })
            .collect();
        print_output(true, &json!({ "entries": results }))
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

fn cmd_metadata(system: &dyn System, cwd: &Path, path_str: &str, json_mode: bool) -> Result<()> {
    let target = Path::new(path_str);
    let meta = document::metadata(system, cwd, target)?;

    let mut result = json!({
        "comment_count": meta.comment_count,
        "line_count": meta.line_count,
        "pending_count": meta.pending_count,
    });
    let map = result.as_object_mut().unwrap();
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
    filter.pending = params.pending;
    filter.pending_for = params.pending_for.map(String::from);
    filter.since = since_dt;

    let results = query::query(system, &target, &filter)?;

    if params.json_mode {
        let entries: Vec<Value> = results
            .iter()
            .map(|r| {
                let mut obj = json!({
                    "path": r.path.display().to_string(),
                    "comment_count": r.comment_count,
                    "pending_count": r.pending_count,
                });
                let map = obj.as_object_mut().unwrap();
                if !r.pending_for.is_empty() {
                    map.insert("pending_for".into(), json!(r.pending_for));
                }
                if let Some(ts) = &r.last_activity {
                    map.insert("last_activity".into(), json!(ts.to_rfc3339()));
                }
                obj
            })
            .collect();
        print_output(true, &json!({ "results": entries }))
    } else {
        for r in &results {
            out(&format!(
                "{} ({} comments, {} pending)",
                r.path.display(),
                r.comment_count,
                r.pending_count,
            ))?;
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
                    .map(|(name, participant)| {
                        let status = match participant.status {
                            config::registry::RegistryParticipantStatus::Active => "active",
                            config::registry::RegistryParticipantStatus::Revoked => "revoked",
                            _ => "unknown",
                        };
                        json!({
                            "name": name,
                            "type": participant.author_type,
                            "status": status,
                            "pubkeys": participant.pubkeys.len(),
                        })
                    })
                    .collect();
                print_output(true, &json!({ "participants": participants }))
            } else {
                for (name, participant) in &registry.participants {
                    let status = match participant.status {
                        config::registry::RegistryParticipantStatus::Active => "active",
                        config::registry::RegistryParticipantStatus::Revoked => "revoked",
                        _ => "unknown",
                    };
                    out(&format!(
                        "{name} ({}) [{status}] {} key(s)",
                        participant.author_type,
                        participant.pubkeys.len(),
                    ))?;
                }
                Ok(())
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

fn cmd_verify(
    system: &dyn System,
    cwd: &Path,
    file: &str,
    public_key: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let doc = parser::parse_file(system, &path)?;
    let comments = doc.comments();

    let mut all_ok = true;
    let mut results: Vec<Value> = Vec::new();

    for cm in &comments {
        let checksum_ok = crypto::verify_checksum(cm);
        if !checksum_ok {
            all_ok = false;
        }

        let signature_status = public_key.map_or("not_checked", |pubkey| {
            if cm.signature.is_some() {
                match crypto::verify_signature(cm, pubkey) {
                    Ok(true) => "valid",
                    Ok(false) => {
                        all_ok = false;
                        "invalid"
                    }
                    Err(_) => {
                        all_ok = false;
                        "error"
                    }
                }
            } else {
                "missing"
            }
        });

        results.push(json!({
            "id": cm.id,
            "checksum_ok": checksum_ok,
            "signature": signature_status
        }));
    }

    if json_mode {
        print_output(true, &json!({ "results": results, "ok": all_ok }))?;
    } else {
        for r in &results {
            let id = r["id"].as_str().unwrap_or("?");
            let chk = if r["checksum_ok"].as_bool().unwrap_or(false) {
                "ok"
            } else {
                "FAIL"
            };
            let sig = r["signature"].as_str().unwrap_or("?");
            out(&format!("{id}: checksum={chk} signature={sig}"))?;
        }
    }

    if all_ok {
        Ok(())
    } else {
        anyhow::bail!("integrity check failed");
    }
}

fn cmd_write(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    path_str: &str,
    content: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let target = Path::new(path_str);

    let body = match content {
        Some(s) => String::from(s),
        None => read_stdin()?,
    };

    document::write(system, cwd, target, &body, config)?;
    print_output(json_mode, &json!({ "written": path_str }))
}
