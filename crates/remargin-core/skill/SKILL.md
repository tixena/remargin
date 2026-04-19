---
name: remargin
description: "Document access layer and structured commenting system for markdown files. Use remargin MCP tools (ls, get, write, comment, ack, query) instead of Read/Edit/Write when working with markdown documents. Provides threaded multi-player comments, integrity checks, batch operations, sandbox staging, plan-based previews for every mutating op, and file access through MCP."
user-invocable: true
---

# Remargin

Remargin is a **document access layer** and **structured commenting system** for markdown files. It replaces direct filesystem access with a set of MCP tools (and a mirrored CLI) that read, write, comment on, and query markdown documents while enforcing identity, signatures, comment-preservation, and a per-directory enforcement mode.

## When to use remargin

Use remargin when:

- Reading or writing markdown documents under review or discussion.
- Adding comments, replies, reactions, or acknowledgments to documents.
- Searching across documents for pending comments or activity.
- Working in a project where the remargin MCP server is configured or the `remargin` CLI is on `$PATH`.

**Trigger phrases**: "remargin that", "remargin this", "let's discuss", "let's review", "review this document", "discuss this document", "comment on", "what comments are pending", "acknowledge", "react to", "check the document", "start a discussion", "leave a comment", "any pending comments".

## Which surface: MCP or CLI?

**If the `mcp__remargin__*` tools are available in your context, use MCP.** The CLI (`remargin ...`) is a fallback for shell / script contexts only.

Reasons:

- MCP inputs are structured JSON — no shell-escape hazards on content containing quotes, backticks, `$`, or `---`.
- MCP returns structured JSON — no output parsing guesswork.
- Both surfaces share the same core; MCP and CLI produce equivalent results. There is no MCP-only or CLI-only feature in the mutating surface.

The CLI has a small admin surface that has no MCP equivalent (`keygen`, `mcp`, `obsidian`, `registry`, `resolve-mode`, `skill`, `version`). Those are user-facing setup tools, not agent tools.

## Critical rule: never operate on managed files directly

**NEVER use `Read`, `Edit`, `Write`, or `Bash` (`awk`, `sed`, `cat`, `grep`) to read, modify, or inspect markdown documents remargin manages.** Always go through the remargin tools.

- Use `remargin get` to read file contents (with `start_line`/`end_line` for ranges, `line_numbers=true` to prefix each line).
- Use `remargin search` to find text across files and get line numbers.
- Use `remargin write` to update body content.
- Use `remargin comment` / `batch` to add comments.
- Use `remargin comments` to list comments in a file.

This ensures:

- Comments are never accidentally deleted or corrupted.
- Document integrity (checksums, signatures) is preserved.
- Comment threading and acknowledgment state stays consistent.
- Structural lint checks run before and after every operation.

Use `Grep` and `Glob` only for discovery (finding files across the repo), not for reading or modifying managed documents.

## The identity rule — read this before posting anything

**Never pass `identity`, `type`, `config_path`, `--identity`, `--type`, or `--config` overrides unless the user has explicitly asked you to act as someone else.** Your identity is resolved from the configured registry and signing key. Overriding = impersonation. This rule surfaced directly from a real incident (2026-04-16) where an agent posted comments under the wrong identity because the override path was taken without justification.

- ❌ `remargin comment file=doc.md content="..." --identity eduardo --type human` — agent is impersonating a human.
- ❌ `mcp__remargin__comment { file, content, identity: "someone-else", type: "human" }` — same impersonation via MCP.
- ✅ `remargin comment file=doc.md content="..."` — uses the configured identity (yours).
- ✅ `mcp__remargin__comment { file, content }` — same, via MCP.

If the user tells you "post this as the release bot" or similar, the override is warranted — keep the original request in context so you can show it if questioned.

