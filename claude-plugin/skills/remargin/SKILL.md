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

## Slash commands (plugin-only)

When the remargin plugin is installed, these slash commands are available:

- `/remargin:process-file <path>` — process a single managed markdown file. Trigger phrases: "process this file", "submit this file", "process <path>", "run remargin on this".
- `/remargin:process-sandbox-group <prompt-name>` — process one sandbox group at a time. Trigger phrases: "process the <prompt-name> group", "submit the <prompt-name> group". Use when the user names a specific prompt group.
- `/remargin:process-sandbox` — process every sandboxed file in the vault, one subagent per resolved prompt group (context isolation per group). Trigger phrases: "process the sandbox", "process sandboxed items in this vault", "submit the sandbox", "run the sandbox", "process everything I staged".
- `/remargin:process-folder <path>` — process a folder driven by activity: first read the full activity delta across ALL identities to build awareness, then act only on items pending to this identity or open/unassigned. Groups by resolved system prompt and spawns one subagent per group. Does NOT touch sandbox markers. Trigger phrases: "process this folder", "process the folder <path>", "go through this folder", "what changed in this folder and handle my part".
- `/remargin:activity [path]` — report what changed since the caller last acted on each managed `.md` under `path`. Read-only. Trigger phrases: "what's new", "what happened since I was last here", "what changed in this workspace", "any activity I missed", "anything for me".
- `/remargin:consolidate <path>` — re-create a single managed markdown file so its body reflects **everything** in its comment threads (agreements, decisions, open issues, actionable items, memos). Distinct from `process-file`, which only replies to pending comments. Comments preserved by default; `--delete-comments` removes them after the rewrite. Authorized for humans; for agents only when the resolved system prompt permits it. Trigger phrases: "consolidate this", "consolidate the document", "consolidate <path>".

Routing rules:

- When the user names a single file, route to `/remargin:process-file`.
- When the user names a specific prompt group, route to `/remargin:process-sandbox-group`.
- When the user asks for the sandbox / staging area / "everything I staged" / similar without naming a group, route to `/remargin:process-sandbox` (vault-wide, subagents per group).
- When the user names a folder (a directory, not a single file and not the sandbox / staging area), route to `/remargin:process-folder` (activity-driven). Branch on the path itself, not its spelling: inspect whether the path is a directory rather than guessing from the name. If the path is ambiguous (file vs folder), resolve by inspecting the path.
- When the user asks what's new / what happened / what changed, route to `/remargin:activity`.
- When the user asks to **consolidate** a document (in chat, or a comment containing `/remargin:consolidate`), route to `/remargin:consolidate` — never reproduce its recreate logic inline.
- If the user gives no path and no sandbox cue, ask which they mean. Do not pick.
- Never bypass a slash command and reproduce its logic inline. The slash command is the canonical entry point; the skill is the router.

---

## If you hit a permission denial, read this first

If a native tool (`Read` / `Edit` / `Write` / `NotebookEdit` / `Bash`)
is denied on a `.md` file or any file under a `.remargin.yaml` realm:
**this is by design.** Switch to the equivalent `mcp__remargin__*` op
(`get`, `write`, `comment`, `delete`, `metadata`, `query`, ...) and
proceed. **Do NOT ask the user**, do NOT propose alternatives, do NOT
surface the denial as a blocker. The skill exists precisely so this
question doesn't need to be asked.

EXCEPTION — if the denial cites a `trusted_roots` or `deny_ops` rule,
the user has declared this path off-limits. Surface the denial.
**Always ask before `unrestrict`** — never call
`remargin claude unrestrict` (or any other path that would reverse a
user-declared restriction) on your own initiative. That is the worst
failure mode of the permissions system. Wait for explicit consent
before any action that would clear the obstacle.

### Compound-denial priority order

When a denial mentions multiple reasons, evaluate in this order and
stop at the first match:

1. **`trusted_roots` or `deny_ops` rule named** → surface the denial
   to the user. Never auto-recover. **Ask before `unrestrict`.**
