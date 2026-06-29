---
description: Consolidate a single managed markdown file — read every comment thread in its full context and re-create the document so it reflects all agreements, decisions, open issues, actionable items, and memos. Authorized for human requesters; for agent requesters only when the resolved folder system prompt explicitly permits consolidation.
---

# /remargin:consolidate <path> [--delete-comments | --preserve-comments]

Re-create a managed markdown file so its body reflects **everything** in its comment threads. Unlike `/remargin:process-file` (which replies to pending comments), consolidate rewrites the document itself. Comments are **preserved by default**; pass `--delete-comments` to remove them after the rewrite.

## Steps

1. **Identify the requester and entity type.** If you were invoked directly by the user in chat, the requester is that **human** — authorized. If you were invoked because a comment contains `/remargin:consolidate` (e.g. routed from `/remargin:process-file`), the **author of that comment** is the requester: read its `author` and `type` (`human` | `agent`) from the comment header via `mcp__remargin__comments` (`file` = the path). The entity type comes from the comment's `type` field — never guess it.

2. **Authorization gate.**
   - **Human requester** → proceed to step 3.
   - **Agent requester** → call `mcp__remargin__prompt_resolve` on `<path>`. The resolved folder-scoped system prompt body must **explicitly authorize consolidation** for this realm. If it does → proceed. If it does not — the prompt is silent on consolidation, or the resolver returned the locked Default (`is_default: true`) — **decline**: post one short `mcp__remargin__reply` on the requesting thread stating that consolidation is not authorized for this agent under the current system prompt, then **STOP**. Make no changes to the document.

3. **Resolve the comment parameter.** `--delete-comments` → delete mode. `--preserve-comments`, or no flag → **preserve mode (default)**.

4. **Read the full thread context.** Call `mcp__remargin__activity` on the file, then `mcp__remargin__comments` (`file` = the path — **not** `mcp__remargin__query`, which honors `.gitignore` and silently returns nothing on gitignored files), then `mcp__remargin__get` for the body. Read **every** thread end-to-end across **all** identities and targets: top-level comments, replies, acks, reactions, who each was addressed `to`, and the thread structure. Consolidation operates on the whole conversation, not the pending-for-me slice.

5. **Classify everything.** For each thread and item, classify it as one of: agreement, decision (settled), open discussion/rationale, open issue, actionable item, memo, or discardable/irrelevant. Reconcile each against the current document body.

6. **Re-create the document.** Rewrite the body so it reflects all of the above: settled decisions stated **as decisions** (never re-posed as open questions), agreements honored (never re-opened or contradicted), open issues and actionable items captured as such, useful rationale and memos folded in, discardable items dropped. The result must be internally consistent — no duplicate or stale sections, no settled item posed as a question. In **preserve** mode this is not a single whole-document write — see step 7.

7. **Apply the comment parameter.**
   - **Preserve (default):** you **cannot** whole-file `write` a commented document. Comment blocks are pinned at their lines; re-create the body by rewriting the **prose around** them — partial-line `mcp__remargin__write` (bottom-up, last gap first; never include a comment block's lines) plus `mcp__remargin__replace` for substitutions. Follow [the skill's rewriting-whole-files.md](../skills/remargin/rewriting-whole-files.md) exactly. Comment blocks stay anchored where they are; consolidation does not move or ack them.
   - **Delete:** after the body is re-created, remove every comment via `mcp__remargin__delete` (by id) or `mcp__remargin__purge`. With comments gone you may then rewrite the body freely (whole-file `write` is fine once no comment blocks remain).

8. **Return a structured summary.** Requester + entity type, authorization result, mode (preserve/delete), threads read, how each was classified, body sections rewritten, and comments preserved or deleted.

## Constraints

- Follow the remargin skill rules: MCP > CLI, comment-safe writes, no per-call identity overrides.
- "Take **all** the comments into account" is the contract: nothing agreed may be re-opened or contradicted, and nothing substantive may be dropped unless it was classified discardable.
- Invariants: no settled decision re-posed as an open question; no duplicate or stale sections; the recreated document is internally consistent.
- The requester's entity type comes from the requesting comment's `type` field (sender info). Only delete comments when `--delete-comments` was explicitly passed.