Every mutating tool (`comment`, `batch`, `edit`, `delete`, `ack`, `react`, `sandbox_add`, `sandbox_remove`, `plan`, `write`, `migrate`, `purge`, `sign`) accepts the identity-override quartet `{config_path, identity, type, key}` (with `config_path` mutually exclusive with the other three). Treat every one of those sites as an impersonation risk.

## Permissions setup

If remargin MCP tools require approval on every call, ask the user to add this wildcard to their `settings.local.json` (or `settings.json`) allow list:

```json
{
  "permissions": {
    "allow": [
      "mcp__remargin__*"
    ]
  }
}
```

This approves all remargin tools at once — no per-tool confirmation needed.

## Core ops reference

Each op lives at both surfaces: the MCP tool name is `mcp__remargin__<op>`; the CLI invocation is `remargin <op>`. Only the highest-impact flags are shown inline — run `remargin <op> --help` for the exhaustive list.

### Document access

| Op | Purpose |
|----|---------|
| `ls` | List files and directories (supports `path`). |
| `get` | Read a file (supports `start_line`/`end_line`, `line_numbers`, `binary`). Run `metadata` first to check size on binary reads. |
| `write` | Write file content (comment-preserving). Use `create=true` for new files, `raw=true` for non-markdown, `binary=true` for base64 bytes, `start_line`/`end_line` for partial writes. |
| `metadata` | Document metadata (frontmatter, comment counts, pending counts, mime, size). |
| `rm` | Remove a file from the managed tree. |

### Commenting

| Op | Purpose |
|----|---------|
| `comment` | Add a comment. Supports `reply_to`, `after_line`, `after_comment`, `auto_ack`, `attachments`, `to`, `sandbox`. |
| `comments` | List comments in a file. `pretty=true` for human-readable threaded display. |
| `batch` | Add multiple comments atomically (single write, single verify). Each sub-op supports its own `auto_ack`. |
| `edit` | Edit an existing comment. Cascades ack-clear to children. |
| `delete` | Delete one or more comments. Cleans up attachments. |
| `ack` | Acknowledge one or more comments. Omit `file` to resolve by ID across the directory tree (scoped by `path`). |
| `react` | Add or remove an emoji reaction. Use `remove=true` to unreact. |

### Sandbox

Sandbox staging is a per-identity, per-file marker ("I am working on this") stored in document frontmatter. It does not hide or copy the file — it is a soft-claim, surface-able via `sandbox_list`.

| Op | Purpose |
|----|---------|
| `sandbox_add` | Stage one or more markdown files in the caller's sandbox. Idempotent per identity. |
| `sandbox_remove` | Remove the caller's sandbox entry from one or more markdown files. Idempotent. |
| `sandbox_list` | List markdown files in a directory that are currently staged for the caller's identity. |

### Search and quality

| Op | Purpose |
|----|---------|
| `query` | Search across documents for comments. Filters: `pending`, `pending_for`, `author`, `since`, `comment_id`. Use `expanded=true` to include matching comments inline. |
| `search` | Search across documents for text. Supports `regex`, `scope` (all/body/comments), `context` lines, `ignore_case`. |
| `lint` | Structural lint checks on a document. |
| `verify` | Verify comment integrity (checksums and signatures) against the participant registry. |
| `migrate` | Convert old-format inline comments to remargin format. |
| `purge` | Strip all comments from a document (destructive — user-initiated only). |

### Dry-run projection

`plan` is the pre-commit projection: it simulates a mutating op and returns the predicted outcome (noop status, would-commit flag, reject-reason, checksums, changed line ranges, comment diff) **without touching disk**. Use it whenever you want to preview an op before committing, especially in strict mode or before a batch.

| Op | Purpose |
|----|---------|
| `plan` | Projection for any mutating op. Takes `op` plus the same arguments you would pass to the mutating call. All 11 mutating ops are wired: `ack`, `batch`, `comment`, `delete`, `edit`, `migrate`, `purge`, `react`, `sandbox-add`, `sandbox-remove`, `write`. |

