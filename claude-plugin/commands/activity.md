---
description: Report "what changed since the caller last touched a managed file" across the vault. Surfaces comments, acks, edits, and sandbox-adds per file. Read-only — never mutates.
---

# /remargin:activity [path]

Run `mcp__remargin__activity` to report what changed since the caller last acted on each managed `.md` file under `path` (defaults to cwd). Use this at the start of a session, or whenever the agent needs to notice what happened while it was away and decide whether to react.

## Steps

1. **Invoke the activity tool.** Call `mcp__remargin__activity` with `path` (the argument, or omit for cwd) and `pretty: true`. Do not pass `since` unless the user explicitly asked for a specific cutoff — the default per-file caller-last-action cutoff is the point of this tool.

2. **Surface the output verbatim.** Paste the pretty-printed activity timeline into the response. Do not paraphrase, summarize, or reformat.

3. **Decide whether to react.** After surfacing the output, briefly call out anything actionable: comments addressed to the caller that are pending, edits on files the caller is staging, sandbox-adds by other identities the caller might want to look at. Keep this short — one or two sentences. The user (or the next agent step) decides what to do with it.

## Constraints

- Read-only. Never mutate state from this command.
- Do not hand-roll the cutoff from `comments` / `query` — those don't compute the per-file caller-last-action cutoff and don't fold edits / re-sandboxes into the same change list. `activity` is the only correct tool for this question.
- If the user gave a `path` argument that resolves outside any remargin realm, surface the tool's error directly. Don't try to recover by searching adjacent directories.
- If `pretty: true` output is empty, that's the correct answer: "nothing new since you last acted in this path."
