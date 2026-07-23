# Remargin

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.1.15-7F6DF2.svg)](Cargo.toml)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://rustup.rs)
[![Made by Tixena Labs](https://img.shields.io/badge/made_by-Tixena_Labs-7F6DF2.svg)](https://tixenalabs.com/)

> A communication protocol for humans and AI agents — and between agents themselves.

Remargin turns any markdown file into a multi-player surface where humans and AI agents leave threaded, addressable, signed, integrity-checked comments. Mutating operations are gated to preserve the conversation. That makes it a credible substrate for multi-agent orchestration — agents coordinate by leaving comments on shared documents, not by sharing memory or a bespoke message bus.

The protocol is a tiny structured comment format inside standard markdown fenced code blocks. The CLI enforces it. The MCP server exposes the same surface to any MCP client. No proprietary format, no database, no SaaS. Just markdown files in your repo.

## Table of contents

- [At a glance](#at-a-glance)
- [Why Remargin?](#why-remargin)
- [Multi-agent example](#multi-agent-example)
- [Features](#features)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Scope: what remargin manages](#scope-what-remargin-manages)
- [Claude Code integration](#claude-code-integration)
- [Session launch (multi-agent orchestration)](#session-launch-multi-agent-orchestration)
- [Comment format](#comment-format)
- [Configuration](#configuration)
- [Permissions and access control](#permissions-and-access-control)
- [Integrity and security](#integrity-and-security)
- [CLI reference](#cli-reference)
- [Tracking change](#tracking-change)
- [Typical workflows](#typical-workflows)
- [Exit codes](#exit-codes)
- [Building](#building)
- [Contributing](#contributing)
- [License](#license)

## At a glance

This is what a remargin comment looks like inside a markdown file:

````markdown
```remargin
---
id: pl1
author: planner
type: agent
ts: 2026-05-19T14:30:00-04:00
to: [engineer]
remargin_kind: [design-question]
checksum: sha256:a1b2c3d4e5f6...
signature: ed25519:LS0tLS1CRUdJTi...
---
Two open choices for the CSV sort CLI:
1. column addressed by **name** vs **index**
2. **in-place** rewrite vs **streaming** output

Vote on each — I'll commit the spec to whatever you pick.
```
````

Standard markdown renderers ignore the `remargin` fenced block. The CLI and MCP server parse and verify it: identity, timestamp, threading (`reply-to`, `thread`), addressing (`to`), free-form labels (`remargin_kind`), SHA-256 content checksum, and optional Ed25519 signature. The full [header field reference](#comment-format) is below.

## Why Remargin?

Multi-agent systems coordinate today through ad-hoc message buses, shared memory, or scrollback chat logs that no one can audit later. Every team rebuilds the same plumbing, badly. Documents and code — the actual artifacts under negotiation — sit on the side, disconnected from the trail of decisions that produced them.

Remargin proposes a simpler substrate:

- **Agents communicate by leaving comments on shared markdown documents.** Each comment carries identity, time, threading, addressing, and content integrity.
- **The protocol guarantees no comment is ever silently dropped.** Every mutating op runs a comment-preservation check — if the post-write set of comment IDs would lose anything from the pre-write set, the write is rejected (exit code 5).
- **Humans participate in the same conversation through the same protocol.** No special role, no separate UI required.
- **The decision trail lives in plain markdown.** Open any file in any editor and read the entire conversation. Replayable, auditable, queryable. No proprietary database.

It also solves the older problem the project started with — humans and a single agent reviewing a document together, with the agent reliably preserving comments and threading. That problem hasn't gone away; the multi-agent framing is a superset.

## Multi-agent example

Three agents — `planner`, `engineer`, `qa` — collaborating on a single `spec.md`. The full conversation lives as threaded comments in the file itself.

`spec.md`:

````markdown
# CSV sort CLI

Sort a CSV file by a column.

```remargin
---
id: pl1
author: planner
type: agent
ts: 2026-05-19T14:30:00-04:00
to: [engineer]
remargin_kind: [design-question]
checksum: sha256:...
signature: ed25519:...
---
Two open choices:
1. column addressed by **name** vs **index**
2. **in-place** rewrite vs **streaming** output

Vote on each.
```

```remargin
---
id: en1
author: engineer
type: agent
ts: 2026-05-19T14:33:00-04:00
to: [planner]
reply-to: pl1
thread: pl1
ack:
  - planner@2026-05-19T14:34:00-04:00
checksum: sha256:...
signature: ed25519:...
---
1. **name** — friendlier; error out if the row has no header.
2. **streaming** — predictable memory for large files.
```

```remargin
---
id: pl2
author: planner
type: agent
ts: 2026-05-19T14:36:00-04:00
to: [qa]
reply-to: en1
thread: pl1
ack:
  - engineer@2026-05-19T14:40:00-04:00
checksum: sha256:...
signature: ed25519:...
---
Spec updated. QA, draft acceptance tests covering name-based selection,
streaming on a 10M-row file, and a clear error on a headerless CSV.
```
````

Each comment is Ed25519-signed by its author's registered key (in `strict` mode). Threading (`reply-to`, `thread`) preserves the tree. `ack:` records consent. To see this conversation rendered in the terminal:

```bash
remargin comments spec.md --pretty
```

To see every comment addressed to you across an entire project (your "inbox"):

```bash
remargin query . --pending-for-me --pretty
```

## Features

- **Threaded comments** with reply chains (`reply-to`, `thread`), acknowledgments (`ack`), and emoji reactions
- **Multi-player identity** with three enforcement modes (open, registered, strict)
- **Cryptographic integrity** via SHA-256 checksums and optional Ed25519 signatures
- **Comment preservation guarantee** — writes never destroy or corrupt existing comments
- **Free-form `remargin_kind` labels** for triage filters (`urgent`, `to-read`, `design-question`, anything)
- **Batch operations** for atomic multi-comment updates in a single write
- **Cross-document queries** to find pending comments, filter by author, recipient, date, or kind
- **Full-text search** across documents with regex support
- **Document access layer** with allowlisted file types, dotfile hiding, and path sandboxing
- **Per-realm permissions** with two-layer enforcement (CLI/MCP + Claude Code native tools)
- **Activity feed** — what's new since you last acted, per-file, across the realm
- **Sandbox staging** — per-identity soft claims on files before a structured processing pass
- **Plan-based previews** — dry-run any mutating op before it touches disk
- **Structural linting** that validates markdown and comment block integrity
- **Migration** from older inline comment formats
- **Dual interface** — works as a standalone CLI or as an MCP server for any MCP client
- **Session launch (multi-agent orchestration)** — `remargin session launch` discovers every identity down the tree and starts one Claude `/loop` session per identity, each in its own terminal-multiplexer tab (herdr or tmux); gated behind the `session` build feature

## Installation

### Install from the GitHub repo (recommended)

Requires [Rust](https://rustup.rs/) 1.85+.

```bash
cargo install --git https://github.com/tixena/remargin
remargin version
```

### Build from source

```bash
git clone https://github.com/tixena/remargin.git
cd remargin
cargo build --release

# Either copy the binary onto your PATH...
cp target/release/remargin ~/.local/bin/

# ...or install via cargo from the local checkout
cargo install --path crates/remargin
```

Verify:

```bash
remargin version
```

## Quick start

### Create your identity

Create a `.remargin.yaml` in your project root. The fastest path is `remargin identity create`, which prints a ready-to-use YAML block to stdout:

```bash
remargin identity create --identity your-name --type human > .remargin.yaml

# Or with a signing key for strict-mode realms:
remargin identity create --identity your-name --type human --key ~/.ssh/remargin_key > .remargin.yaml
```

The generated file looks like:

```yaml
identity: your-name
type: human
```

`mode:` is a tree-level property — set it in `.remargin.yaml` directly when you want to switch modes. Modes and the registry file are covered under [Configuration](#configuration).

### Comment on a document

```bash
# Add a top-level comment after line 42
remargin comment docs/design.md "This section needs more detail." --after-line 42

# Reply to an existing comment
remargin comment docs/design.md "Good point, I'll expand this." --reply-to abc

# Reply and acknowledge the parent in one step
remargin comment docs/design.md "Addressed, see updated section." --reply-to abc --auto-ack

# Read comment body from a file (or stdin)
remargin comment docs/design.md -F review-notes.md
echo "Quick note" | remargin comment docs/design.md -F -
```

### See what's pending

```bash
# Everything still open, directed or broadcast
remargin query docs/ --pending

# Only comments directed at the current identity
remargin query . --pending-for-me

# Only broadcast (empty-`to`) comments not yet acked
remargin query . --pending-broadcast

# Pretty-printed, with full comment bodies grouped by file
remargin query . --pending-for-me --expanded --pretty
```

### Read, write, search

```bash
# Read a file (with line numbers or a range)
remargin get docs/design.md -n
remargin get docs/design.md --start-line 10 --end-line 50

# Write a file (preserves all existing comments)
remargin write docs/design.md "Updated content..."
remargin write docs/new-doc.md "# New Doc" --create

# Full-text search
remargin search "TODO" --path docs/
remargin search "error|warning" --regex --ignore-case

# Find/replace across document BODY text only (never inside comments).
# Integrity-gated like write: a comment is never corrupted, and a pattern
# that occurs only inside a comment is a no-op. Works on a file or a folder.
remargin replace "old name" "new name" --path docs/
remargin replace "id=(\d+)" "id=[$1]" --regex --path docs/design.md
remargin replace "foo" "bar" --path docs/ --dry-run   # preview; writes nothing
```

### Acknowledge and react

```bash
# Ack one or more comments in a specific file
remargin ack --file docs/design.md abc def

# Or ack by ID without specifying the file (folder-wide resolution)
remargin ack abc

# Add an emoji reaction
remargin react docs/design.md abc "👍"
```

## Scope: what remargin manages

A **remargin realm** is any directory tree containing a `.remargin.yaml` (discovered by walking upward from the current directory, like `.git`). **Every `.md` file inside a realm is a remargin-managed document.** There is no per-file opt-in: once the config file is present, every markdown file under that tree is accessed through `remargin` (CLI or MCP), not through raw filesystem edits.

This covers notes, drafts, READMEs, scratch files, and anything else ending in `.md`. A file that has never had a comment is still managed — remargin's frontmatter tracking, comment-preservation invariants, and identity/mode enforcement apply the moment any tool touches it.

## Claude Code integration

Remargin integrates with [Claude Code](https://docs.anthropic.com/en/docs/claude-code) in two ways. For multi-agent setups, this is usually how you wire each agent into the protocol.

### MCP server

The MCP server exposes all remargin operations as tools Claude can call directly. It is the document access layer for the agent.

```bash
# Install at project scope (recommended)
remargin mcp install

# Or install at user scope (available in all projects)
remargin mcp install --user

# Verify installation
remargin mcp test
```

Once installed, Claude Code gets these tools: `ls`, `get`, `get_image`, `write`, `replace`, `metadata`, `comment`, `comments`, `batch`, `edit`, `delete`, `ack`, `react`, `query`, `search`, `report_spill`, `activity`, `lint`, `verify`, `migrate`, `purge`, `plan`, `sandbox_add`, `sandbox_list`, `sandbox_remove`, `prompt_resolve`, `prompt_list`, `permissions_show`, `permissions_check`, `identity_create`, `whoami`, `cp`, `mv`, `rm`. (`get_image` returns an image content block for a referenced image; `report_spill` ratchets the search page cap down after an over-limit result.)

### Plugin

The Claude Code plugin ships the remargin skill plus the `/remargin:process-file` and `/remargin:process-sandbox-group` slash commands. The skill teaches Claude *when* and *how* to use the MCP tools — trigger phrases, display format for comments, critical rules (like never using `Read`/`Edit`/`Write` for remargin-managed documents), and common workflows.

```bash
# Register the marketplace and install the plugin
remargin claude plugin install

# Verify installation
remargin claude plugin test
```

### Hooks: enforcing the boundary

Two Claude Code hooks keep agents on the sanctioned surface. Both are idempotent; `install` accepts `--local` to write project-scope settings instead of the default `~/.claude/settings.json`.

**PreToolUse enforcement** — inspects every gated tool call (`Read`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit`, `Grep`, `Glob`, `Bash`) and denies the ones that would touch a remargin-managed path, redirecting the agent to the matching `mcp__remargin__*` op. This hook is the single source of truth for enforcement.

```bash
remargin claude pretool install
remargin claude pretool test
```

**SessionStart guard** — the enforcement hook *fails open*: if `remargin` is not on `PATH` the `PreToolUse` command exits 127, which Claude Code treats as non-blocking, so the tool call proceeds unprotected with no signal. The guard runs once at session start, re-checks that `remargin` resolves on `PATH` and that the realm's `.remargin.yaml` parses, and — because a `SessionStart` hook cannot block a session — surfaces any failure as a loud diagnostic injected into the session context (and a warning to the user) telling the agent enforcement may be silently disabled.

```bash
remargin claude session-guard install
remargin claude session-guard test
```

Run `remargin doctor` at any time to confirm both hooks are wired; it reports a critical finding for each missing hook, naming the install command to run.

### Permissions (optional)

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

### Recommended setup

For a single agent in one project:

```bash
remargin mcp install
remargin claude plugin install
remargin claude pretool install       # enforce the boundary
remargin claude session-guard install # backstop the fail-open hook
```

For a multi-agent setup (multiple Claude Code instances sharing a realm):

1. Generate a signing key per agent: `remargin keygen ~/.remargin/keys/<agent-name>`.
2. Register each agent's public key in `.remargin-registry.yaml`.
3. Give each agent its own `.remargin.yaml` (or a `--config` override) declaring that agent's identity + key.
4. Set `mode: strict` at the realm level so every comment must be signed by a registered participant.

The agents share the realm, see each other's comments, ack each other, thread, react — all via the same MCP surface.

## Session launch (multi-agent orchestration)

`remargin session launch` turns a tree of identity-scoped realms into a running multi-agent workspace. It walks *down* from the current directory, finds every `.remargin.yaml` that declares its **own** `identity`, and starts **one Claude session per identity** — each in its own terminal-multiplexer tab, with:

- **cwd** set to that identity's folder,
- a **remargin MCP server** scoped to that folder + identity,
- the **composed system prompt** (the folder's resolved `system_prompt` plus remargin's operating rules),
- running under Claude Code's **`/loop`** (the interval) with a **`/goal`** stop condition.

The workflow is: a human stages work (drops a file into an identity's sandbox, leaves a comment) → each identity-bound agent processes its pending work through the document layer on its next `/loop` wake → the human reviews. remargin only *launches*. It writes no PID file, runs no supervisor, and performs no teardown; a session ends when its `/goal` is reached or you kill its tab. Which session handles which work is a matter of how you lay out realms and identities — concurrent writes can't corrupt a file (writes are atomic), so coordination is the workflow owner's design, not something the launcher enforces.

> The `session:` config block and the `session launch` command are gated behind a **`session` Cargo feature**, off by default so a shipped binary stays lean. The default binary has **no** `session` subcommand — build or install remargin with the feature to use it (see [The `session` feature](#the-session-feature) below).

### The `session:` block

Each agent's `.remargin.yaml` declares its launch parameters in an optional `session:` block. `goal` is **required to launch** (a session missing it fails to build); `loop` is optional and defaults to `5m`; the rest is optional.

```yaml
identity: finance_agent
key: ~/.remargin/keys/finance
mode: strict
system_prompt:
  name: Finance
  prompt: "You are the finance agent. Process your pending sandbox work."
session:
  loop: 30s                                                       # optional — /loop cadence (a duration: 30s, 5min, 1h); defaults to 5m
  goal: "process pending work; stop when the sandbox is empty"    # required — the /goal stop condition
  claude: { model: claude-opus-4-8, effort: high }                # optional — backend model + effort
  budget: { max_turns: 20 }                                       # optional — omit for no cap (e.g. local models)
```

### Commands and flags

```bash
remargin session launch                          # launch every discovered identity
remargin session launch --dry-run                # print the discovery table; spawn nothing
remargin session launch --print                  # emit the exact per-identity commands; start nothing
remargin session launch --multiplexer herdr      # force a specific multiplexer (herdr | tmux)
remargin session launch --identity finance,ops   # only these identities
```

- **`--dry-run`** walks the tree and prints one row per discovered identity — identity, folder, resolved prompt, `loop`, `goal`, scope — shows the defaulted `5m (default)` cadence for any identity that omits `loop`, flags any missing a required `goal`, and exits non-zero when any are unlaunchable. It spawns nothing.
- **`--print`** emits the exact per-identity launch commands (as runnable `cd <folder> && claude …` lines) and starts nothing — for wiring the sessions into your own setup.
- **`--multiplexer herdr|tmux`** picks the multiplexer. Unset means auto: herdr when its server is reachable, else tmux. An explicit value always wins.
- **`--identity a,b`** restricts discovery to the named identities (comma-separated).
- **`--backend`** selects the session backend (default `claude`).

A `--dry-run` over the six-identity `demo-remargin` tree (`ops` here is missing its `goal`):

```
$ remargin session launch --dry-run
IDENTITY             FOLDER                     PROMPT       LOOP  GOAL                         SCOPE
eburgos_notes_agent  demo-remargin              (default)    30s   process pending; stop empty  demo-remargin
audience             demo-remargin/audience     Audience     30s   process pending; stop empty  demo-remargin/audience
content              demo-remargin/content      Content      30s   process pending; stop empty  demo-remargin/content
coordinator          demo-remargin/coordinator  Coordinator  30s   process pending; stop empty  demo-remargin/coordinator
finance              demo-remargin/finance      Finance      30s   process pending; stop empty  demo-remargin/finance
ops                  demo-remargin/ops          Ops          30s   MISSING goal                 demo-remargin/ops
6 identities; 1 not launchable (missing loop/goal).
```

A bare launch prints the session name and how to attach:

```
$ remargin session launch
Launched 6 session(s) in herdr session: eburgos_notes-demo-4f9c
Attach with:  herdr session attach eburgos_notes-demo-4f9c
```

### herdr (flagship) vs tmux (fallback)

[herdr](https://herdr.dev) is an agent-aware terminal workspace manager: it addresses tabs and agents **by name**, exposes blocking `wait` primitives, and natively detects Claude's session state. That makes it the default whenever it is installed and its server is running. tmux is the zero-extra-dependency fallback.

Prerequisites for the herdr path:

- herdr installed and its server running (`herdr status` must succeed).
- Recommended: `herdr integration install claude`, which sharpens Claude-state detection.

Selection rules:

- **`--multiplexer` unset:** herdr when its server is reachable, else tmux — silently, no error.
- **`--multiplexer herdr` (explicit) but herdr unavailable:** the launch errors *before* creating anything, naming the fix (start/install herdr, or run with `--multiplexer tmux`).
- **`--multiplexer tmux`:** always uses tmux.

### Attach and watch

Reattach any time — one tab per identity:

```bash
herdr session attach <name>     # herdr
tmux attach -t <name>           # tmux
```

Watching, focusing, and stopping a session are your multiplexer's job. To stop one, kill its tab (or the whole session); it also stops itself when its `/goal` is reached.

### Launching on a remote host

To launch on a remote machine, **run the launcher on that host** (over SSH) so its multiplexer uses the local socket — machines that share paths and tooling make this clean. `herdr --remote <host>` is **attach-only**: it proxies the TUI so a human can watch from a laptop, and cannot drive a launch.

### The `session` feature

The `session:` config block and the `session launch` command are compiled only when the **`session` Cargo feature** is enabled — off by default so a shipped/installed binary stays lean. The default binary has no `session` subcommand. To use the feature, build or install with it enabled:

```bash
cargo build -p remargin --features session
# or, from the workspace root:
cargo build --features remargin/session
```

### Permissions

Launched agents run under Claude Code's `--permission-mode auto` so an unattended `/loop` agent can call its remargin MCP tools without stalling on a per-call permission prompt (`acceptEdits` auto-approves only file edits, not MCP tool calls).

## Comment format

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

With threading, addressing, and acknowledgment:

````markdown
```remargin
---
id: xyz
author: claude
type: agent
ts: 2026-04-06T14:33:00-04:00
to: [eduardo]
reply-to: abc
thread: abc
checksum: sha256:e5f6g7h8...
ack:
  - eduardo@2026-04-06T15:00:00-04:00
---
Replying to the comment above.
```
````

You don't write this format by hand — the CLI and MCP tools produce it.

### Header fields

| Field | Required | Description |
|-------|----------|-------------|
| `id` | Yes | Unique identifier (per-document scope, alphanumeric). |
| `author` | Yes | Author name or identifier. |
| `type` | Yes | `human` or `agent`. |
| `ts` | Yes | ISO 8601 timestamp with timezone. |
| `checksum` | Yes | SHA-256 hash of normalized comment content. |
| `to` | No | List of recipients whose attention is requested. Omit for broadcast. |
| `reply-to` | No | ID of the direct parent comment. |
| `thread` | No | ID of the thread root (oldest ancestor). |
| `remargin_kind` | No | Free-form labels (max 15 chars each, `[a-zA-Z0-9_- ]`, no leading/trailing space, distinct within a comment). |
| `attachments` | No | List of file paths relative to document directory. |
| `reactions` | No | Map of emoji to list of authors. |
| `ack` | No | List of `author@timestamp` acknowledgment entries. |
| `signature` | No | Ed25519 signature (required in `strict` mode). |

## Configuration

Remargin uses two config files, discovered by walking up from the current directory (like `.git`). The presence of `.remargin.yaml` defines the realm — every markdown file under that tree is managed from that point on.

### `.remargin.yaml` — project settings

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

### `session:` block — per-agent launch parameters

Consumed by [`remargin session launch`](#session-launch-multi-agent-orchestration). Declares how this folder's identity is launched as a looping Claude session. `goal` is required to launch; `loop` is optional and defaults to `5m`; `claude` and `budget` are optional. The block only parses (and the `session launch` command only exists) when remargin is built with the `session` feature.

```yaml
session:
  loop: 30s                                                     # optional — /loop cadence (30s, 5min, 1h, …); defaults to 5m
  goal: "process pending work; stop when the sandbox is empty"  # required — the /goal stop condition
  claude: { model: claude-opus-4-8, effort: high }              # optional — backend model + effort
  budget: { max_turns: 20 }                                     # optional — omit for no cap
```

### `.remargin-registry.yaml` — participant registry

Required for `registered` and `strict` modes. Maps participant IDs to their public keys and status:

```yaml
participants:
  eduardo:
    type: human
    public_key: ssh-ed25519 AAAAC3Nza...
    status: active
  planner:
    type: agent
    public_key: ssh-ed25519 AAAAC3Nzb...
    status: active
  engineer:
    type: agent
    public_key: ssh-ed25519 AAAAC3Nzc...
    status: active
```

### Enforcement modes

| Mode | Registry Required | Signatures Required | Description |
|------|-------------------|---------------------|-------------|
| `open` | No | No | Anyone can post. Default. |
| `registered` | Yes | No | Only participants in the registry can post. |
| `strict` | Yes | Yes | Registered + every comment Ed25519-signed. |

### CLI overrides

All config values can be overridden per invocation:

```bash
remargin --identity alice --type human --mode strict comment ...
```

## Permissions and access control

Remargin supports a `permissions:` block in `.remargin.yaml` that restricts which paths agents can mutate and which Bash commands can run on those paths. Two enforcement layers consume the block:

```yaml
permissions:
  trusted_roots:
    - ~/src/tixena/eburgos_notes
    - ~/src/tixena/remargin
    - path: src/01_personal/secure
      also_deny_bash: [curl, wget]
    - path: '*'
  deny_ops:
    - path: src/01_personal/signed_archive
      ops: [purge, delete]
  allow_dot_folders: ['.github']
```

- **Layer 1 (remargin-core, CLI + MCP, per-op).** Every mutating op parent-walks `.remargin.yaml` and refuses ops outside the `trusted_roots` allow-list or matching `deny_ops`. The walk runs fresh on every call — no cache, no reload command, no mtime watcher. Editing `.remargin.yaml` between two ops takes effect on the second op without a restart.
- **Layer 2 (Claude Code `PreToolUse` hook, native tools).** The `remargin claude pretool` hook inspects every gated tool call (`Read`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit`, `Grep`, `Glob`, `Bash`) and denies the ones that would touch a managed path, redirecting the agent to the matching `mcp__remargin__*` op. It resolves the boundary from `.remargin.yaml` on every call and is **the single source of truth** for native-tool enforcement — `remargin claude restrict` no longer projects `permissions.deny` rules into the settings files. Run `remargin doctor` to confirm the hook is wired and to find (and clear) any leftover projected rules an older restrict left behind.

The single exception to per-op evaluation is `trusted_roots`, which defines the MCP server's filesystem sandbox at boot time — the sandbox cannot be expanded mid-session.

### Op classification: read vs write

`trusted_roots` and the dot-folder default-deny only gate **write-side** ops. Read-side ops bypass `trusted_roots` entirely so a restricted path can still be inspected without unrestrict/restrict ceremony. To block reads on a path, declare an explicit `deny_ops` entry naming the read op (`deny_ops` is evaluated for both kinds).

| Kind | Ops |
|------|-----|
| Read (bypass `trusted_roots`) | `comments`, `get`, `lint`, `ls`, `metadata`, `query`, `search`, `verify` |
| Write (gated by `trusted_roots`) | `ack`, `batch`, `comment`, `cp`, `delete`, `edit`, `migrate`, `purge`, `react`, `replace`, `sandbox-add`, `sandbox-remove`, `sign`, `write` |

The lists are pinned by `READ_OPS` and `MUTATING_OPS` in `remargin_core::permissions::op_guard`. Adding a new op MUST classify it at PR time by adding the canonical name to one of those constants; unknown ops fail closed (treated as write-side under `trusted_roots`).

The user-visible denial messages are pinned by `denial_error_wording_matches_canonical_template` in `crates/remargin-core/src/permissions/op_guard/tests.rs`. The canonical templates are:

- `op '<op>' on '<target>' is denied: outside the allow-list declared by 'trusted_roots' in <yaml>`
- `op '<op>' on '<target>' is denied by 'deny_ops' rule in <yaml>`

### Commands

```
# Add / remove restrictions
remargin claude restrict <PATH | *> [--also-deny-bash CMD,CMD] [--cli-allowed]
remargin claude unrestrict <PATH | *>

# Inspect
remargin permissions show [--json]
remargin permissions check <PATH> [--why]
```

Because the hook is the single source of truth, a fresh `claude restrict` writes **only** the `.remargin.yaml` entry — no `permissions.deny` rules, and therefore no sidecar. The sidecar at `<.claude-anchor>/.claude/.remargin-restrictions.json` survives only for realms an older remargin projected rules into: `claude unrestrict` reads it to scrub those legacy rules cleanly without ever touching user-added rules, and `remargin doctor` flags any that were never reversed. When present, it is `.gitignore`d automatically (its absolute paths and per-machine timestamps don't belong in version control).

`permissions check <path>` exits gitignore-style: 0 when the path is restricted, 1 when not. Pair with `--why` for the matching rule's kind, source file, and rule text.

The canonical `permissions show --json` schema (every field, per entry kind) is documented as the module-level rustdoc on `remargin_core::permissions::inspect` and pinned by a `#[serde(deny_unknown_fields)]` schema test in `crates/remargin/tests/cli_permissions.rs` — adding a field on the Rust types without updating the doc fails the build.

The read-only inspection surfaces are exposed via MCP as `mcp__remargin__permissions_show` and `mcp__remargin__permissions_check`. `claude restrict` and `claude unrestrict` are intentionally CLI-only: they mutate permission policy and that decision belongs to the human, not to the agent. `mcp__remargin__plan` likewise rejects `op="claude_restrict"` and `op="claude_unrestrict"`.

## Integrity and security

### Checksums

Every comment gets a SHA-256 checksum of its content (normalized whitespace). This detects any post-creation modification of comment text.

```bash
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

Signatures cover the comment content plus metadata (id, author, type, timestamp, recipients, threading, attachments, `remargin_kind`), ensuring authenticity and tamper detection.

### Comment preservation

Every write operation enforces a strict invariant: the set of comment IDs before and after the write must match exactly, with only the expected delta (new comments added, or specific comments deleted). Any unexpected change aborts the operation with exit code 5. This guarantees that document edits — whether by humans or agents — never accidentally destroy comments left by other participants.

```bash
$ remargin write spec.md "$(cat broken-spec.md)"
error: comment preservation violation
       comments dropped: [pl1, en1]
       canonical pre-write set: 3 comments
       post-write set: 1 comment
exit code: 5
```

### Authenticated author frontmatter

Document-level `author` frontmatter is authenticated on every write, so a caller cannot spoof authorship. On **create**, remargin stamps the authenticated caller's identity, dropping any `author` supplied in the payload. On **edit**, an unchanged or omitted author is preserved; changing it is gated by the realm mode — `open` allows any value, `registered` requires an active participant, and `strict` rejects changing an existing author (an authorless document may only gain the caller's own identity).

## CLI reference

```
remargin [OPTIONS] <COMMAND>
```

### Comment management

| Command | Description |
|---------|-------------|
| `comment` | Create a comment (supports `--reply-to`, `--after-line`, `--after-comment`, `--to`, `--attach`, `--auto-ack`, `--comment-file`/`-F`, `--kind`) |
| `comments` | List all comments in a document (supports `--pretty` for threaded tree display, `--kind` filter) |
| `batch` | Create multiple comments atomically via `--ops` JSON (per-operation `auto_ack` support) |
| `edit` | Edit an existing comment (cascading ack clear on children) |
| `delete` | Delete one or more comments |
| `ack` | Acknowledge one or more comments (supports folder-wide resolution by ID when `--file` is omitted) |
| `react` | Add or remove an emoji reaction |
| `sign` | Add an Ed25519 signature to one or more existing comments |

### Document access

| Command | Description |
|---------|-------------|
| `get` | Read a file's contents (with optional line range and `--line-numbers`/`-n`; `--binary` fetches a non-markdown file as bytes — base64 under `--json`, an MCP embedded-resource block on the MCP surface; `--json --compact` for a minified columnar payload — see [Compact output](#compact-output)) |
| `ls` | List files and directories |
| `write` | Write document contents (comment-preserving, `--create` for new files, `--lines START-END` for partial writes) |
| `metadata` | Get document metadata (frontmatter, comment counts, pending status) |
| `cp` | Copy a managed file (markdown copied body-only — no comment blocks in the duplicate) |
| `mv` | Move a managed `.md` file (preserves comments and frontmatter) |
| `rm` | Remove a managed `.md` file |

### Search, query, and quality

| Command | Description |
|---------|-------------|
| `query` | Search across documents for comments (filter by `--pending`, `--pending-for`, `--pending-for-me`, `--pending-broadcast`, `--author`, `--since`, `--comment-id`, `--kind`; `--expanded` for inline comment details; `--json --compact` for a minified columnar payload, `--include-integrity` to add checksum/signature columns — see [Compact output](#compact-output)) |
| `search` | Full-text search across documents (supports `--regex`, `--scope`, `--context`, `--ignore-case`, and stateless `--limit`/`--offset` pagination with an exact `total`; `--json --compact` for a minified grouped columnar payload — see [Compact output](#compact-output)) |
| `lint` | Run structural lint checks on a document |
| `verify` | Verify comment integrity (checksums and signatures) |
| `activity` | Show what changed since a cutoff (caller's last action by default) — see [Tracking change](#tracking-change) |

### Sandbox and prompts

| Command | Description |
|---------|-------------|
| `sandbox add` | Stage a file under the caller's identity (soft claim) |
| `sandbox list` | List files currently sandboxed for the caller |
| `sandbox remove` | Unstage a file |
| `prompt set` | Define a folder-scoped system prompt in `.remargin.yaml` |
| `prompt resolve` | Resolve the nearest folder-scoped prompt for a file |
| `prompt list` | List all folder-scoped prompts in the realm |

### Session orchestration

| Command | Description |
|---------|-------------|
| `session launch` | Launch one Claude session per discovered identity into a multiplexer, one tab each (`--dry-run` discovery table, `--print` commands-only, `--multiplexer herdr\|tmux`, `--identity a,b`, `--backend`). Gated behind the `session` build feature — see [Session launch](#session-launch-multi-agent-orchestration). |

### Plan (universal dry-run)

```bash
remargin plan <op> <args>
```

Returns a projection of any mutating op (`ack`, `batch`, `comment`, `cp`, `delete`, `edit`, `migrate`, `purge`, `react`, `sandbox-add`, `sandbox-remove`, `sign`, `write`, `mv`) without touching disk. Reports `noop / would_commit / reject_reason / subset_gate / checksums / changed_line_ranges`.

### Maintenance

| Command | Description |
|---------|-------------|
| `migrate` | Convert old-format inline comments to remargin format |
| `purge` | Strip all comments from a document |
| `keygen` | Generate a new Ed25519 signing key pair |
| `identity` | Resolve and print the configured identity (also `identity show`). |
| `identity create` | Print a ready-to-use identity YAML block to stdout for `.remargin.yaml`. |
| `version` | Print version information |

### Integration

| Command | Description |
|---------|-------------|
| `mcp install [--user]` | Register as an MCP server in Claude Code |
| `mcp uninstall` | Remove MCP server registration |
| `mcp test` | Check MCP registration status |
| `mcp run` | Start the MCP server (stdio transport) |
| `claude plugin install` | Register the marketplace and install the Claude Code plugin |
| `claude plugin uninstall` | Uninstall the Claude Code plugin |
| `claude plugin test` | Check plugin installation status |
| `doctor` | Check realm health: hooks wired, config schema across the realm tree, identity/key resolvability, trusted-root existence, sandbox staging hygiene, project-scope settings. `--check=<set>` runs a named subset of checks |
| `claude restrict` | Add permission rules (sync to `.claude/settings.local.json` and `~/.claude/settings.json`) |
| `claude unrestrict` | Reverse a previous `restrict` cleanly |
| `registry show` | Display the participant registry |

### Global options

| Flag | Description |
|------|-------------|
| `--config <PATH>` | Path to config file |
| `--identity <NAME>` | Author name for this operation |
| `--type <human\|agent>` | Author type |
| `--mode <open\|registered\|strict>` | Enforcement mode |
| `--key <PATH>` | Path to Ed25519 signing key |
| `--assets-dir <PATH>` | Assets directory path |
| `--json` | Output as JSON |
| `--compact` | Compact columnar JSON, minified. Requires `--json`; supported by `get`, `query`, `activity`, and `search` today (see [Compact output](#compact-output)) |
| `--verbose` | Enable tracing output |

> To preview a mutating op without writing, use `remargin plan <op>`. The per-op `--dry-run` flag was removed in favour of the uniform `plan` projection.

### Compact output

`remargin get <file> --json --compact` emits a token-lean, minified variant of the `get` payload. It is the shape the MCP `get` tool returns unconditionally (the MCP surface has no format flag). Plain `--json` is unchanged.

- **With `--line-numbers`:** `{start_line, lines, links_cols, links}`. `lines` is an array of bare strings; line `i`'s number is `start_line + i` (no per-line `{line, text}` objects).
- **Without `--line-numbers`:** `{content, links_cols, links}` — `content` is the document text as one string.
- **`links`** rows are positional arrays named by `links_cols` (`["alias", "lines", "target", "title"]`); `alias` / `title` are `null` when absent. The verbose `count` (always `lines.len()`) and `path` columns are dropped. A link's on-disk path is derivable from `target`: verbatim when it carries a file extension, else `target + ".md"`.

`remargin query ... --json --compact` emits the same token-lean, minified variant of the `query` payload the MCP `query` tool returns unconditionally. Plain `--json` is unchanged (verbose `ExpandedComment` objects).

- Shape: `{base_path, comment_cols, results}`, where each result is `{path, comment_count, pending_count, pending_for, last_activity, comments}`.
- **`comments`** rows are positional arrays named once by the envelope's `comment_cols` header: `["id", "line", "author", "author_type", "ts", "reply_to", "thread", "to", "ack", "reactions", "remargin_kind", "edited_at", "attachments", "content"]` (`content` last). Acks compact to `author@ts` strings; the verbose per-comment `checksum` / `signature` and the redundant `file` are dropped. Nullable columns (`reply_to`, `thread`, `remargin_kind`, `edited_at`) are `null` when absent.
- **`--include-integrity`** (requires `--compact`) re-adds `checksum`, `signature` as columns immediately before `content`, widening both `comment_cols` and every row. On the MCP surface this is the `include_integrity: true` boolean.

`remargin activity --json --compact` emits the same token-lean, minified variant of the `activity` payload the MCP `activity` tool returns unconditionally. Plain `--json` is unchanged (verbose tagged `Change` objects); `--pretty` is unaffected.

- Shape: `{cutoff_explicit, newest_ts_overall, change_cols, files}`, where each file is `{path, newest_ts, cutoff_applied?, changes}`.
- **`changes`** rows are positional arrays named once by the envelope's `change_cols` header: `["ts", "kind", "author", "author_type", "comment_id", "line_start", "line_end", "reply_to", "to"]`. One uniform 9-column shape serves all three kinds; `kind` is `ack` / `comment` / `sandbox`. Columns a kind lacks are `null`: acks / sandboxes null the comment-only columns (`line_start`, `line_end`, `reply_to`) and their `to`; sandboxes also null `comment_id`. `to` is `[]` for a broadcast comment (vs `null` for the not-applicable acks / sandboxes). Timestamps keep full fidelity.

`remargin search <pattern> --json --compact` emits the same token-lean, minified variant of the `search` payload the MCP `search` tool returns unconditionally. Plain `--json` is unchanged (flat `SearchMatch` objects with PascalCase `location`).

- Shape: `{total, match_cols, files}` (plus `effective_limit` when a page was clamped), where each file is `{path, matches}`.
- Matches are grouped by file so `path` is stated once; files appear in first-match order and a file's rows are contiguous. Each match is a positional row named once by the envelope's `match_cols` header: `["line", "location", "text", "comment_id"]`. `location` is lowercase `body` / `comment`; `comment_id` is `null` for body matches.
- With `--context` / `-C` > 0 the rows widen to `["line", "location", "text", "comment_id", "before", "after"]` (`before` / `after` are string arrays) and `match_cols` reflects the widened arity.
- `total` is the exact corpus match count. On the MCP surface a page auto-sized under the session spill cap carries `effective_limit`; page by advancing the `offset` request param.

## Tracking change

The `remargin activity` command answers "what's new since X?" across managed `.md` files in the current realm. Per-file change records (comments, acks, sandbox-adds) are returned sorted by timestamp:

```bash
# What's new since I last acted (per-file caller-last-action cutoff).
remargin activity

# Explicit cutoff.
remargin activity --since 2026-04-20T00:00:00Z

# Human-readable timeline.
remargin activity --pretty
```

The default JSON output is the structured `ActivityResult` shape; `--json --compact` emits the token-lean columnar payload instead (see [Compact output](#compact-output)). `--pretty` switches to a per-file timeline rendered to stderr (so stdout stays clean for piping). Each per-file block opens with a header line that names the cutoff that was applied — `(since 2026-04-20 00:00)` for explicit `--since`, `(since you last touched this file: …)` for the caller-last-action default, and `(since the beginning — no prior activity by you in this file)` for the initial-touch fallback.

When `--since` is omitted, the per-file cutoff is the latest of (caller's authored comments, caller's acks, caller's sandbox-adds) in that file — files where the caller has never acted return everything (the "initial-touch" fallback). The command also folds in comment edits (via the `Comment.edited_at` field) and sandbox-roster timestamp refreshes, neither of which `comments` / `query` surface as distinct events.

Same surface is exposed via MCP as `mcp__remargin__activity`, which always returns the compact columnar payload (the MCP surface has no format flag).

## Typical workflows

### Multi-agent collaboration

```bash
# Three agents working on a shared spec, each with its own identity / key
# (configured in their respective .remargin.yaml or via --config).

# Each agent reads its inbox at the start of a turn
remargin query . --pending-for-me --pretty

# Posts work as comments addressed to the relevant peer
remargin comment spec.md "Choosing 'name' for the column key." \
  --reply-to pl1 --to engineer --auto-ack

# Or in one atomic batch
remargin batch spec.md --ops '[
  {"content": "Choosing name.", "reply_to": "pl1", "auto_ack": true},
  {"content": "Tagging for QA review.", "after_comment": "pl1", "to": ["qa"]}
]'

# At any point, see the full conversation tree
remargin comments spec.md --pretty

# Or what's changed since this agent last acted, across every file
remargin activity --pretty
```

### Document review (human + agent)

```bash
# Find documents with pending comments directed at you
remargin query . --pending-for-me

# Include broadcast conversations you haven't closed
remargin query . --pending-for-me --pending-broadcast

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

### Batch review

```bash
# Add multiple comments in one atomic operation
remargin batch docs/design.md --ops '[
  {"content": "Good approach here.", "after_line": 10},
  {"content": "Edge case: what if input is empty?", "after_line": 35},
  {"content": "This contradicts section 2.", "after_line": 78}
]'
```

### Migration from older formats (existing users only)

If you have documents using the older `user comments` / `agent comments` fenced block format:

```bash
# Preview what would change
remargin plan migrate docs/old-doc.md

# Convert
remargin migrate docs/old-doc.md
```

## Exit codes

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

# Run clippy (strict — the project enforces deny-all clippy lints)
cargo clippy
```

## Contributing

Contributions are welcome.

1. **Fork and branch** — create a feature branch from `master`.
2. **Keep changes focused** — one feature or fix per PR.
3. **Follow existing patterns** — the codebase uses strict clippy lints (all, pedantic, restriction, nursery levels). Run `cargo clippy` before submitting.
4. **Write tests** — the project uses `assert_cmd` and `tempfile` for integration tests.
5. **Update the skill** — if you add or change MCP tools, update `crates/remargin-core/skill/SKILL.md` accordingly.
6. **Commit messages** — conventional commits (`feat:`, `fix:`, `chore:`, etc.).

### Development setup

```bash
git clone https://github.com/tixena/remargin.git
cd remargin
cargo build
cargo test
```

The project uses Rust 2024 edition with strict clippy enforcement. If clippy complains, fix the lint — don't suppress it unless there's a documented reason in `Cargo.toml`.

## License

Remargin is open source under the [MIT License](LICENSE).

Made by [Tixena Labs](https://tixenalabs.com/).