### Admin (CLI-only)

These are user-facing setup tools and do not appear as MCP tools:

- `keygen` — generate an Ed25519 signing key pair.
- `mcp` — run the stdio MCP server (entry point for `mcp__remargin__*`).
- `obsidian` — install / uninstall the Obsidian vault plugin (feature-gated).
- `registry` — manage the participant registry file.
- `resolve-mode` — resolve the effective enforcement mode for a directory.
- `skill` — manage the Claude Code skill (this file).
- `identity` — resolve and print the configured identity.
- `version` — print version information.

## Agent-safety rules

Each rule below states a clear "don't" with paired ✅/❌ examples. They compound: a single violation can break multiple rules at once.

### Identity override

Already covered above; it is rule number one. Don't override `identity` / `author_type` without explicit user instruction.

### Reply threading

Always set `reply_to` when you are replying. A comment without `reply_to` is a **new thread at the current insertion point**, not a reply — it will not be grouped under the message you meant to answer, and the parent's author will not get a pending-for count decrement when you ack it.

- ❌ `remargin comment file=doc.md content="Good question. I'd add a revoked_keys list." after_comment=abc` — creates a sibling comment, not a reply.
- ✅ `remargin comment file=doc.md content="Good question. I'd add a revoked_keys list." reply_to=abc` — threaded reply.

### Auto-ack discipline

`auto_ack: true` is allowed only when you are replying to a comment that was addressed to you (via the `to` field) and your reply fully addresses it. Auto-acking a comment addressed to someone else is speaking on their behalf.

- ❌ Auto-acking a comment addressed to `eduardo` while signed in as `claude-agent`.
- ✅ Auto-acking a comment addressed to you (`to: [claude-agent]`) whose content your reply has fully answered.

`auto_ack` without `reply_to` is rejected by the core — treat that as intentional guardrail, not a bug.

### Line-number volatility

Comment IDs are stable; line numbers are not. Any mutation (comment, edit, delete, write) shifts every subsequent line number in the file. Never hold a line number across more than one mutation.

- ❌ Run `search` for "error handling", get line 42, then later in the session post a comment with `after_line=42`. If anything was inserted in the meantime, line 42 is not what it used to be.
- ✅ Anchor to comment IDs or heading text when possible (`after_comment=abc`). If you must use a line, re-resolve it via `search` or `get line_numbers=true` immediately before the mutation.

### Batch for multiple mutations on the same file

When placing 2 or more comments on the same file, **always use `batch`**. Do not call `comment` sequentially.

Why: each `comment` call inserts a block, shifting subsequent line numbers. If you comment at line 50, line 80 in the original file is now line 90. Your second `comment after_line=80` lands in the wrong place. `batch` resolves all line numbers against the original document in one atomic pass.

- ❌ Two back-to-back `comment` calls referencing original-document line numbers.
- ✅ `batch operations=[{content: "...", after_line: 50}, {content: "...", after_line: 80}, {content: "...", reply_to: "abc", auto_ack: true}]`

### Strict-mode awareness

Before composing in an unknown directory, run `remargin resolve-mode` (or the mirrored MCP tool if you have one wired). Possible values:

- `open` — anyone may post; no signatures required.
- `registered` — only identities in the registry may post; still no signatures.
- `strict` — registered identities only, and every comment must carry a valid Ed25519 signature.

In strict mode, an unsigned or unregistered post is rejected by the verify gate that runs before every write (rem-ef1). If you are not sure whether your identity has a signing key configured, run `remargin identity` first. Do not assume an earlier op succeeding implies future ops in the same mode will.

### Sandbox ≠ commit

`sandbox_add` is a two-step workflow: `add` returns a staged marker. The file is not "committed" or "submitted" — that is an adapter-level concept `sandbox` does not enforce. If a user says "stage this for review", `sandbox_add` is the right call; if they say "submit this", clarify first.

