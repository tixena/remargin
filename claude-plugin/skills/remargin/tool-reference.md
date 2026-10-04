# Tool reference

Each op exists at both surfaces: MCP `mcp__remargin__<op>`; CLI `remargin <op>`. Run `remargin <op> --help` for the exhaustive flag list. The rules governing when and how to use these ops stay in SKILL.md; this file is the argument-level lookup.

## Document access

| Op | Purpose |
|----|---------|
| `ls` | List files and directories. |
| `get` | Read a file. `start_line`/`end_line`/`line_numbers`/`binary`. The response carries `total_lines`; a window cut to the size budget carries `effective_end_line` (continue from the next line). Run `metadata` before binary reads. |
| `write` | Write file content (comment-preserving). `create`, `raw`, `binary`, `start_line`/`end_line` for partial writes. |
| `metadata` | Frontmatter, comment counts, pending counts, mime, size. |
| `rm` | Remove a file. |

## Commenting

| Op | Purpose |
|----|---------|
| `comment` | Add one top-level comment. `after_line`, `after_comment`, `attachments`, `to`, `sandbox`. For thread replies use `reply`. |
| `reply` | **PREFERRED** for thread responses. `parent_id` (required), `content`, `auto_ack` (smart default: ack iff parent.author != caller), `to`, `attachments`, `sandbox`, `kind`. |
| `comments` | List one file's comments as columnar rows `{total, comment_cols, comments, effective_limit?}`. Filters `kind`, `pending`, `pending_for`, `pending_for_me`; `include_integrity` adds `checksum`/`signature`; paged with `offset`/`limit`. Reads gitignored files. CLI `--json` gives plain objects; `--pretty` the threaded display. |
| `batch` | Add multiple comments atomically (single write, single verify). Each sub-op supports its own `auto_ack`, `reply_to`, etc. **Use this for N>1 comments on the same file.** |
| `edit` | Edit an existing comment. Cascades ack-clear to children. |
| `delete` | Delete one or more comments. Cleans up attachments. |
| `ack` | Acknowledge one or more comments. Omit `file` to resolve by ID across the directory tree. |
| `react` | Add or remove an emoji reaction. `remove=true` to unreact. |

## Sandbox

Sandbox staging is a per-identity, per-file marker stored in document frontmatter. Soft claim only — not "committed" or "submitted."

| Op | Purpose |
|----|---------|
| `sandbox_add` | Stage one or more markdown files. Idempotent. |
| `sandbox_remove` | Remove the caller's marker. Idempotent. |
| `sandbox_list` | List files staged for the caller's identity. |

## Identity

| Op | Purpose |
|----|---------|
| `identity_create` | Render a ready-to-use identity YAML block. Returns `{identity, type, key, yaml}`. Caller writes the YAML to disk; agents are banned from writing to `.remargin.yaml` directly. `mode:` is never emitted. |

## Search and quality

| Op | Purpose |
|----|---------|
| `activity` | "What's new since X" across managed `.md`. Per-file change records (comments, acks, sandbox-adds) sorted by ts. With `since` omitted, the per-file cutoff is the caller's last action — files where the caller has never acted return everything. Folds in comment edits (via `edited_at`) and sandbox refreshes. Columnar `{…, total, change_cols, files, effective_limit?}`; paged with `offset`/`limit` over changes. |
| `query` | Search across documents for comments. Filters: `pending` (broad — directed + broadcast), `pending_for` (directed to recipient), `pending_for_me` (directed to caller), `pending_broadcast` (unacked broadcasts), `author`, `since`, `comment_id`. Pending filters compose as a union. `expanded=true` includes comments inline. Also filters `kind`, `content_regex`. Columnar `{base_path, total, comment_cols, results, effective_limit?}`; paged with `offset`/`limit` over comments (files in summary mode). |
| `search` | Search across documents for text. `regex`, `scope` (all/body/comments), `context`, `ignore_case`, `limit`/`offset` (paged; response always carries `total`). Returns the compact grouped columnar shape `{total, match_cols, files}`. Pages are auto-sized under the session spill cap; a bounded page carries `effective_limit` — advance the `offset` request param for the rest. |
| `report_spill` | Signal that your client just spilled a remargin result to a file (over its output-token limit). Ratchets the session's page cap DOWN so future `search` pages fit. Infers the size from the last result; `size` overrides. Call it BEFORE reading the spilled file. See Critical rule 22. |
| `lint` | Structural lint checks. |
| `verify` | Check checksums and signatures against the registry. |
| `migrate` | Convert old-format inline comments to remargin format. |
| `purge` | Strip all comments (destructive — user-initiated only). |

## Plan (preview surface)

| Op | Purpose |
|----|---------|
| `plan` | Projection for any mutating op. Takes `op` + the same args as the underlying call. Returns predicted outcome without touching disk. Covers `ack`, `batch`, `comment`, `reply`, `delete`, `edit`, `migrate`, `purge`, `react`, `sandbox-add`, `sandbox-remove`, `sign`, `write`. `op: "reply"` is a synonym for `op: "comment"` with a required `parent_id`. |

## Admin (CLI-only — user-facing setup)

- `keygen` — generate Ed25519 signing key pair.
- `mcp` — run the stdio MCP server (entry point for `mcp__remargin__*`).
- `obsidian` — install/uninstall the Obsidian vault plugin.
- `registry` — manage the participant registry file.
- `resolve-mode` — resolve the effective enforcement mode.
- `session` — `session launch [<name>]` starts one Claude `/loop` session per identity (every identity discovered under cwd, or a named `sessions:` manifest fleet) into a multiplexer (herdr/tmux); gated behind the `session` build feature. See the README.
- `skill` — manage the Claude Code skill (SKILL.md and these reference files).
- `identity` — print configured identity. `identity create --identity NAME --type human|agent [--key PATH]` prints YAML to stdout.
- `version` — print version info.

## Permissions

| Need | MCP tool | CLI |
|---|---|---|
| Restrict a path | _CLI-only_ | `remargin claude restrict` |
| Unrestrict a path | _CLI-only_ | `remargin claude unrestrict` |
| Show resolved permissions | `mcp__remargin__permissions_show` | `remargin permissions show` |
| Check if path is restricted | `mcp__remargin__permissions_check` | `remargin permissions check` |

`claude restrict` and `claude unrestrict` are intentionally CLI-only:
they mutate permission policy and that decision belongs to the human,
not to the agent. The MCP surface deliberately omits them, and
`mcp__remargin__plan` also rejects `op="claude_restrict"` and
`op="claude_unrestrict"` for the same reason. Never call
`remargin claude unrestrict` from a Bash subprocess to clear a denial
— surface the denial to the user and wait for explicit consent.

No identity flags on these commands — editing your own permissions doesn't need an identity declaration.
