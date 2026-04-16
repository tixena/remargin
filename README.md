# Remargin

Enhanced inline review protocol and document access layer for markdown.

Remargin turns any markdown document into a multi-player collaboration surface with threaded comments, integrity verification, and a complete document access layer that guarantees comments are never lost or corrupted.

## Why Remargin?

Collaborative document review in markdown is fragile. AI agents delete comments, break formatting, skip acknowledgments, or reply only in chat instead of inline. Manual comment conventions (`<!-- TODO -->`, custom fenced blocks) lack threading, identity, checksums, or any enforcement.

Remargin fixes this with:

- **A protocol** that defines a structured comment format inside standard markdown fenced code blocks
- **A CLI tool** that enforces the protocol, manages comments, and serves as the exclusive document access layer
- **An MCP server** that exposes the same capabilities to AI agents via [Model Context Protocol](https://modelcontextprotocol.io/)

Documents remain valid markdown. Comments are human-readable. No proprietary formats, no databases, no lock-in.

## Features

- **Threaded comments** with reply chains, acknowledgments, and emoji reactions
- **Multi-player identity** with three enforcement modes (open, registered, strict)
- **Cryptographic integrity** via SHA-256 checksums and optional Ed25519 signatures
- **Comment preservation guarantee** -- writes never destroy or corrupt existing comments
- **Batch operations** for atomic multi-comment updates in a single write
- **Cross-document queries** to find pending comments, filter by author/recipient/date
- **Full-text search** across documents with regex support
- **Document access layer** with allowlisted file types, dotfile hiding, and path sandboxing
- **Automatic frontmatter** tracking pending comment counts and last activity
- **Structural linting** that validates markdown and comment block integrity
- **Migration** from older inline comment formats
- **Dual interface** -- works as a standalone CLI or as an MCP server for Claude Code

## Installation

### From Source

Requires [Rust](https://rustup.rs/) (1.85+):

```bash
git clone https://github.com/tixena/remargin.git
cd remargin
cargo build --release
```

The binary will be at `target/release/remargin`. Add it to your `PATH`:

```bash
# Copy to a directory in your PATH
cp target/release/remargin ~/.local/bin/

# Or install directly with cargo
cargo install --path .
```

Verify the installation:

```bash
remargin version
```

## Quick Start

### Initialize a Project

Create a `.remargin.yaml` config file in your project root:

```yaml
identity: your-name
type: human
mode: open
```

### Add a Comment to a Document

```bash
remargin comment docs/design.md "This section needs more detail."
```

### Reply to a Comment

```bash
remargin comment docs/design.md "Good point, I'll expand this." --reply-to abc
```

### Reply and Acknowledge in One Step

```bash
remargin comment docs/design.md "Addressed, see updated section." --reply-to abc --auto-ack
```

### Read Comment Body from a File

```bash
remargin comment docs/design.md -F review-notes.md
echo "Quick note" | remargin comment docs/design.md -F -
```

### List Comments

```bash
remargin comments docs/design.md
remargin comments docs/design.md --pretty
```

### Acknowledge a Comment

```bash
# Ack in a specific file
remargin ack --file docs/design.md abc def

# Folder-wide ack (finds the comment by ID across the directory tree)
remargin ack abc
remargin ack abc --path docs/
```

### React to a Comment

```bash
remargin react docs/design.md abc "👍"
```

### Find Pending Comments Across Documents

```bash
remargin query docs/ --pending
remargin query . --pending-for your-name
remargin query docs/ --pending-for your-name --expanded
remargin query . --comment-id abc
```

### Search for Text

```bash
remargin search "TODO" --path docs/
remargin search "error|warning" --regex --ignore-case
```

### Read and Write Documents

```bash
# Read a file
remargin get docs/design.md

# Read with line numbers
remargin get docs/design.md -n

# Read a specific line range
remargin get docs/design.md --start-line 10 --end-line 50

# Write (preserves all existing comments)
remargin write docs/design.md "Updated content..."

# Create a new file
remargin write docs/new-doc.md "# New Doc" --create
```

## Comment Format

Comments live inside standard markdown fenced code blocks with the `remargin` language tag and a YAML header:

````markdown
```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:a1b2c3d4...
---
This is the comment content.

It can span multiple lines with **markdown formatting**.
```
````

With threading and acknowledgment:

````markdown
```remargin
---
id: xyz
author: claude
type: agent
ts: 2026-04-06T14:33:00-04:00
reply-to: abc
thread: abc
checksum: sha256:e5f6g7h8...
ack:
  - eduardo@2026-04-06T15:00:00-04:00
---
Replying to the comment above.
```
````

You don't write this format by hand -- the CLI and MCP tools produce it.

### Header Fields

| Field | Required | Description |
|-------|----------|-------------|
| `id` | Yes | Unique identifier (per-document scope, alphanumeric) |
| `author` | Yes | Author name or identifier |
| `type` | Yes | `human` or `agent` |
| `ts` | Yes | ISO 8601 timestamp with timezone |
| `checksum` | Yes | SHA-256 hash of comment content |
| `to` | No | List of recipients whose attention is requested |
| `reply-to` | No | ID of direct parent comment |
| `thread` | No | ID of thread root (oldest ancestor) |
| `attachments` | No | List of file paths relative to document directory |
| `reactions` | No | Map of emoji to list of authors |
| `ack` | No | List of `author@timestamp` acknowledgment entries |
| `signature` | No | Ed25519 signature (required in strict mode) |

## Configuration

Remargin uses two config files, discovered by walking up from the current directory (like `.git`):

### `.remargin.yaml` -- Project Settings

```yaml
# Default author identity
identity: eduardo

# Author type: human or agent
type: human

# Enforcement mode: open, registered, or strict
mode: open

# Path to Ed25519 private key (for signing)
# Short name resolves to ~/.ssh/<name>, full path used as-is
key: id_ed25519

# Directory for comment attachments (relative to document)
assets_dir: assets

# Glob patterns for files to ignore
ignore:
  - "drafts/**"
  - "*.tmp"
```

### `.remargin-registry.yaml` -- Participant Registry

Required for `registered` and `strict` modes. Maps participant IDs to their public keys and status:

```yaml
participants:
  eduardo:
    type: human
    public_key: ssh-ed25519 AAAAC3Nza...
    status: active
  claude:
    type: agent
    public_key: ssh-ed25519 AAAAC3Nzb...
    status: active
```

### Enforcement Modes

| Mode | Registry Required | Signatures Required | Description |
|------|-------------------|---------------------|-------------|
| `open` | No | No | Anyone can post. Default mode. |
| `registered` | Yes | No | Only participants in the registry can post. |
| `strict` | Yes | Yes | Registered + all comments must be Ed25519 signed. |

### CLI Overrides

All config values can be overridden per-invocation:

```bash
remargin --identity alice --type human --mode strict comment ...
```

## CLI Reference

```
remargin [OPTIONS] <COMMAND>
```

### Comment Management

| Command | Description |
|---------|-------------|
| `comment` | Create a comment (supports `--reply-to`, `--after-line`, `--after-comment`, `--to`, `--attach`, `--auto-ack`, `--comment-file`/`-F`) |
| `comments` | List all comments in a document (supports `--pretty` for threaded tree display) |
| `batch` | Create multiple comments atomically via `--ops` JSON (per-operation `auto_ack` support) |
| `edit` | Edit an existing comment (cascading ack clear on children) |
| `delete` | Delete one or more comments |
| `ack` | Acknowledge one or more comments (supports folder-wide resolution by ID when `--file` is omitted) |
| `react` | Add or remove an emoji reaction |

### Document Access

| Command | Description |
|---------|-------------|
| `get` | Read a file's contents (with optional line range and `--line-numbers`/`-n`) |
| `ls` | List files and directories |
| `write` | Write document contents (comment-preserving, `--create` for new files) |
| `metadata` | Get document metadata (frontmatter, comment counts, pending status) |

### Search and Quality

| Command | Description |
|---------|-------------|
| `query` | Search across documents for comments (filter by `--pending`, `--pending-for`, `--author`, `--since`, `--comment-id`; `--expanded` for inline comment details) |
| `search` | Full-text search across documents (supports `--regex`, `--scope`, `--context`, `--ignore-case`) |
| `lint` | Run structural lint checks on a document |
| `verify` | Verify comment integrity (checksums and signatures) |

### Maintenance

| Command | Description |
|---------|-------------|
| `migrate` | Convert old-format inline comments to remargin format |
| `purge` | Strip all comments from a document |
| `keygen` | Generate a new Ed25519 signing key pair |
| `version` | Print version information |

### Integration

| Command | Description |
|---------|-------------|
| `mcp install [--user]` | Register as an MCP server in Claude Code |
| `mcp uninstall` | Remove MCP server registration |
| `mcp test` | Check MCP registration status |
| `mcp run` | Start the MCP server (stdio transport) |
| `skill install [--agent <agent>] [--global]` | Install the skill (default agent: claude) |
| `skill uninstall [--agent <agent>]` | Remove the skill |
| `skill test [--agent <agent>]` | Check skill installation status |
| `registry show` | Display the participant registry |

### Global Options

| Flag | Description |
|------|-------------|
| `--config <PATH>` | Path to config file |
| `--identity <NAME>` | Author name for this operation |
| `--type <human\|agent>` | Author type |
| `--mode <open\|registered\|strict>` | Enforcement mode |
| `--key <PATH>` | Path to Ed25519 signing key |
| `--assets-dir <PATH>` | Assets directory path |
| `--dry-run` | Preview changes without writing |
| `--json` | Output as JSON |
| `--verbose` | Enable tracing output |

## Claude Code Integration

Remargin integrates with [Claude Code](https://docs.anthropic.com/en/docs/claude-code) in two ways:

### MCP Server

The MCP server exposes all remargin operations as tools that Claude can call directly. This is the primary integration -- it makes remargin the document access layer for AI agents.

```bash
# Install at project scope (recommended)
remargin mcp install

# Or install at user scope (available in all projects)
remargin mcp install --user

# Verify installation
remargin mcp test
```

Once installed, Claude Code will have access to these tools: `ls`, `get`, `write`, `metadata`, `comment`, `comments`, `batch`, `edit`, `delete`, `ack`, `react`, `query`, `search`, `lint`, `verify`, `migrate`, `purge`.

### Skill

The skill teaches agents *when* and *how* to use the MCP tools -- trigger phrases, display format for comments, critical rules (like never using `Read`/`Edit`/`Write` for remargin-managed documents), and common workflows.

```bash
# Install at project scope (defaults to --agent claude)
remargin skill install

# Install for a specific agent
remargin skill install --agent claude
remargin skill install --agent gemini

# Or install globally
remargin skill install --global

# Verify installation
remargin skill test
remargin skill test --agent gemini
```

### Permissions

To avoid per-tool confirmation prompts, add this to your Claude Code `settings.local.json`:

```json
{
  "permissions": {
    "allow": [
      "mcp__remargin__*"
    ]
  }
}
```

### Recommended Setup

For a project using remargin with Claude Code:

```bash
# Install both MCP server and skill
remargin mcp install
remargin skill install  # installs for Claude Code by default

# Add permissions (optional, avoids confirmation prompts)
# Edit .claude/settings.local.json and add mcp__remargin__* to allow list
```

## Integrity and Security

### Checksums

Every comment gets a SHA-256 checksum of its content (normalized whitespace). This detects any post-creation modification of comment text.

```bash
# Verify all checksums in a document
remargin verify docs/design.md
```

### Signatures

In `strict` mode, comments must be signed with Ed25519 keys. Generate a key pair:

```bash
remargin keygen ~/.remargin/keys/mykey
```

This produces `mykey` (private) and `mykey.pub` (public). Add the public key to the registry and configure the private key in `.remargin.yaml`:

```yaml
key: ~/.remargin/keys/mykey
```

Signatures cover the comment content plus metadata (id, author, type, timestamp, recipients, threading, attachments), ensuring authenticity and tamper detection.

### Comment Preservation

Every write operation enforces a strict invariant: the set of comment IDs before and after the write must match exactly, with only the expected delta (new comments added, or specific comments deleted). Any unexpected change aborts the operation. This guarantees that document edits -- whether by humans or agents -- never accidentally destroy comments.

## Typical Workflows

### Document Review

```bash
# Find documents with pending comments for you
remargin query . --pending-for your-name

# See expanded details (matching comments grouped by file)
remargin query . --pending-for your-name --expanded

# Read the document
remargin get docs/proposal.md

# See the discussion
remargin comments docs/proposal.md --pretty

# Add your review comments
remargin comment docs/proposal.md "Needs error handling." --after-line 42

# Acknowledge comments addressed to you
remargin ack --file docs/proposal.md abc def

# Or ack by ID without specifying the file
remargin ack abc def

# When review is complete, produce a clean version
remargin purge docs/proposal.md
```

### Batch Review

```bash
# Add multiple comments in one atomic operation
remargin batch docs/design.md --ops '[
  {"content": "Good approach here.", "after_line": 10},
  {"content": "Edge case: what if input is empty?", "after_line": 35},
  {"content": "This contradicts section 2.", "after_line": 78}
]'
```

### Migration from Old Format

If you have documents using the older `user comments` / `agent comments` fenced block format:

```bash
# Preview what would change
remargin migrate docs/old-doc.md --dry-run

# Convert
remargin migrate docs/old-doc.md
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Lint failure |
| 3 | Integrity failure |
| 4 | Missing attachment |
| 5 | Comment preservation violation |
| 6 | Skill error |
| 7 | Comment not found (folder-wide ack) |
| 8 | Ambiguous comment ID (found in multiple files) |

## Building

```bash
# Debug build
cargo build

# Release build (with LTO)
cargo build --release

# Run tests
cargo test

# Run clippy (strict -- the project enforces deny-all clippy lints)
cargo clippy
```

## Contributing

Contributions are welcome. Here are some guidelines:

1. **Fork and branch** -- create a feature branch from `master`
2. **Keep changes focused** -- one feature or fix per PR
3. **Follow existing patterns** -- the codebase uses strict clippy lints (all, pedantic, restriction, nursery levels). Run `cargo clippy` before submitting
4. **Write tests** -- the project uses `assert_cmd` and `tempfile` for integration tests
5. **Update the skill** -- if you add or change MCP tools, update `crates/remargin-core/skill/SKILL.md` accordingly
6. **Commit messages** -- use conventional commits (`feat:`, `fix:`, `chore:`, etc.)

### Development Setup

```bash
git clone https://github.com/tixena/remargin.git
cd remargin
cargo build
cargo test
```

The project uses Rust 2024 edition with strict clippy enforcement. If clippy complains, fix the lint -- don't suppress it unless there's a documented reason in `Cargo.toml`.

## License

MIT