### Don't delete others' comments

Never delete another participant's comment unless the user explicitly tells you to. If an operation fails because of someone else's comment, find an alternative approach or ask the user. Deleting to unblock your own operation is never acceptable.

### `write` safety

`write` replaces the **entire file content**. All existing comments must be carried intact in the new content — the checksum on each comment validates this. If the write would remove or alter any comment, remargin rejects it.

- Never use `write` to rewrite a file that has other participants' comments without reading the full file first. Read it with `get`, make targeted body changes only, and write back the complete content including all comment blocks verbatim.
- If a `write` fails due to comment preservation, do **NOT** delete comments to make it work. Find an alternative approach or ask the user.
- For non-markdown files, pass `raw=true`. For binary files, pass `binary=true` with base64 content (rejected for `.md`).

## Comment format

Comments use a fenced code block with language tag `remargin` and a YAML header. **You do not write this format manually.** The tools produce it.

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

It can be multiple paragraphs with **markdown formatting**.
```
````

Threading and acknowledgment add more keys:

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

## Common workflows

### Read a document

```
remargin get path="docs/design.md"
remargin get path="docs/design.md" start_line=1 end_line=50
remargin get path="docs/design.md" line_numbers=true
```

### List files

```
remargin ls path="docs/"
```

### Add a comment

```
remargin comment file="docs/design.md" content="This section needs more detail on error handling."
```

### Reply to a comment

```
remargin comment file="docs/design.md" content="Good point, I'll expand this." reply_to="abc"

# Reply and acknowledge the parent in one atomic write (only when addressed to you)
remargin comment file="docs/design.md" content="Addressed." reply_to="abc" auto_ack=true
```

### Add a comment at a specific anchor

```
remargin comment file="docs/design.md" content="Consider edge case X here." after_line=42
remargin comment file="docs/design.md" content="Also relevant here." after_comment="abc"
```

### Multiple comments at once (atomic)

```
remargin batch file="docs/design.md" operations='[
  {"content": "First note", "after_line": 50},
  {"content": "Second note", "after_line": 80},
  {"content": "Reply to abc", "reply_to": "abc", "auto_ack": true}
]'
```

On MCP, the same call takes a structured `operations` array directly — no JSON string encoding.

### Acknowledge comments

```
remargin ack file="docs/design.md" ids=["abc", "def"]

# Folder-wide ack (resolves by ID across the directory tree)
remargin ack ids=["abc"]
remargin ack ids=["abc"] path="docs/"

# Unacknowledge
remargin ack file="docs/design.md" ids=["abc"] remove=true
```

### React to a comment

```
remargin react file="docs/design.md" id="abc" emoji="👍"
remargin react file="docs/design.md" id="abc" emoji="👍" remove=true
```

### Find pending comments across documents

```
remargin query path="docs/" pending=true
remargin query path="." pending_for="eduardo"
remargin query path="." pending_for="eduardo" expanded=true
remargin query path="." comment_id="abc"
```

### Search text across documents

```
remargin search pattern="notification"
remargin search pattern="error" path="docs/" scope="comments"
remargin search pattern="TODO|FIXME" regex=true ignore_case=true context=2
```

### Dry-run a mutation

```
# Preview what `comment` would produce without writing
remargin plan comment file="docs/design.md" content="Preview this first."

# Preview a batch
remargin plan batch file="docs/design.md" ops='[...]'

