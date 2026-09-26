---
description: Process a single managed markdown file under its resolved system prompt. Reads pending comments, replies, acks, and edits the doc body as the prompt directs. Does NOT clear the file's sandbox marker (sandbox cleanup is the per-group command's job).
---

# /remargin:process-file <path>

Given a file path, process the file under its resolved system prompt.

## Steps

1. **Check activity first.** Call `mcp__remargin__activity` with `path` = the file. Read the full delta since your last action on this file — reactions, acks on threads you're in, comments addressed to others, edits, signatures landed since you last looked. Pending-for-me is only one slice of the picture; everything else lives in activity and may change what your reply should say. See remargin skill Critical rule 6.

2. **Resolve the system prompt.** Call `mcp__remargin__prompt_resolve` with the given path. The result contains the prompt name and body. The resolver falls back to a locked Default body when the `.remargin.yaml` walk exhausts.

3. **Frame the work.** Read the prompt body and treat it as your current task definition. Everything below operates under that prompt.

4. **Process the file.** Read the file via `mcp__remargin__get`. Surface pending comments via `mcp__remargin__comments` with `file` = the path — this reads the named file directly. **Do not use `mcp__remargin__query` to find pending comments here:** `query` walks the directory tree and honors `.gitignore`, so on a file in a gitignored folder it returns nothing and the command silently no-ops on a file full of pending comments. `comments` reads the file regardless of gitignore status. Reply to each via `mcp__remargin__reply` for single responses, or `mcp__remargin__batch` for multiple in one atomic write (each sub-op carries its own `reply_to`, `auto_ack` where appropriate per the remargin skill rules). Edit the doc body via `mcp__remargin__write` partial-line writes where the prompt calls for it. When drafting replies, take activity into account — don't repeat an answer someone else already posted on the same thread, and adjust for any edits that have invalidated your draft.

> **Consolidation requests route out.** Check thread consolidation first, because `/remargin:consolidate-thread` also contains the text `/remargin:consolidate`:
>
> - If a pending comment asks to consolidate **one thread** (it contains `/remargin:consolidate-thread`, or asks in words to consolidate a specific thread), do **not** reply or edit inline here — route to `/remargin:consolidate-thread <path> <comment-id>` with the id and flag the comment names. It rewrites only the section that thread is anchored under.
> - Otherwise, if a pending comment asks to **consolidate** the document (it contains `/remargin:consolidate`, or asks in words to consolidate / re-create the document from all comments), do **not** reply or edit inline here — route to `/remargin:consolidate <path>`. Consolidation re-creates the whole document body from every thread and is a different operation from replying to pending comments.

5. **Do NOT remove the sandbox marker.** Sandbox cleanup is the responsibility of the per-group command, not this one. Manual per-file invocation is non-destructive on sandbox state.

6. **Verify no inbound pendings remain.** Call `mcp__remargin__comments` with `file` = the path (again, not `query` — same gitignore blindness). Inspect every comment still shown as pending: pending replies you just posted (where `author` == your identity from `mcp__remargin__whoami`) are expected and OK — they're awaiting the other party's ack. Any **inbound** pending (a comment whose `author` is someone else) means you skipped its reply. Go back to step 4 and address it. **Do not move to step 7 with inbound pendings outstanding.**

7. **Return a receipt, not a summary.** The chat message proves the round ran and shows the queue state. Nothing else.

   ```
   notes/generated_types.md — processed

   | found   | 10 to me · 2 broadcast · 5 to others |
   | actions | replied 12 · acked 3 · body edits 2 · issues filed 1 |
   | after   | 0 inbound pending · 6 awaiting your ack |
   ```

   Counts only — no comment text, no ids. Drop any row whose counts are all zero. The `after` row carries step 6's "0 inbound pending" confirmation.

   **Never restate, quote, summarize, or paraphrase comment or reply content in chat** — no per-comment tables, no "where I disagreed with you", no decision recaps, no list of document changes. If it was worth saying, it is already in the document.

   Two things stay in chat, because they are *not* in the document: **blockers** (an op that was denied, or a comment you could not act on) and anything that **needs the owner's decision**. One line each, with why. Nothing else.

## Constraints

- Follow the remargin skill rules: use MCP > CLI, batch for N replies, ack only after the work is done, no per-call identity overrides, never delete other participants' comments.
- The resolved system prompt is the source of truth for what "process" means in this realm. If it conflicts with anything else in your context, the prompt wins.
