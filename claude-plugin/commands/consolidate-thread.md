---
description: Consolidate one comment thread of a managed markdown file into the section it is anchored under — rewrite only that section's prose so it states what the thread settled, then keep or delete only that thread's comments. Authorized for human requesters; for agent requesters only when the resolved folder system prompt explicitly permits consolidating a single thread.
---

# /remargin:consolidate-thread <path> <comment-id> [--delete-comments | --preserve-comments]

Fold one thread into the document body. `<comment-id>` may be the thread's root or any reply in it; the whole thread is consolidated. Only the section the thread's root is anchored under changes — every other section, and every comment of every other thread, stays byte-identical. Comments are **preserved by default**; pass `--delete-comments` to remove the thread's comments after the rewrite. `/remargin:consolidate` runs this same operation once per thread, so the two commands share one definition of how a thread becomes body text.

## Steps

1. **Identify the requester and entity type.**
   - Invoked directly by the user in chat → the requester is that **human**.
   - Invoked because a comment contains `/remargin:consolidate-thread` (e.g. routed from `/remargin:process-file`) → the requester is the **author of that comment**; read its `author` and `type` (`human` | `agent`) via `mcp__remargin__comments` (`file` = the path).
   - Invoked by you as a step of another skill or command you are running (for example a decision board settling one item) → the requester is **your own identity**; read its type from `mcp__remargin__whoami`.

   The entity type comes from the comment's `type` field or from `whoami` — never guess it.

2. **Authorization gate.**
   - **Human requester** → proceed.
   - **Agent requester** → call `mcp__remargin__prompt_resolve` on `<path>`. The resolved folder-scoped system prompt must **explicitly permit consolidating a single thread**. Permission for whole-document consolidation (`/remargin:consolidate`) alone does **not** count, and neither does the locked Default (`is_default: true`). If it is not permitted → **decline**: post one short `mcp__remargin__reply` on the requesting comment stating that thread consolidation is not authorized for this agent under the current system prompt, then **STOP** with no other change. When there is no requesting comment (the third case in step 1), make no change at all and report the decline as a chat blocker instead.

3. **Resolve the comment parameter.** `--delete-comments` → delete mode. `--preserve-comments`, or no flag → **preserve mode (default)**.

4. **Resolve the thread.** Call `mcp__remargin__comments` with `file` = the path — **not** `mcp__remargin__query`, which honors `.gitignore` and silently returns nothing on gitignored files. `comments` returns rows in pages: keep calling with `offset` increased by the rows received until it reaches `total`, and read each row's cells by the column names in `comment_cols`.
   - Find `<comment-id>`. If it is not in the file → **STOP** with an error naming the id; change nothing.
   - The thread's **root** is that comment's `thread` value, or the comment itself when it has no `thread`.
   - The **thread** is the root plus every comment whose `thread` equals the root's id.

5. **Resolve the section.** Call `mcp__remargin__get` with `line_numbers: true`.
   - The section heading is the nearest ATX heading (`#` … `######`) above the root comment's `line`. Lines inside fenced code blocks and inside `` ```remargin `` comment blocks are never headings.
   - The section runs from that heading to the line before the next heading of the same or a higher level (fewer or equal `#`), or to the end of the file.
   - With no heading above the root, the section is the body text before the first heading. The YAML frontmatter is never part of any section.
   - Record the line range of every comment block inside the section — this thread's and any other thread's. They are pinned and are never part of a write range.

6. **Read.** Call `mcp__remargin__activity` on the file, then read the whole thread end-to-end: every reply, ack, reaction, recipient (`to`) and `kind`, in thread order, together with the section's current prose.

7. **Delete-mode preflight.** In delete mode, before writing anything, preview the deletion with `mcp__remargin__plan` (`op: delete`, `file` = the path, `ids` = every id in the thread). If it returns a `reject_reason` → **STOP** with that reason as a chat blocker; change nothing.

8. **Rewrite the section's prose.** Follow [the skill's rewriting-whole-files.md](../skills/remargin/rewriting-whole-files.md): partial-line `mcp__remargin__write` with `start_line`/`end_line`, **bottom-up** (last gap first), never including a comment block's lines; `mcp__remargin__replace` only for substitutions that cannot reach outside the section.
   - State the thread's outcome as settled content: decisions as decisions, agreed changes as done, and the artifact (commit, file, value, link) when there is one. Never re-pose a settled point as a question.
   - A position argued in the thread but never taken up is dropped, not stated as fact. An open issue or actionable item the thread leaves open is recorded in the section **as open**, so deleting the comments loses nothing.
   - Useful rationale and memos from the thread are folded in; discardable remarks are dropped.
   - Keep the heading line unless the thread settled a new title for it.
   - Change nothing outside the section: no other section's prose, no comment block of any thread, no frontmatter.

9. **Apply the comment parameter.**
   - **Preserve (default):** leave every comment of the thread where it is, unacked.
   - **Delete:** call `mcp__remargin__delete` with exactly the thread's ids — every comment in the thread, whoever wrote it, and no other comment.

10. **Return a receipt, not a summary.** The chat message proves the run happened and shows the queue state. Nothing else.

    ```
    notes/decision_boards/mcp-organization-config-issues.md — thread consolidated (delete · agent requester, folder prompt permits)

    | thread  | 2 comments · section "8 of 11 — Updating a document type resets its warning days to 30" |
    | actions | section rewritten 1 · comments deleted 2 |
    | after   | 0 inbound pending · 12 awaiting your ack |
    ```

    Counts and the section heading only — no thread text, no ids. In preserve mode the `actions` row reads `comments preserved <N>`.

    **Never restate, quote, summarize, or paraphrase comment or reply content in chat.** The rewritten section *is* the report.

    Two things stay in chat, because they are *not* in the document: **blockers** — a declined authorization, an unknown id, a refused deletion — and anything that **needs the owner's decision**. One line each, with why. Nothing else.

## Constraints

- Follow the remargin skill rules: MCP > CLI, comment-safe writes, no per-call identity overrides.
- Scope is the section: every line outside it, and every comment block anywhere (including other threads in the same section), stays byte-identical. Preserve mode acks nothing.
- Nothing the thread settled may be re-opened or contradicted, and nothing left open may be silently dropped.
- Only delete comments when `--delete-comments` was explicitly passed, and then only the thread's.