# Preview a write (returns reject_reason if raw/binary/unsupported)
remargin plan write file="docs/design.md" content="..."
```

On MCP, `plan` takes the op name as a field: `plan { op: "comment", file, content, ... }`.

### Stage files for review (sandbox)

```
remargin sandbox add files=["docs/design.md", "docs/api.md"]
remargin sandbox list path="docs/"
remargin sandbox remove files=["docs/design.md"]
```

### Write document content

```
remargin write path="docs/design.md" content="Updated content here..."
```

`write` preserves all existing comments. It will not destroy comment blocks.

**Non-markdown files:** use `raw=true`.

```
remargin write path="config/settings.json" content='{"key": "value"}' raw=true
remargin write path="assets/data.yaml" content="name: example" raw=true create=true
```

`raw=true` is rejected for `.md` files — markdown always goes through the comment-preserving path.

**Binary files:** use `binary=true` (base64-encoded `content`); implies `raw=true`.

**Partial writes:** use `start_line`/`end_line` (1-indexed inclusive) to replace a specific line range without rewriting the whole file. Incompatible with `create`, `raw`, and `binary`.

### Create a new document

```
remargin write path="docs/new-doc.md" content="# New Document\n\nInitial content." create=true
```

`create=true` fails if the file already exists (prevents accidental overwrites).

### Fetch binary content

To fetch non-markdown files (images, PDFs, audio, etc.) as bytes, pass `binary=true` to `get`. The response carries `mime`, `size_bytes`, `path`, and the bytes themselves base64-encoded in `content`.

```
remargin get path="assets/screenshot.png" binary=true
```

**Before fetching binary content, always call `metadata` first** to check `size_bytes` and `mime`. Base64 inflates payloads by ~33%, so large blobs through JSON mode are the caller's responsibility.

```
remargin metadata path="assets/screenshot.png"
# -> { binary: true, mime: "image/png", size_bytes: 48321, ... }
```

`binary=true` is rejected for `.md` files — markdown must go through the text path so comment preservation is never bypassed.

On the CLI, `get --binary --out <path>` writes the bytes to a file and prints only a summary:

```
remargin get --binary --out /tmp/pic.png assets/screenshot.png
```

### Review a document (full workflow)

1. `ls` to find the document.
2. `get` to read its contents.
3. `comments` to see existing discussion.
4. `comment` or `batch` to add your review comments.
5. Process and `ack` comments addressed to you (see "Processing comments addressed to you" below).
6. `query` to check for anything else pending.

### Processing comments addressed to you

When comments are addressed to you (via `to` field) or the user asks you to "process" comments, follow this workflow **in order**. Ack is the **last step**, not the first.

1. **Read** the comment and any referenced documents, links, or files.
2. **Reason** about what the comment is saying — what is the author asking, deciding, or informing you about?
3. **Execute** any actionable items:
   - If the comment asks you to read something, read it and form an understanding.
   - If the comment asks you to do work, do the work (create files, update docs, write code, create tasks).
   - If the comment makes a decision, update your plans and any affected documents accordingly.
   - If the comment asks a question, reply with a substantive answer (not a summary of the question).
4. **Reply** with a comment (via `reply_to`) that demonstrates you did the work — reference specifics, share conclusions, raise concerns. Do not reply with summaries of what the comment said back to the person who wrote it.
5. **Ack** the comment only after all the above is complete. Ack means "I have fully addressed this." A premature ack is a lie — it tells the author their comment was handled when it wasn't.

**When NOT to reply:**

- When someone agrees with your comment ("Agreed", "Sounds good"), just ack their reply and move on. Do NOT create a new comment just to say "Acked." or "Noted." — it adds zero information and creates a pending item the other person has to clear. Only reply if you have something substantive to add.

**Common mistakes to avoid:**

- Do NOT ack immediately after reading. Ack is not "I read this."
- Do NOT reply with a surface-level summary.
- Do NOT ack and then start doing the work. Work first, ack second.
- Do NOT skip referenced files. If the comment says "look at X", read X before acking.
- Do NOT reply to agreements with "Acked." — that's noise, not communication.

## Comment display format

The `comments` tool supports two output modes:

- **Default (no flag)**: returns JSON. Use when you need to process comment data programmatically (to ack, reply, filter, or reason about content).
- **`pretty=true`**: returns a pre-formatted, human-readable threaded display. Use when the user asks to see comments interactively ("show me the comments", "what comments are pending", "review this document").

**CRITICAL: MCP tool results are not visible to the user.** The user only sees the tool call indicator in their terminal, not the returned content. When using `pretty=true`, you **must** copy the full result into your text response so the user can actually see it. Calling the tool alone is not enough.

### When to use `pretty=true`

When the user asks to see, review, or display comments, use `pretty=true` and **pass the output through verbatim**. Do not paraphrase, summarize, or re-render. The tool produces the exact format for terminal display with ctrl+clickable `file:line` links.

**After showing pretty output, STOP.** Do not add summaries, reformatted lists, or any restatement of comment data below the tool output. The pretty output is the complete answer. Any text you write that references comment IDs, line numbers, or content from memory will be wrong.

```
remargin comments file="docs/design.md" pretty=true
```

### When to use default JSON

When you need to process comments programmatically — to ack them, reply to them, filter them, or reason about their content — use default JSON.

```
remargin comments file="docs/design.md"
```

### Pretty output format reference

The `pretty=true` output uses this format (produced by the tool, not by the agent):

#### Single comment

```
docs/design.md:25
  abc · eduardo (human) · 2026-04-06 14:32
  │ The comment content goes here, wrapped
  │ across multiple lines as needed.
  │ pending