2. **`path escapes sandbox`** → configuration issue (insufficient
   `trusted_roots` for the MCP). Surface to the user; do not retry.
3. **Otherwise (managed-file native-tool denial)** → switch to the
   equivalent `mcp__remargin__*` op and proceed silently.

### Symptom -> action map

| Symptom | Action |
|---|---|
| `Read` denied on a `.md` file | `mcp__remargin__get` |
| `Edit` / `Write` denied on a `.md` file | `mcp__remargin__write` (or `comment` if it's a thread reply) |
| `Bash(cat ...)` / `Bash(sed ...)` / `Bash(rm ...)` denied | `mcp__remargin__get` / `mcp__remargin__write` / `mcp__remargin__delete` |
| `NotebookEdit` denied on a `.md` file | `mcp__remargin__write` |
| Op denied with **`trusted_roots`** rule | surface the denial; **ask before `unrestrict`**; do not call `remargin claude unrestrict` |
| Op denied with **`deny_ops`** rule | surface the denial; user has explicitly disallowed this op on this path |
| `path escapes sandbox` from MCP | configuration issue (insufficient `trusted_roots`); surface to user; do not retry |

### Anti-patterns

- **Anti-pattern: stopping on a permission denial and asking the user
  "should I try a different tool?"** — the answer is always "yes, use
  remargin." The skill exists precisely so this question doesn't need
  to be asked. Switch to the MCP equivalent and proceed silently
  (see the recovery directive above).
- **Anti-pattern: do not call `remargin claude unrestrict` to clear a
  `trusted_roots` denial.** The user's `claude restrict` invocation
  that added it is user intent. Reversing it without explicit consent
  is the worst failure mode of the permissions system. **Ask before
  `unrestrict`.** This rule applies regardless of surface: MCP doesn't
  expose the tool, but the Bash subprocess path to the CLI is still
  reachable — and the prohibition stands there too. The fence is your
  behavior, not the surface.

---

## Critical rules (read first, scan often)

1. **Realm scope.** Every `.md` inside a realm is managed. NEVER use `Read` / `Edit` / `Write` / `Bash` (`cat`, `sed`, `awk`, `cp`, `mv`, `tee`, redirection) on a managed file. Always go through remargin tools.
2. **MCP > CLI.** If `mcp__remargin__*` tools are reachable, use MCP. The CLI is a shell-context fallback only.
3. **At least one reply per comment, threaded via `reply_to`.** Use `batch` for N replies in one turn. If a comment raises two distinct subjects, post two replies (each its own thread under the same parent) — that's cleaner than one reply that mixes them. What's NEVER ok is bundling answers to multiple **separate** comments into one consolidated reply.
4. **Acks AND comments both signal completion.** Don't post either before the work is done. No promissory comments like "rewriting now" / "I'll do X next" — the user can't tell from the comment whether the work happened. Ack is not a read-receipt either: pending comments are work items, not background context.
5. **Don't return pending comments to the user as their to-do** when the action is yours.
6. **Line numbers shift on every mutation.** Re-resolve immediately before any line-anchored op, or use `batch` for multi-step. Bottom-up ordering is not a substitute for `batch` — it's the same anti-pattern in disguise. If you find yourself ordering inserts bottom-up to dodge line shifts, you forgot `batch` exists. When responding to a comment, use `reply` (not `comment` with `reply_to`).
7. **`reply` is the preferred surface for thread responses.** Wraps `comment` with a required `parent_id` and surfaces the smart `auto_ack` default as headline behavior. The smart default acks the parent iff its author differs from yours — explicit `auto_ack: true|false` overrides. Do NOT auto-include the parent's author in `to:`; be explicit.
8. **`--config` XOR `(--identity + --type + --key)`.** Three branches: `--config FILE` alone, full triplet, or filter on the walked candidate. Mixing those two halves in one call is rejected at parse time — the CLI errors before the op runs.
9. **`auto_ack` defaults to a smart `Option<bool>`.** If you omit `auto_ack` on a reply (the common case), the parent is auto-acked iff its author differs from your identity — replies to your own comments don't ack. Set `auto_ack: true` to force the ack (legal only when (a) the parent is addressed to you via `to:` AND (b) your reply fully resolves the ask). Set `auto_ack: false` to force-skip even when replying to someone else. `auto_ack: true` without `reply_to` is rejected; `auto_ack: false` and the default (omitted) are no-ops without a `reply_to`.
10. **Never declare a different identity per call** unless the user explicitly asked. Per-call `identity` / `type` / `config_path` to declare someone else = impersonation.
11. **Never delete other participants' comments** to unblock your own op. Find another path or ask the user.
12. **Always run `remargin activity` (or `/remargin:activity`) BEFORE processing comments — pending comments are only one signal.** Activity surfaces the full delta since you last acted on each file: comments addressed to you, comments addressed to others, broadcast comments, new acks on threads you participate in, reactions added/removed, comment edits, signatures landed on previously-unsigned comments, sandbox-adds by other identities. Replying to your pending queue without checking activity means you miss context that may change what your reply should say — e.g. someone else already answered, a thread you're in just got new participants, an edit invalidated the assumption behind your draft. Do not hand-roll timestamps from `comments` / `query` for this purpose; those tools don't compute the per-file caller-last-action cutoff and don't fold edits / reactions / sandbox refreshes into a single change list.
13. **Use `pending=true` to find what you owe — `pending_for_me=true` is narrow and silently skips broadcasts.** `pending=true` is the canonical filter for "what work do I have on this file" — it returns the union of directed-pending and unacked broadcasts. `pending_for_me=true` returns only comments directed to your identity via `to:` plus replies whose parent author is you; it omits unacked broadcasts (`to: []`, no `reply_to`) that you may effectively own, and any pending owned by `<unassigned>`. Reaching for `pending_for_me=true` to answer "what do I need to act on" is a known footgun — broadcasts disappear from your queue and the user has to remind you. Default to `pending=true`; reach for the narrow filters only to disambiguate after the broad list.
14. **Each participant owns their own ack queue.** Don't ack on someone else's behalf, and never advise them to leave a comment unacked — their queue is their decision. You may deliberately leave your own reply unacked to keep it visible in your own pending queue.
15. **Comments are markdown — write them as markdown.** Every comment body (new comments, replies, edits) renders as markdown for human readers. Use markdown formatting where it helps readability:
    - Inline `` `code` `` for paths, identifiers, op names, commands.
    - Fenced code blocks for multi-line code, YAML, JSON, command output.
    - Bullet or numbered lists for enumerations.
    - **Bold** or *italic* for emphasis on a phrase, not whole paragraphs.
    - Markdown links (`[label](url)`) when pointing at external references.

    Don't over-decorate plain prose — a one-line answer stays a one-line answer. The bar is readability for a human scanning a thread, not styling for its own sake.
16. **Prefer partial writes over rewriting the whole file.** `write` accepts `start_line` / `end_line` (1-indexed, inclusive) to replace just a line range while leaving the rest of the file untouched. Use this whenever you're changing a few lines, fixing a section, or updating one paragraph in a large doc. Rewriting the whole file forces you to carry the entire body in your context (slow, expensive, and one typo can corrupt the rest). Comment preservation, frontmatter handling, and the verify gate all run identically on partial writes. Reserve whole-file `write` for new files (`create=true`) or genuine wholesale rewrites. **To re-create most or all of a commented document, you cannot whole-file `write` it (the payload would have to reproduce every comment block byte-for-byte) — follow [rewriting-whole-files.md](rewriting-whole-files.md).**
17. **Comments must be self-contained.** Write every comment so it stands on its own to a human scanning the thread later. Spell names and terms out in full — no acronyms or invented shorthand (write "Module 1", not "M1"; write the person's full name, not an initial). Never refer to another comment by its ID (e.g. "see 3pd", "as in ow6") — IDs are opaque to a reader and meaningless out of context. Instead quote or paraphrase what that comment said, and point at the relevant file or section if needed. A reader should never have to expand an acronym or go look up a comment ID to understand what you wrote.
18. **Surface open questions as comments, not prose.** When you author or edit a managed document and hit a decision you can't make, a question only the owner can answer, or a tradeoff that needs a human call, post it as a `comment` / `reply` addressed to the owner via `to:` so it lands in their pending queue — never leave it as an "open question" / "TBD" / "for discussion" paragraph in the body. The body holds decided content; unresolved items live in the thread, where the owner is actually notified. One comment per separable question (`batch` for several) — and keep each one scoped to its single ask: a brief rationale is fine, but don't fold a status / progress / blocker report into a decision comment. A reader scanning the body should see decisions, not your deliberation.
19. **Sign only what you own; never sign to make `verify` pass.** Your signature vouches that *you authored* the content — sign your own comments and nothing else (the forgery guard enforces it, but the discipline is yours). A failed `verify` (`signature_invalid` or checksum mismatch) is a diagnostic signal, not something to silence: it means a wrong signing key, the wrong identity, edited/tampered content, or an unregistered key. Fix the root cause — never reach for `sign` / `repair_checksum` to paper over a failed verify, and never re-sign another author's content.

---

## Decision flowcharts

Each section starts with the question an agent is actually asking.

### Q: I have N pending comments to reply to. What do I do?

This is the most common multi-comment workflow. Use `batch`. **Do not** bundle into one comment, **do not** post N sequential `comment` calls.

**Before the steps below: run activity first.** `remargin activity --pretty <folder>` or `/remargin:activity <folder>`. Read the full timeline before opening the pending queue — reactions, acks on threads you're in, comments addressed to others, edits, and signatures since your last visit all live there and may change what your reply should say. Pending-for-me is only one slice of the picture; activity is the full delta. See Critical rule 12.

1. List the pending ones: `remargin query --pending --pretty <folder>` (CLI) or `mcp__remargin__query` with `pending: true`.
2. For each comment, complete the action it asks (file the bd task, update the doc, run the verification, etc.). Adjust your reply for anything activity surfaced — e.g. don't repeat an answer someone else already posted on the same thread.
3. Reply to all N via ONE `batch` call:

   ```
   remargin batch --ops '[
     {"content": "answer to A...", "reply_to": "abc"},
     {"content": "answer to B...", "reply_to": "def", "auto_ack": true},
     {"content": "answer to C...", "reply_to": "ghi"}
   ]' file.md
   ```

   Or via `mcp__remargin__batch` with the same shape (no JSON-string encoding).

4. `auto_ack` defaults to the smart shape per Critical rule 9: omit it on most replies; set `true` only when (a) the parent is addressed to you via `to:` AND (b) your reply fully resolves the ask; set `false` to force-skip the ack.
5. For broadcasts (`to: []`) or comments addressed to others, the smart default still applies (parent.author != caller → ack). Override with explicit `auto_ack: false` if the broadcast nature means an ack would be premature; ack separately via `remargin ack` if appropriate.

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
| Find/replace across body (file or folder) | `replace pattern=... replacement=... path=... [regex=true] [ignore_case=true]` (body-only; never touches comments; `path` required) |
| Replace a line range (preferred for edits) | `write path=... start_line=N end_line=M content=...` |
| Replace whole file (rare — usually wrong for edits) | `write path=... content=...` (comment-preserving) |
| Create a new file | `write path=... content=... create=true` |
| Write non-markdown | `write path=... content=... raw=true` |
| Copy a file (markdown: body-only, no comments in copy) | `cp src=... dst=...` |
| Move/rename a file | `mv src=... dst=...` |
| Delete a file | `rm path=...` |

**Do not** use `Read` / `Edit` / `Write` / `Bash` shell tools on managed `.md` files. The realm rule has no exceptions.

### Q: The doc references an image. Should I view it?

Yes — always view it before acting on the surrounding text. Markdown is often sparse because the visual is the spec (showing a bug, layout, before/after state, etc.). Skipping the image produces vague or wrong conclusions.

Applies to every syntax:

- Obsidian wikilinks `[[diagram.png]]`
- Markdown image syntax `![alt](path/to/image.png)`
- HTML `<img src="...">`
- Relative or absolute paths inside any of the above

Use `get path=... binary=true` for the image. Run `metadata` first if you need to check size.

### Q: How do I declare identity for a mutating call?

Three exclusive branches — pick exactly one:

| Branch | Pattern | When |
|---|---|---|
| **Config alone** | `--config FILE` (CLI) / `config_path: "FILE"` (MCP) | The file declares a complete identity. Mutually exclusive with the other three — mixing is rejected before the op runs. |
| **Full triplet** | `--identity NAME --type human|agent --key PATH` | Declaring a complete identity inline. |
| **Filter (or none)** | Subset of triplet, or no flags | Args narrow the walked candidate set. Zero or many matches = error. |

**Default**: don't declare anything. The walked `.remargin.yaml` resolves your identity. Per-call declaration of someone else's identity is impersonation.

### Q: Whose identity do I put in `to:` so the right person sees it?

The human user's remargin identity is most likely defined at `~/.remargin.yaml` (`type: human`) and is the same across every repo/realm on this machine; use that identity in the `to:` field when a comment needs the human to see it in their pending queue. The agent identity is realm-specific — get the active one from `whoami`.

### Q: A user asked to "show comments" / "what's pending" — what do I return?

Run the CLI with `--pretty` on `comments` or `query`, then **paste the full output verbatim into your text response.** The pretty threaded display is CLI-only — the MCP `comments`/`query` tools return JSON. MCP results are not visible to the user; calling the tool alone is not enough. Do not paraphrase or summarize.

```
remargin comments <file> --pretty
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

### Q: I want to restrict (or unrestrict) a path.

1. `remargin claude restrict <path>` — appends an entry to
   `<.claude-anchor>/.remargin.yaml` AND syncs the equivalent rules
   into `.claude/settings.local.json` + `~/.claude/settings.json`.
   Layer 1 (remargin-core) starts refusing ops on the path on the
   very next call. Layer 2 (Claude's NATIVE Read/Edit/Write/Bash
   tools) takes effect when Claude reloads its settings (typically
   a Claude restart — outside remargin's control).
2. `remargin claude unrestrict <path>` — exact reverse. Uses a
   sidecar (`<.claude-anchor>/.claude/.remargin-restrictions.json`)
   to know precisely which rules to remove; never touches user-added
   rules.
3. `remargin permissions show` — print the resolved permissions
   tree at cwd. JSON via `--json`.
4. `remargin permissions check <path> [--why]` — gitignore-style:
   exit 0 when restricted, 1 when not.

Wildcard form: `remargin claude restrict "*"` and
`remargin claude unrestrict "*"` cover the entire realm anchored at
the matching `.remargin.yaml`.

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

❌ **N sequential `comment` calls on the same file** — including bottom-up ordering as a workaround. Line numbers shift between calls; the second/third lands wrong. Use `batch`. If you find yourself ordering inserts bottom-up to dodge line shifts, that's the same anti-pattern in disguise — you forgot the `batch` tool exists.

❌ **Acking before doing the work.** Ack signals "done." Doing it in the wrong order makes the ack a lie.

❌ **Listing pending comments to the user as their to-do** when the action is yours. Pendings are work items, not background.

❌ **Using CLI when MCP is reachable.** Shell-escape hazards on `$`, backticks, `---`. Loses type-safety. More permission prompts.

❌ **`Read` / `Edit` / `Write` / `Bash` on a managed `.md`.** No per-file opt-out. Realm scope is total.

❌ **`auto_ack: true` on a comment addressed to someone else.** Speaks on their behalf.

❌ **Trusting line numbers across two mutations.** Comment IDs are stable; line numbers aren't.

❌ **Per-call identity declaration without explicit user instruction.** Impersonation.

❌ **Replying with a summary of the original comment** instead of doing the work. Reply demonstrates substance, not paraphrase.

❌ **Expanding a reply's scope beyond what the parent comment asked.** If you spot an adjacent issue while answering, note it as a one-line dependency at most — don't restructure the reply around it. The user asked about X; answer X.

❌ **Cross-referencing internal IDs in user-facing replies OR in document bodies** ("see Decision 13", "as in `xyz`", "per the `abc` thread", "(per `25w`)"). Agents track IDs; users read linearly. The doc body is even worse than replies for this — comments get cleaned up after a discussion, leaving doc-body citations as dangling references that no one can resolve. Restate the relevant content inline. Both replies and doc bodies must stand on their own without knowledge of the comment thread that produced them.

❌ **Replying "Acked." or "Noted." to an agreement.** Adds zero info, creates a pending the other person has to clear. Just ack and move on.

❌ **`write` that drops comments to unblock yourself.** If preservation fails, re-read with `get`, rebuild correct content, retry. Never delete others' comments.

❌ **Rewriting a file to "fix" a verify mismatch.** That's the symptom, not the cause. Surface to the user.

❌ **Rewriting the whole file when you only need to change a few lines.** `write` accepts `start_line` + `end_line` for partial writes that leave the rest of the file untouched. Use the partial form; reserve whole-file writes for new files (`create=true`) or genuine wholesale rewrites.

❌ **Whole-file `write` to re-create a commented document.** The payload would have to reproduce every comment block byte-for-byte (checksums/signatures included), which you must not do — the preservation gate rejects the write. Rewrite the prose around the pinned comment blocks instead: see [rewriting-whole-files.md](rewriting-whole-files.md).

❌ **Minting or moving signing keys to "fix" a signing failure.** A missing key, `signature_invalid`, or unregistered identity is an admin/setup gap — not yours to solve by generating a new key (it breaks the identity→pubkey binding in the registry, so every signature then fails) or copying keys into folders. Surface to the user.

❌ **Trying to build code inside a realm.** A restricted realm blocks shell writes to *every* file under it, not just `.md` — you can't `cargo new` / `npm init` / scaffold / compile there, because the build writes non-markdown files the hook refuses. Realms are for *using* remargin, not building software. If a task needs to build code, it belongs outside the realm — say so and stop.

---

## Working with git

The Claude Code hook that enforces remargin access blocks any Bash command whose argument string contains the vault path (e.g. `/home/eduardoburgos/src/tixena/eburgos_notes`). This means these forms are **always blocked**, no matter how you spell them:

```bash
git -C /path/to/vault status           # blocked — path in argument
git --git-dir=/path/to/vault/.git log  # blocked — path in argument
git --work-tree=/path/to/vault status  # blocked — path in argument
```

Plain git commands with **no path argument** are **not blocked** and work normally from within the vault's shell context:

```bash
git status      # works
git log         # works
git push        # works
git add <file>  # works
git commit      # works
```

**How to apply:** always run git commands without explicit path flags. Never try `-C`, `--git-dir`, or `--work-tree` pointing at the vault — they will fail with a hook denial regardless of the specific git subcommand or flag combination. There is no workaround via path rewriting; the block is on the argument string, not the operation type.

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
- Status: `open`
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

### Find everything I owe a response on (canonical)

```
remargin query path=. pending=true expanded=true
```

Returns the union of directed-pending and unacked broadcasts. **This is the default for "what work do I have on this file."** See Critical rule 13.

### Find ONLY pending directed at me (narrow — skips broadcasts)

```
remargin query path=. pending_for_me=true expanded=true
```

Returns only comments with your identity in `to:` plus replies whose parent is yours. Use this to disambiguate *after* the broad list — never as the starting query.

### Find ONLY broadcasts (no `to:`) the caller hasn't acked

```
remargin query path=. pending_broadcast=true
```

### Pretty-print all comments on a doc for the user

```
remargin comments src/discussions/roadmap.md --pretty
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
| `comment` | Add one top-level comment. `after_line`, `after_comment`, `attachments`, `to`, `sandbox`. For thread replies use `reply`. |
| `reply` | **PREFERRED** for thread responses. `parent_id` (required), `content`, `auto_ack` (smart default: ack iff parent.author != caller), `to`, `attachments`, `sandbox`, `remargin_kind`. |
| `comments` | List comments in a file. MCP returns JSON; CLI `--pretty` gives the human-readable threaded display. |
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
| `identity_create` | Render a ready-to-use identity YAML block. Returns `{identity, type, key, yaml}`. Caller writes the YAML to disk; agents are banned from writing to `.remargin.yaml` directly. `mode:` is never emitted. |

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
| `plan` | Projection for any mutating op. Takes `op` + the same args as the underlying call. Returns predicted outcome without touching disk. Covers `ack`, `batch`, `comment`, `reply`, `delete`, `edit`, `migrate`, `purge`, `react`, `sandbox-add`, `sandbox-remove`, `sign`, `write`. `op: "reply"` is a synonym for `op: "comment"` with a required `parent_id`. |

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
| Restrict a path | _CLI-only_ | `remargin claude restrict` |
| Unrestrict a path | _CLI-only_ | `remargin claude unrestrict` |
| Show resolved permissions | `mcp__remargin__permissions_show` | `remargin permissions show` |
| Check if path is restricted | `mcp__remargin__permissions_check` | `remargin permissions check` |

`claude restrict` and `claude unrestrict` are intentionally CLI-only:
they mutate permission policy and that decision belongs to the human,
not to the agent. The MCP surface deliberately omits them, and
`mcp__remargin__plan` also rejects `op="claude_restrict"` and
`op="claude_unrestrict"` for the same reason. Never call
`remargin claude unrestrict` from a Bash subprocess to clear a denial
— surface the denial to the user and wait for explicit consent.

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

## Pretty display format (produced by CLI `--pretty`, do not write manually)

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
is the intended behavior under `trusted_roots`: the user wants
explicit per-call oversight of remargin's MCP tools, since remargin
may be the only path reaching the restricted content.

If a user prefers silent forwarding (no prompts), it is **their**
opt-in choice — `remargin claude restrict` does not make this decision
for them. Suggest, but do not assume, that they add this block to
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
ship. (`claude restrict` / `claude unrestrict` are CLI-only and not
exposed via MCP.)

When `remargin claude restrict <path>` itself runs, it APPENDS deny
rules (plus any explicit `allow_dot_folders` re-allows) to the same
`permissions` block (see the "restrict / unrestrict" decision flowchart
above for the full mechanism). The synchronizer is idempotent.
Crucially, `claude restrict` does **not** add `mcp__remargin__*` to the
allow list — if the user has it there, it is because they put it there
themselves, and `claude unrestrict` will leave it alone.

---

## Strict mode

Three modes resolved by walking up for `.remargin.yaml`:

- `open` — anyone may post; no signatures required.
- `registered` — only identities in the registry may post; no signatures.
- `strict` — registered identities only, every comment carries a valid Ed25519 signature.

In strict mode, the verify gate runs before every write; unsigned/unregistered posts are rejected.

**Signing keys and the registry are admin setup — not self-serve.** Signing uses your identity's private key (`key:` in `.remargin.yaml`); verification checks your signature against your registered public key in the participant registry `.remargin-registry.yaml` (resolved by walking up the tree). If you cannot sign — missing key file, `signature_invalid`, or your identity isn't registered — STOP and surface it to the user. Do **not** generate a new key for an already-registered identity (it breaks the identity→pubkey binding and every signature then fails verification), do **not** guess where keys or trust live, and do **not** write keys into arbitrary folders. Provisioning keys and editing the registry are the human's job.

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
