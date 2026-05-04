---
name: remargin
description: "Document access layer and structured commenting system for markdown files. Use remargin MCP tools (ls, get, write, comment, ack, query) instead of Read/Edit/Write when working with markdown documents. Provides threaded multi-player comments, integrity checks, batch operations, sandbox staging, plan-based previews for every mutating op, and file access through MCP."
user-invocable: true
---

# Remargin

Remargin is a **document access layer** and **structured commenting system** for markdown files. It replaces direct filesystem access with MCP tools (and a mirrored CLI) that read, write, comment on, and query markdown documents while enforcing identity, signatures, comment-preservation, and a per-directory enforcement mode.

A **realm** is any directory tree containing a `.remargin.yaml` (discovered by walking up from cwd, like `.git`). **Every `.md` inside a realm is managed.** No per-file opt-in.

**Trigger phrases**: "remargin that", "let's discuss", "review this document", "comment on", "what comments are pending", "acknowledge", "any pending comments".

---

## Critical rules (read first, scan often)

1. **Realm scope.** Every `.md` inside a realm is managed. NEVER use `Read` / `Edit` / `Write` / `Bash` (`cat`, `sed`, `awk`, `cp`, `mv`, `tee`, redirection) on a managed file. Always go through remargin tools.
2. **MCP > CLI.** If `mcp__remargin__*` tools are reachable, use MCP. The CLI is a shell-context fallback only.
3. **At least one reply per comment, threaded via `reply_to`.** Use `batch` for N replies in one turn. If a comment raises two distinct subjects, post two replies (each its own thread under the same parent) — that's cleaner than one reply that mixes them. What's NEVER ok is bundling answers to multiple **separate** comments into one consolidated reply.
4. **Ack only AFTER the action is complete.** Ack is not a read-receipt. Pending comments are work items, not background context.
5. **Don't return pending comments to the user as their to-do** when the action is yours.
6. **Line numbers shift on every mutation.** Re-resolve immediately before any line-anchored op, or use `batch` for multi-step.
7. **`--config` XOR `(--identity + --type + --key)`.** Three branches: `--config FILE` alone, full triplet, or filter on the walked candidate. Mixing those two halves in one call is rejected at parse time — the CLI errors before the op runs.
8. **`auto_ack: true` is allowed only when** (a) the parent comment is addressed to you via `to:` AND (b) your reply fully resolves the ask. `auto_ack` without `reply_to` is rejected.
9. **Never declare a different identity per call** unless the user explicitly asked. Per-call `identity` / `type` / `config_path` to declare someone else = impersonation.
10. **Never delete other participants' comments** to unblock your own op. Find another path or ask the user.
11. **Use `remargin activity` to find what's new in managed `.md` since you last acted.** Do not hand-roll timestamps from `comments` / `query` for this purpose — those tools don't compute the per-file caller-last-action cutoff and don't fold edits / re-sandboxes into a single change list.

---

## Decision flowcharts

Each section starts with the question an agent is actually asking.

### Q: I have N pending comments to reply to. What do I do?

This is the most common multi-comment workflow. Use `batch`. **Do not** bundle into one comment, **do not** post N sequential `comment` calls.

1. List them: `remargin query --pending --pretty <folder>` (CLI) or `mcp__remargin__query` with `pending: true`.
2. For each comment, complete the action it asks (file the bd task, update the doc, run the verification, etc.).
3. Reply to all N via ONE `batch` call:

   ```
   remargin batch --ops '[
     {"content": "answer to A...", "reply_to": "abc"},
     {"content": "answer to B...", "reply_to": "def", "auto_ack": true},
     {"content": "answer to C...", "reply_to": "ghi"}
   ]' file.md
   ```

   Or via `mcp__remargin__batch` with the same shape (no JSON-string encoding).

4. `auto_ack: true` per op only when (a) the parent is addressed to you via `to:` AND (b) your reply fully resolves the ask.
5. For broadcasts (`to: []`) or comments addressed to others, leave `auto_ack` off and ack separately via `remargin ack` if appropriate.

❌ **Never:** post one comment that summarizes answers to N other comments. Forces the user to re-thread mentally or reply with their own consolidation. Defeats threading. This was a real failure pattern — don't repeat it.

❌ **Never:** call `remargin comment` N times in sequence on the same file. Line numbers shift between calls; subsequent inserts land in the wrong places.

### Q: What's new in managed `.md` since I last acted?

