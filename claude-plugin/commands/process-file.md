---
description: Process a single managed markdown file under its resolved system prompt. Reads pending comments, replies, acks, and edits the doc body as the prompt directs. Does NOT clear the file's sandbox marker (sandbox cleanup is the per-group command's job).
---

# /remargin:process-file <path>

Given a file path, process the file under its resolved system prompt.

## Steps

1. **Resolve the system prompt.** Call `mcp__remargin__prompt_resolve` with the given path. The result contains the prompt name and body. The resolver falls back to a locked Default body when the `.remargin.yaml` walk exhausts.

2. **Frame the work.** Read the prompt body and treat it as your current task definition. Everything below operates under that prompt.

3. **Process the file.** Read the file via `mcp__remargin__get`. Surface pending comments via `mcp__remargin__query` with `pending: true`. Reply to each via `mcp__remargin__batch` (one batch op, one reply per comment, `auto_ack` where appropriate per the remargin skill rules). Edit the doc body via `mcp__remargin__write` partial-line writes where the prompt calls for it.

4. **Do NOT remove the sandbox marker.** Sandbox cleanup is the responsibility of the per-group command, not this one. Manual per-file invocation is non-destructive on sandbox state.

5. **Return a structured summary.** Files touched, comments replied to, comments acked, ops performed.

## Constraints

- Follow the remargin skill rules: use MCP > CLI, batch for N replies, ack only after the work is done, no per-call identity overrides, never delete other participants' comments.
- The resolved system prompt is the source of truth for what "process" means in this realm. If it conflicts with anything else in your context, the prompt wins.
