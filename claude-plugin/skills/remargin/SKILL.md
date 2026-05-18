---
name: remargin
description: Routes natural-language requests for processing sandboxed or individual managed markdown files to the corresponding /remargin:* slash commands.
---

# Remargin (plugin skill)

## Trigger phrases

- "process sandboxed items in this vault", "submit the sandbox", "run the sandbox", "process the sandbox" → for each resolved prompt group, invoke `/remargin:process-sandbox-group <prompt-name>` once per group.
- "process this file", "submit this file", "process <path>", "run remargin on this" → invoke `/remargin:process-file <path>`.

## Routing rules

- When the user names a single file, route to `/remargin:process-file`.
- When the user asks for the sandbox / staging area / "everything I staged" / similar, route to `/remargin:process-sandbox-group`. Resolve groups via `sandbox_list` + `prompt_resolve`, iterate one slash-command invocation per group (the SAME slash command, called once per `<prompt-name>`).
- If the user gives no path and no sandbox cue, ask which they mean. Do not pick.
- Never bypass the slash command and reproduce its logic inline. The slash command is the canonical entry point; the skill is the router.

## Inherited rules

The per-file and per-group commands inherit the remargin skill rules (ack/batch/threading) — see the global remargin skill (`~/.claude/skills/remargin/SKILL.md`) for the canonical reference. In short:

- MCP > CLI. Use `mcp__remargin__*` tools, not shell-outs.
- Batch multiple replies into a single `mcp__remargin__batch` call.
- Ack only after the work has landed.
- Never delete another participant's comments.
- No per-call identity overrides.