`remargin activity [<PATH>] [--since <ISO>] [--pretty]`. Walks
managed `.md` files and returns per-file change records (comments,
acks, sandbox-adds) sorted by ts. With `--since` omitted, the
per-file cutoff is the caller's last action in that file (max of
the caller's authored comments, acks, and sandbox-adds in that
file); files where the caller has never acted return everything —
the "initial-touch" fallback. JSON is the default; `--pretty`
renders a human-readable timeline.

Use this instead of stitching `comments` / `query` calls together
with hand-rolled timestamps. The activity command also folds in
edits (via `Comment.edited_at`) and re-sandboxes (via the
sandbox-add timestamp refresh) — neither of which `comments` /
`query` surface as distinct events.

### Q: A pending comment is just FYI / acknowledgment-only. What do I do?

If the content is "ok", "got it", "thanks", "noted", or pure information with no actionable payload — **ack immediately**. No reply needed.

### Q: I want to leave multiple comments at once (not all replies).

Same answer as above: `batch`. Each op can independently be a reply (`reply_to`), an anchor at a line (`after_line`), an anchor under a comment (`after_comment`), or a top-level comment.

```
remargin batch --ops '[
  {"content": "Edge case here", "after_line": 42},
  {"content": "Reply to abc", "reply_to": "abc", "auto_ack": true},
  {"content": "Sibling under abc", "after_comment": "abc"}
]' file.md
```

`batch` resolves all line numbers against the original document in one atomic pass — sequential `comment` calls do not.

### Q: I need to read/modify a managed `.md` file.

| Need | Tool |
|---|---|
| Read full file | `get path=...` |
| Read a range | `get path=... start_line=N end_line=M` |
| Read with line numbers | `get path=... line_numbers=true` |
| Read binary (non-md) | `get binary=true` (run `metadata` first to check `size_bytes`) |
| Search text | `search pattern=... [scope=all|body|comments] [regex=true]` |
| Replace whole file | `write path=... content=...` (comment-preserving) |
| Replace a line range | `write path=... start_line=N end_line=M content=...` |
| Create a new file | `write path=... content=... create=true` |
| Write non-markdown | `write path=... content=... raw=true` |
| Delete a file | `rm path=...` |

**Do not** use `Read` / `Edit` / `Write` / `Bash` shell tools on managed `.md` files. The realm rule has no exceptions.

### Q: How do I declare identity for a mutating call?

Three exclusive branches — pick exactly one:

| Branch | Pattern | When |
|---|---|---|
| **Config alone** | `--config FILE` (CLI) / `config_path: "FILE"` (MCP) | The file declares a complete identity. Mutually exclusive with the other three — mixing is rejected before the op runs. |
| **Full triplet** | `--identity NAME --type human|agent --key PATH` | Declaring a complete identity inline. |
| **Filter (or none)** | Subset of triplet, or no flags | Args narrow the walked candidate set. Zero or many matches = error. |

**Default**: don't declare anything. The walked `.remargin.yaml` resolves your identity. Per-call declaration of someone else's identity is impersonation.

### Q: A user asked to "show comments" / "what's pending" — what do I return?

Use `pretty=true` on `comments` or `query`, then **paste the full output verbatim into your text response.** MCP results are not visible to the user; calling the tool alone is not enough. Do not paraphrase or summarize.

```
remargin comments file=... pretty=true
```

After showing pretty output, **stop**. Do not add summaries, reformatted lists, or restatements — they will be wrong (memory/state mismatch) and noisy.

### Q: How do I anchor a new comment to a specific place in the file?

| Anchor | Field | Stable across mutations? |
|---|---|---|
| Comment ID | `after_comment="abc"` or `reply_to="abc"` | Yes — IDs are stable. |
| Line number | `after_line=42` | **No** — re-resolve via `search` or `get line_numbers=true` *immediately* before the call. |
| Heading text | search → `after_line` | Re-resolve same as line. |

For >1 line-anchored insert, use `batch` (one atomic pass).

### Q: I'm about to mutate something — should I preview first?

`plan` is the universal preview. Takes the same args as the underlying op; returns `{noop, would_commit, reject_reason, ...}` without touching disk.

```
remargin plan comment file=... content="..."
remargin plan write file=... content="..."
remargin plan batch file=... ops='[...]'
```

`plan` is the only preview surface and covers every mutating op: `ack`, `batch`, `comment`, `delete`, `edit`, `migrate`, `purge`, `react`, `sandbox-add`, `sandbox-remove`, `sign`, `write`.

### Q: I'm in an unfamiliar directory. What do I check first?

