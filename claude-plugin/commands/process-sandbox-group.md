---
description: Process all sandboxed files in the caller's vault that resolve to a given system prompt name. Removes the sandbox marker on success, leaves the file sandboxed on failure. Continue-on-failure across files within the group.
---

# /remargin:process-sandbox-group <prompt-name>

Given a system-prompt name, process every sandboxed file in this vault that resolves to that prompt.

## Steps

1. **Enumerate the caller's sandboxed files.** Call `mcp__remargin__sandbox_list`. The result is a list of file paths.

2. **Filter by resolved prompt.** For each file, call `mcp__remargin__prompt_resolve` and keep files whose resolved prompt name equals `<prompt-name>`. If the filtered list is empty, return a summary indicating no files matched and exit.

3. **Frame the work.** Look up the prompt body via `mcp__remargin__prompt_resolve` once (any matching file's resolution will do; they all resolve to the same prompt by construction). Treat the body as the current task definition.

4. **Process each file, sequentially.** For each file in the filtered list:
   1. Apply the per-file processing flow described in `/remargin:process-file` (read, surface pending comments, reply, edit body — all under the framed prompt).
   2. On success: call `mcp__remargin__sandbox_remove` with the file path.
   3. On failure: leave the sandbox marker in place. Record the failure. Carry on to the next file.

5. **Return a structured summary.** Files attempted, files successfully processed, files left sandboxed due to failure, per-file outcomes.

## Constraints

- Continue-on-failure within the group: a single file failure does not abort the rest.
- Same remargin skill rules as `/remargin:process-file`.
- Sandbox marker removal is per-file, after that file's processing succeeds — not at the end of the group. Partial progress is preserved.
- The system prompt is fixed for the duration of this invocation. Files outside the group are not touched.
