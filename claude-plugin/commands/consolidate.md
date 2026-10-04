---
description: Consolidate a single managed markdown file — read every comment thread in its full context and re-create the document so it reflects all agreements, decisions, open issues, actionable items, and memos. Authorized for human requesters; for agent requesters only when the resolved folder system prompt explicitly permits consolidation.
---

# /remargin:consolidate <path> [--delete-comments | --preserve-comments]

Re-create a managed markdown file so its body reflects **everything** in its comment threads. Unlike `/remargin:process-file` (which replies to pending comments), consolidate rewrites the document itself. It is a composite: it runs [`/remargin:consolidate-thread`](consolidate-thread.md) once per thread, oldest first, then makes one consistency pass over the whole body. Comments are **preserved by default**; pass `--delete-comments` to remove them after the rewrite.

## Steps

1. **Identify the requester and entity type.** If you were invoked directly by the user in chat, the requester is that **human** — authorized. If you were invoked because a comment contains `/remargin:consolidate` (e.g. routed from `/remargin:process-file`; a comment containing `/remargin:consolidate-thread` goes to that command instead), the **author of that comment** is the requester: read its `author` and `type` (`human` | `agent`) from the comment header via `mcp__remargin__comments` (`file` = the path). The entity type comes from the comment's `type` field — never guess it.

2. **Authorization gate.**
   - **Human requester** → proceed to step 3.
   - **Agent requester** → call `mcp__remargin__prompt_resolve` on `<path>`. The resolved folder-scoped system prompt body must **explicitly authorize consolidation** for this realm. If it does → proceed. If it does not — the prompt is silent on consolidation, or the resolver returned the locked Default (`is_default: true`) — **decline**: post one short `mcp__remargin__reply` on the requesting thread stating that consolidation is not authorized for this agent under the current system prompt, then **STOP**. Make no changes to the document.
   - This one check covers every per-thread run in step 6; they do not run their own gate.

3. **Resolve the comment parameter.** `--delete-comments` → delete mode. `--preserve-comments`, or no flag → **preserve mode (default)**. The flag is passed through unchanged to every per-thread run in step 6.

4. **Read the full thread context.** Call `mcp__remargin__activity` on the file, then `mcp__remargin__comments` (`file` = the path — **not** `mcp__remargin__query`, which honors `.gitignore` and silently returns nothing on gitignored files), then `mcp__remargin__get` for the body. `comments` returns rows in pages: keep calling with `offset` increased by the rows received until it reaches `total`, and read each row's cells by the column names in `comment_cols`. Read **every** thread end-to-end across **all** identities and targets: top-level comments, replies, acks, reactions, who each was addressed `to`, and the thread structure. Consolidation operates on the whole conversation, not the pending-for-me slice.

5. **Classify every thread** as settled (agreement or decision), open (open discussion, open issue or actionable item), or discardable, for the receipt. In delete mode, preview the deletion of every comment in the file once with `mcp__remargin__plan` (`op: delete`, `ids` = every comment id); if it returns a `reject_reason`, **STOP** with that reason as a chat blocker and change nothing.

6. **Consolidate each thread, oldest first.** Sort the threads by their root comment's `ts`, oldest first, so a later thread's outcome wins over an earlier one on the same point. For each thread, run steps 4–9 of [`/remargin:consolidate-thread`](consolidate-thread.md) with that thread's root id and this run's flag: resolve the thread, resolve its section, read, rewrite the section's prose, then keep or delete the thread's comments. Skip its authorization gate (step 2 above covers the whole run), its delete-mode preflight (step 5 above covers it) and its receipt. Each run starts from a fresh `get` and `comments`, because earlier runs shift line numbers.

7. **Consistency pass over the whole body.** The per-thread runs each see one section; this pass keeps the whole-document promise they cannot:
   - Remove a section that duplicates another.
   - Where two sections state different outcomes for the same point, keep the later thread's outcome and rewrite the other to match it, or drop it.
   - Remove text made stale by what the threads settled.
   - Move content that a thread's run placed under its anchor section into the section it is about, when those differ (for example a comment appended at the end of the file about an earlier section).
   - Never re-open a settled point, and never re-pose one as a question.

   Rewrite prose only. In **preserve** mode you **cannot** whole-file `write` a commented document: comment blocks are pinned at their lines, so rewrite the **prose around** them — partial-line `mcp__remargin__write` (bottom-up, last gap first; never include a comment block's lines) plus `mcp__remargin__replace` for substitutions, following [the skill's rewriting-whole-files.md](../skills/remargin/rewriting-whole-files.md) exactly. Consolidation does not move or ack comment blocks. In **delete** mode no comment blocks remain after step 6, so a whole-file `write` is fine.

8. **Return a receipt, not a summary.** The chat message proves the round ran and shows the queue state. Nothing else.

   ```
   notes/generated_types.md — consolidated (preserve · human requester)

   | found   | 14 threads · 9 settled · 3 open · 2 discardable |
   | actions | sections rewritten 6 · comments preserved 14 |
   | after   | 8 inbound pending · 6 awaiting your ack |
   ```

   Counts only — no thread text, no ids, and no per-thread classification list. Drop any row whose counts are all zero. In delete mode the `actions` row reads `comments deleted 14` instead.

   **Never restate, quote, summarize, or paraphrase comment or reply content in chat** — no per-thread tables, no "where I disagreed with you", no decision recaps, no list of body changes. The consolidated document *is* the report; the receipt only proves it was rewritten.

   Two things stay in chat, because they are *not* in the document: **blockers** — a declined authorization (step 2), or a thread you could not reconcile — and anything that **needs the owner's decision**. One line each, with why. Nothing else.

## Constraints

- Follow the remargin skill rules: MCP > CLI, comment-safe writes, no per-call identity overrides.
- "Take **all** the comments into account" is the contract: nothing agreed may be re-opened or contradicted, and nothing substantive may be dropped unless it was classified discardable.
- How one thread becomes body text is defined once, in `/remargin:consolidate-thread`; this command composes it and adds only the ordering and the consistency pass.
- Invariants: no settled decision re-posed as an open question; no duplicate or stale sections; the recreated document is internally consistent.
- The requester's entity type comes from the requesting comment's `type` field (sender info). Only delete comments when `--delete-comments` was explicitly passed.
