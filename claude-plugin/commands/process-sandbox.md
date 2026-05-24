---
description: Process every currently-sandboxed file in the vault. Groups by resolved system prompt and spawns one subagent per group so each group runs in its own fresh context — no system-prompt mixing across groups.
---

# /remargin:process-sandbox

Vault-wide sandbox processing. Each prompt group runs in an isolated subagent context so the system prompts don't bleed across groups.

## Steps

1. **Enumerate currently-sandboxed files via activity.** Call `mcp__remargin__activity` with `path` = the vault root and `pretty: true`. The result is a timestamp-sorted stream of events across **all identities**. A file is currently sandboxed iff its most recent sandbox event is a `sandbox-add` with no later `sandbox-remove` by the same identity. Collect that set.

   **Do not use `sandbox_list` for enumeration here.** It is caller-scoped and returns only the caller's own sandbox. In the typical agent-processing workflow the human user stages files for the agent — those won't appear in the agent's `sandbox_list`. `activity` sees stages by every identity.

2. **Group by prompt.** For each file, call `mcp__remargin__prompt_resolve` and bucket by the resolved prompt name. If no files are sandboxed, return a summary saying so and exit.

3. **Process each group via a subagent — sequentially.** For each prompt name with at least one sandboxed file:
   1. Spawn a subagent via the `Agent` tool with `subagent_type: "general-purpose"`. The prompt for the subagent: instruct it to process exactly the files in this group, under the resolved system prompt body, following the same flow as `/remargin:process-sandbox-group <prompt-name>`. Include the prompt body inline so the subagent has full context.
   2. Wait for the subagent to complete. Capture its summary.
   3. Move to the next group. Do NOT do groups in parallel — sequential subagents preserve the user's ability to follow what's happening.

4. **Aggregate.** Combine each subagent's outcome into a single vault-level summary: groups processed, files successfully processed, files left sandboxed due to failure, per-group outcomes.

## Constraints

- One subagent per group, sequential. Context isolation comes from the subagent boundary, not from process boundaries.
- Each subagent receives the prompt body inline; it must not consult any other system prompt.
- Sandbox marker removal happens inside each subagent on per-file success (same rule as `/remargin:process-sandbox-group`). Failures leave files sandboxed.
- Continue-on-failure across groups: a failure in group A does not stop group B.
- Same remargin skill rules as `/remargin:process-file` apply inside every subagent (MCP > CLI, batch for N replies, ack only after the work is done, etc.).
- Files outside the sandbox are not touched.
