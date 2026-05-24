---
description: Process every currently-sandboxed file in the vault that resolves to a given system prompt name. Removes the sandbox marker on success, leaves the file sandboxed on failure. Continue-on-failure across files within the group.
---

# /remargin:process-sandbox-group <prompt-name>

Given a system-prompt name, process every sandboxed file in this vault that resolves to that prompt.

## Steps

1. **Enumerate currently-sandboxed files via activity.** Call `mcp__remargin__activity` with `path` = the vault scope (or the directory you're processing) and `pretty: true`. The result is a timestamp-sorted stream of events (comments, acks, edits, sandbox-adds, sandbox-removes) across **all identities**. Extract:
   - **Sandboxed set:** files whose most recent sandbox event is a `sandbox-add` with no later `sandbox-remove` by the same identity. This is the set to process.
   - **Recent context:** reactions on threads you're in, acks on your comments, comments addressed to others, edits, and signatures landed since your last action. Hold this for step 4 (per-file processing) — it's what your replies need to take into account. See remargin skill Critical rule 12.

   **Do not use `sandbox_list` for enumeration here.** It is caller-scoped and returns only the caller's own sandbox. In the typical agent-processing workflow the human user stages files for the agent — those won't appear in the agent's `sandbox_list`. `activity` sees stages by every identity, which is what this skill needs.

2. **Filter by resolved prompt.** For each file in the sandboxed set, call `mcp__remargin__prompt_resolve` and keep files whose resolved prompt name equals `<prompt-name>`. If the filtered list is empty, return a summary indicating no files matched and exit.

3. **Frame the work.** Look up the prompt body via `mcp__remargin__prompt_resolve` once (any matching file's resolution will do; they all resolve to the same prompt by construction). Treat the body as the current task definition.

4. **Process each file, sequentially.** For each file in the filtered list:
   1. Apply the per-file processing flow described in `/remargin:process-file` (read, surface pending comments, reply, edit body — all under the framed prompt). Take the activity context from step 1 into account when drafting replies — don't repeat an answer someone else already posted, adjust for edits that have moved the conversation.
   2. On success: call `mcp__remargin__sandbox_remove` with the file path.
   3. On failure: leave the sandbox marker in place. Record the failure. Carry on to the next file.

5. **Return a structured summary.** Files attempted, files successfully processed, files left sandboxed due to failure, per-file outcomes.

## Constraints

- Continue-on-failure within the group: a single file failure does not abort the rest.
- Same remargin skill rules as `/remargin:process-file`.
- Sandbox marker removal is per-file, after that file's processing succeeds — not at the end of the group. Partial progress is preserved.
- The system prompt is fixed for the duration of this invocation. Files outside the group are not touched.