```

#### Threaded reply (indented under parent)

```
docs/design.md:25
  abc · eduardo (human) · 2026-04-06 14:32
  │ I think the registry should support key rotation.
  │ What happens when someone's key is compromised?
  │ pending

  docs/design.md:35
    xyz · claude (agent) · 2026-04-06 14:33
    │ ⤷ reply-to: abc
    │ Good question. I'd add a `revoked_keys` list to the
    │ registry entry. When a key is revoked, all comments
    │ signed with it get flagged but not deleted.
    │ ✓ acked by eduardo @ 2026-04-06 15:00
```

#### Footer

```
─────
3 comments · 2 pending
```

## Escape hatches

- **Preview before committing.** Any mutating op can be previewed via `plan`. Use it when you are unsure about strict-mode gating, noop detection, or comment placement. `plan` is the single preview surface — the per-op `--dry-run` flag was removed.
- **Verify failures.** If `verify` reports a mismatch, do NOT rewrite the file to "fix" the checksum — that is the symptom, not the cause. Surface the mismatch to the user; it usually means a manual edit or a cross-identity signing issue.
- **Clobbered files.** If a write fails comment preservation, re-read the current file via `get`, re-build the correct content, and retry. Never delete comments to unblock.
- **Not sure which mode.** Run `remargin resolve-mode` and `remargin identity` before composing in unfamiliar directories.

## Key concepts

- **Identity**: every comment has an author (string identifier) and type (`human` or `agent`).
- **Threading**: comments can reply to other comments via `reply_to` (direct parent) and `thread` (root ancestor).
- **Acknowledgment**: comments are acknowledged with `ack`, recording who and when (full timestamp).
- **Integrity**: every comment gets a checksum. In strict mode, comments are also signed with Ed25519 keys.
- **Batch atomicity**: multiple comment operations in one `batch` call produce a single document write and a single verify pass.
- **Comment preservation**: the tools guarantee no comments are lost during writes — the before and after comment list must match exactly with only the expected delta.
- **Noop**: a write producing byte-identical content to the on-disk file returns `noop: true` without touching the file — retries and idempotent re-submits settle here without disturbing mtime.
- **Sandbox**: a per-identity marker claimed via `sandbox_add`, listed via `sandbox_list`, and released via `sandbox_remove`. Persisted in document frontmatter. Not the same as "committed" or "submitted" — it is a soft claim only.
- **Plan**: a projection (`plan <op>`) that returns the predicted outcome of a mutating op without writing anything. This is the one and only preview surface — per-op `--dry-run` flags were removed. Covers every mutating op.
