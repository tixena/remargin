---
description: Process a single managed markdown file under its resolved system prompt. Reads pending comments, replies, acks, and edits the doc body as the prompt directs. Does NOT clear the file's sandbox marker (sandbox cleanup is the per-group command's job).
---

# /remargin:process-file <path>

Given a file path, process the file under its resolved system prompt.

## Steps

1. **Check activity first.** Call `mcp__remargin__activity` with `path` = the file. Read the full delta since your last action on this file — reactions, acks on threads you're in, comments addressed to others, edits, signatures landed since you last looked. Pending-for-me is only one slice of the picture; everything else lives in activity and may change what your reply should say. See remargin skill Critical rule 12.

2. **Resolve the system prompt.** Call `mcp__remargin__prompt_resolve` with the given path. The result contains the prompt name and body. The resolver falls back to a locked Default body when the `.remargin.yaml` walk exhausts.

3. **Frame the work.** Read the prompt body and treat it as your current task definition. Everything below operates under that prompt.

4. **Process the file.** Read the file via `mcp__remargin__get`. Surface pending comments via `mcp__remargin__comments` with `file` = the path — this reads the named file directly. **Do not use `mcp__remargin__query` to find pending comments here:** `query` walks the directory tree and honors `.gitignore`, so on a file in a gitignored folder it returns nothing and the command silently no-ops on a file full of pending comments. `comments` reads the file regardless of gitignore status. Reply to each via `mcp__remargin__reply` for single responses, or `mcp__remargin__batch` for multiple in one atomic write (each sub-op carries its own `reply_to`, `auto_ack` where appropriate per the remargin skill rules). Edit the doc body via `mcp__remargin__write` partial-line writes where the prompt calls for it. When drafting replies, take activity into account — don't repeat an answer someone else already posted on the same thread, and adjust for any edits that have invalidated your draft.

5. **Do NOT remove the sandbox marker.** Sandbox cleanup is the responsibility of the per-group command, not this one. Manual per-file invocation is non-destructive on sandbox state.

6. **Verify no inbound pendings remain.** Call `mcp__remargin__comments` with `file` = the path (again, not `query` — same gitignore blindness). Inspect every comment still shown as pending: pending replies you just posted (where `author` == your identity from `mcp__remargin__whoami`) are expected and OK — they're awaiting the other party's ack. Any **inbound** pending (a comment whose `author` is someone else) means you skipped its reply. Go back to step 4 and address it. **Do not move to step 7 with inbound pendings outstanding.**

7. **Return a structured summary.** Files touched, comments replied to, comments acked, ops performed. Explicitly confirm "0 inbound pendings remaining."

## Constraints

- Follow the remargin skill rules: use MCP > CLI, batch for N replies, ack only after the work is done, no per-call identity overrides, never delete other participants' comments.
- The resolved system prompt is the source of truth for what "process" means in this realm. If it conflicts with anything else in your context, the prompt wins.