```
remargin resolve-mode      # open | registered | strict
remargin identity          # who am I? do I have a key wired?
remargin permissions show  # what's restricted in this realm?
```

In strict mode, an unsigned or unregistered post is rejected by the verify gate before write. Don't assume an earlier op succeeding implies the next will.

### Q: I want to restrict (or unprotect) a path.

1. `remargin restrict <path>` — appends an entry to
   `<.claude-anchor>/.remargin.yaml` AND syncs the equivalent rules
   into `.claude/settings.local.json` + `~/.claude/settings.json`.
   Layer 1 (remargin-core) starts refusing ops on the path on the
   very next call. Layer 2 (Claude's NATIVE Read/Edit/Write/Bash
   tools) takes effect when Claude reloads its settings (typically
   a Claude restart — outside remargin's control).
2. `remargin unprotect <path>` — exact reverse. Uses a sidecar
   (`<.claude-anchor>/.claude/.remargin-restrictions.json`) to know
   precisely which rules to remove; never touches user-added rules.
3. `remargin permissions show` — print the resolved permissions
   tree at cwd. JSON via `--json`.
4. `remargin permissions check <path> [--why]` — gitignore-style:
   exit 0 when restricted, 1 when not.

Wildcard form: `remargin restrict "*"` and `remargin unprotect "*"`
cover the entire realm anchored at the matching `.remargin.yaml`.

Optional flags:
- `--also-deny-bash <cmd>` (repeatable) — extra Bash command names
  to deny on the restricted path (e.g. `curl`, `wget`).
- `--cli-allowed` — keep the `remargin` CLI usable on the path
  (only the MCP / agent surfaces are blocked).

No identity flags. Editing your own permissions doesn't need an
identity declaration.

---

## Anti-patterns (consolidated)

Each is a concrete failure mode that has bitten a session.

❌ **Bundling N replies into one comment.** Defeats threading. Use `batch` with one op per reply.

❌ **N sequential `comment` calls on the same file.** Line numbers shift between calls; the second/third lands wrong. Use `batch`.

❌ **Acking before doing the work.** Ack signals "done." Doing it in the wrong order makes the ack a lie.

❌ **Listing pending comments to the user as their to-do** when the action is yours. Pendings are work items, not background.

❌ **Using CLI when MCP is reachable.** Shell-escape hazards on `$`, backticks, `---`. Loses type-safety. More permission prompts.

❌ **`Read` / `Edit` / `Write` / `Bash` on a managed `.md`.** No per-file opt-out. Realm scope is total.

❌ **`auto_ack: true` on a comment addressed to someone else.** Speaks on their behalf.

❌ **Trusting line numbers across two mutations.** Comment IDs are stable; line numbers aren't.

❌ **Per-call identity declaration without explicit user instruction.** Impersonation.

❌ **Replying with a summary of the original comment** instead of doing the work. Reply demonstrates substance, not paraphrase.

❌ **Cross-referencing internal IDs in user-facing replies OR in document bodies** ("see Decision 13", "as in `xyz`", "per the `abc` thread", "(per `25w`)"). Agents track IDs; users read linearly. The doc body is even worse than replies for this — comments get cleaned up after a discussion, leaving doc-body citations as dangling references that no one can resolve. Restate the relevant content inline. Both replies and doc bodies must stand on their own without knowledge of the comment thread that produced them.

❌ **Replying "Acked." or "Noted." to an agreement.** Adds zero info, creates a pending the other person has to clear. Just ack and move on.

❌ **`write` that drops comments to unblock yourself.** If preservation fails, re-read with `get`, rebuild correct content, retry. Never delete others' comments.

❌ **Rewriting a file to "fix" a verify mismatch.** That's the symptom, not the cause. Surface to the user.

---

## Worked examples

### Reply to 5 pending comments on the same doc, threaded

```
remargin batch --ops '[
  {"content": "Mechanism: we project intent into Claude permissions...", "reply_to": "qp7"},
  {"content": "Done — added recursive respect subsection.", "reply_to": "nvf"},
  {"content": "Added deny_ops to the schema.", "reply_to": "ru2", "auto_ack": true},
  {"content": "Confirmed; sidecar tracks for clean reversal.", "reply_to": "c3e"},
  {"content": "Architecture corrected per your note.", "reply_to": "uyg"}
]' src/discussions/design.md
```

### Update a doc body via partial write

```
remargin write --lines 16-16 src/discussions/roadmap.md <<'EOF'
- Status: `open` ([rem-8cnc](bd://rem-8cnc))
EOF
```

Comment blocks elsewhere in the file are preserved automatically.

### Identity-declared write (config branch)

```
remargin --config ~/.remargin.yaml comment file.md "..."
```

### Read a range of a file

```
remargin get path=docs/design.md start_line=200 end_line=260 line_numbers=true
```

### Find pending stuff directed at me

```
remargin query path=. pending_for_me=true expanded=true
```

### Find broadcasts (no `to:`) the caller hasn't closed

```
remargin query path=. pending_broadcast=true
```

### Pretty-print all comments on a doc for the user

```
remargin comments file=src/discussions/roadmap.md pretty=true
```

(Then paste the output verbatim into your text response.)

---

## Tool reference

Each op exists at both surfaces: MCP `mcp__remargin__<op>`; CLI `remargin <op>`. Run `remargin <op> --help` for the exhaustive flag list.

### Document access

| Op | Purpose |
|----|---------|
| `ls` | List files and directories. |
| `get` | Read a file. `start_line`/`end_line`/`line_numbers`/`binary`. Run `metadata` before binary reads. |
| `write` | Write file content (comment-preserving). `create`, `raw`, `binary`, `start_line`/`end_line` for partial writes. |
| `metadata` | Frontmatter, comment counts, pending counts, mime, size. |
| `rm` | Remove a file. |

### Commenting

| Op | Purpose |
|----|---------|
| `comment` | Add one comment. `reply_to`, `after_line`, `after_comment`, `auto_ack`, `attachments`, `to`, `sandbox`. |
| `comments` | List comments in a file. `pretty=true` for human-readable threaded display. |
| `batch` | Add multiple comments atomically (single write, single verify). Each sub-op supports its own `auto_ack`, `reply_to`, etc. **Use this for N>1 comments on the same file.** |
| `edit` | Edit an existing comment. Cascades ack-clear to children. |
| `delete` | Delete one or more comments. Cleans up attachments. |
| `ack` | Acknowledge one or more comments. Omit `file` to resolve by ID across the directory tree. |
| `react` | Add or remove an emoji reaction. `remove=true` to unreact. |

### Sandbox

Sandbox staging is a per-identity, per-file marker stored in document frontmatter. Soft claim only — not "committed" or "submitted."

| Op | Purpose |
|----|---------|
| `sandbox_add` | Stage one or more markdown files. Idempotent. |
| `sandbox_remove` | Remove the caller's marker. Idempotent. |
| `sandbox_list` | List files staged for the caller's identity. |

### Identity

| Op | Purpose |
|----|---------|
| `identity_create` | Render a ready-to-use identity YAML block. Returns `{identity, type, key, yaml}`. Caller writes the YAML to disk; `rem-is4z` bans agents writing to `.remargin.yaml` directly. `mode:` is never emitted. |

### Search and quality

| Op | Purpose |
|----|---------|
| `activity` | "What's new since X" across managed `.md`. Per-file change records (comments, acks, sandbox-adds) sorted by ts. With `since` omitted, the per-file cutoff is the caller's last action — files where the caller has never acted return everything. Folds in comment edits (via `edited_at`) and sandbox refreshes. |
| `query` | Search across documents for comments. Filters: `pending` (broad — directed + broadcast), `pending_for` (directed to recipient), `pending_for_me` (directed to caller), `pending_broadcast` (unacked broadcasts), `author`, `since`, `comment_id`. Pending filters compose as a union. `expanded=true` includes comments inline. |
| `search` | Search across documents for text. `regex`, `scope` (all/body/comments), `context`, `ignore_case`. |
| `lint` | Structural lint checks. |
| `verify` | Check checksums and signatures against the registry. |
| `migrate` | Convert old-format inline comments to remargin format. |
| `purge` | Strip all comments (destructive — user-initiated only). |

### Plan (preview surface)

| Op | Purpose |
|----|---------|
| `plan` | Projection for any mutating op. Takes `op` + the same args as the underlying call. Returns predicted outcome without touching disk. Covers `ack`, `batch`, `comment`, `delete`, `edit`, `migrate`, `purge`, `react`, `sandbox-add`, `sandbox-remove`, `sign`, `write`. |

### Admin (CLI-only — user-facing setup)

- `keygen` — generate Ed25519 signing key pair.
- `mcp` — run the stdio MCP server (entry point for `mcp__remargin__*`).
- `obsidian` — install/uninstall the Obsidian vault plugin.
- `registry` — manage the participant registry file.
- `resolve-mode` — resolve the effective enforcement mode.
- `skill` — manage the Claude Code skill (this file).
- `identity` — print configured identity. `identity create --identity NAME --type human|agent [--key PATH]` prints YAML to stdout.
- `version` — print version info.

### Permissions

| Need | MCP tool | CLI |
|---|---|---|
| Restrict a path | _CLI-only (rem-888p)_ | `remargin restrict` |
| Unprotect a path | _CLI-only (rem-888p)_ | `remargin unprotect` |
| Show resolved permissions | `mcp__remargin__permissions_show` | `remargin permissions show` |
| Check if path is restricted | `mcp__remargin__permissions_check` | `remargin permissions check` |

`restrict` and `unprotect` are intentionally CLI-only: they mutate
permission policy and that decision belongs to the human, not to the
agent. The MCP surface deliberately omits them, and `mcp__remargin__plan`
also rejects `op="restrict"` and `op="unprotect"` for the same reason.
Never call `remargin unprotect` from a Bash subprocess to clear a
denial — surface the denial to the user and wait for explicit consent.

No identity flags on these commands — editing your own permissions doesn't need an identity declaration.

---

## Comment format

The tools produce this; you do not write it manually.

````markdown
```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:a1b2c3d4...
---
This is the comment content. Multi-paragraph markdown allowed.
```
````

Threaded reply with ack:

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

---

## Pretty display format (produced by `pretty=true`, do not write manually)

### Single comment

```
docs/design.md:25
  abc · eduardo (human) · 2026-04-06 14:32
  │ The comment content goes here.
  │ pending
```

### Threaded reply

```
docs/design.md:25
  abc · eduardo (human) · 2026-04-06 14:32
  │ Question.
  │ pending

  docs/design.md:35
    xyz · claude (agent) · 2026-04-06 14:33
    │ ⤷ reply-to: abc
    │ Answer.
    │ ✓ acked by eduardo @ 2026-04-06 15:00
```

### Footer

```
─────
3 comments · 2 pending
```

---

## Permissions setup

By default, Claude Code prompts on every `mcp__remargin__*` call. That
is the intended behavior under `restrict`: the user wants explicit
per-call oversight of remargin's MCP tools, since remargin may be the
only path reaching the restricted content.

If a user prefers silent forwarding (no prompts), it is **their**
opt-in choice — `remargin restrict` does not make this decision for
them. Suggest, but do not assume, that they add this block to
`.claude/settings.local.json`:

```json
{
  "permissions": {
    "allow": ["mcp__remargin__*"]
  }
}
```

Approves all remargin tools at once. The wildcard automatically covers
the read-only inspection tools (`mcp__remargin__permissions_show`,
`mcp__remargin__permissions_check`) — no edit needed when new commands
ship. (`restrict` / `unprotect` are CLI-only — rem-888p — and not
exposed via MCP.)

When `remargin restrict <path>` itself runs, it APPENDS deny rules
(plus any explicit `allow_dot_folders` re-allows) to the same
`permissions` block (see the "restrict / unprotect" decision flowchart
above for the full mechanism). The synchronizer is idempotent.
Crucially, `restrict` does **not** add `mcp__remargin__*` to the allow
list — if the user has it there, it is because they put it there
themselves, and `unprotect` will leave it alone.

---

## Strict mode

Three modes resolved by walking up for `.remargin.yaml`:

- `open` — anyone may post; no signatures required.
- `registered` — only identities in the registry may post; no signatures.
- `strict` — registered identities only, every comment carries a valid Ed25519 signature.

In strict mode, the verify gate runs before every write (`rem-ef1`); unsigned/unregistered posts are rejected.

---

## Sandbox ≠ commit

`sandbox_add` is a soft claim ("I'm working on this"). The file is not "committed" or "submitted" — that is an adapter-level concept. If a user says "stage this for review", `sandbox_add` is right. If they say "submit this", clarify first.

---

## Key concepts

- **Identity**: every comment has an author (string) and type (`human` or `agent`).
- **Threading**: `reply_to` (direct parent) and `thread` (root ancestor).
- **Acknowledgment**: `ack` records who and when (full timestamp).
- **Integrity**: every comment gets a checksum. Strict mode adds Ed25519 signatures.
- **Batch atomicity**: multiple ops in one `batch` produce a single write and a single verify pass.
- **Comment preservation**: tools guarantee no comments are lost during writes — the before/after comment list must match exactly.
- **Noop**: a write producing byte-identical content returns `noop: true` without touching the file.
- **Sandbox**: per-identity marker in frontmatter. Soft claim only.
- **Plan**: universal projection for any mutating op. Returns the predicted outcome without writing.
