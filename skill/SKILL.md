---
name: remargin
description: "Document access layer and structured commenting system for markdown files. Use remargin MCP tools (ls, get, write, comment, ack, query) instead of Read/Edit/Write when working with markdown documents. Provides threaded multi-player comments, integrity checks, batch operations, and file access through MCP."
user-invocable: true
---

# Remargin

Remargin is a **document access layer** and **structured commenting system** for markdown files. It replaces direct filesystem access with a set of MCP tools that read, write, comment on, and query markdown documents.

## When to Use

Use remargin when:
- Reading or writing markdown documents under review or discussion
- Adding comments, replies, reactions, or acknowledgments to documents
- Searching across documents for pending comments or activity
- Working in a project where remargin MCP is configured

**Trigger phrases**: "remargin that", "remargin this", "let's discuss", "let's review", "review this document", "discuss this document", "discuss on that document", "comment on", "discuss this doc", "what comments are pending", "acknowledge", "react to", "check the document", "start a discussion", "leave a comment", "any pending comments"

## Critical Rule: Never Operate on Files Directly

**NEVER use `Read`, `Edit`, `Write`, or `Bash` (awk, sed, cat, grep) to read, modify, or inspect markdown documents that remargin manages.** Always use the remargin MCP tools instead:

- Use `remargin get` to read file contents (with `start_line`/`end_line` for ranges, `line_numbers=true` to see line numbers)
- Use `remargin search` to find text and get line numbers
- Use `remargin write` to update body content
- Use `remargin comment` / `batch` to add comments
- Use `remargin comments` to list comments in a file

This ensures:

- Comments are never accidentally deleted or corrupted
- Document integrity (checksums, signatures) is preserved
- Comment threading and acknowledgment state stays consistent
- Structural lint checks run before and after every operation

The document access layer exists to prevent agents from corrupting comments. Bypassing it with direct filesystem tools defeats the entire purpose of remargin.

Use `Grep` and `Glob` only for discovery (finding files across the repo), not for reading or modifying managed documents.

## Permissions Setup

If remargin MCP tools require approval on every call, ask the user to add this wildcard to their `settings.local.json` (or `settings.json`) allow list:

```json
{
  "permissions": {
    "allow": [
      "mcp__remargin__*"
    ]
  }
}
```

This approves all remargin tools at once — no per-tool confirmation needed.

## MCP Tools Reference

### Document Access

| Tool | Purpose |
|------|---------|
| `ls` | List files and directories |
| `get` | Read a file's contents (supports `start_line`/`end_line` for ranges, `line_numbers` to prefix each line with its number) |
| `write` | Write file contents (comment-preserving — never destroys existing comments). Pass `create=true` to create a new file. |
| `metadata` | Get document metadata (frontmatter, comment counts, pending status) |

#### `write` safety

`write` replaces the **entire file content**. All existing comments must be carried **intact** in the new content — the checksum on each comment validates this. If the write would remove or alter any comment, remargin rejects it. This is by design — it is the core reason remargin exists.

- Never use `write` as a shortcut to rewrite a file that has other participants' comments. If you need to update body text, read the full file first with `get`, make targeted changes to body segments only, and write back the complete content including all comment blocks verbatim.
- If a `write` fails due to comment preservation, do **NOT** delete comments to make it work. Find an alternative approach (e.g., write to a different file) or ask the user.

#### Never delete comments you didn't author

Never delete another participant's comment unless the user explicitly tells you to. If an operation fails because of someone else's comment, find an alternative approach or ask the user. Deleting someone's comment to unblock your own operation is never acceptable.

### Commenting

| Tool | Purpose |
|------|---------|
| `comment` | Add a comment to a document (supports `reply_to`, `after_line`, `after_comment`, attachments) |
| `comments` | List all comments in a document |
| `batch` | Add multiple comments atomically (one write, one diff) |
| `edit` | Edit an existing comment (cascading ack clear on children) |
| `delete` | Delete one or more comments (cleans up attachments) |
| `ack` | Acknowledge one or more comments |
| `react` | Add or remove an emoji reaction |

### Search and Quality

| Tool | Purpose |
|------|---------|
| `query` | Search across documents for comments — filter by `pending`, `pending_for`, `author`, `since` |
| `search` | Search across documents for text matches — supports `regex`, `scope` (all/body/comments), `context` lines, `ignore_case` |
| `lint` | Run structural lint checks on a document |
| `verify` | Verify comment integrity (checksums and signatures) |
| `migrate` | Convert old-format inline comments to remargin format |
| `purge` | Strip all comments from a document |

## Comment Format

Comments use a fenced code block with language tag `remargin` and a YAML header:

````markdown
```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:a1b2c3d4...
---
This is the comment content.

It can be multiple paragraphs with **markdown formatting**.
```
````

With threading and acknowledgment:

````markdown
```remargin
---
id: xyz
author: claude
type: agent
ts: 2026-04-06T14:33:00-04:00
reply-to: abc
thread: abc
checksum: sha256:e5f6g7h8...
ack:
  - eduardo@2026-04-06T15:00:00-04:00
---
Replying to the comment above.
```
````

**You do not write this format manually.** The MCP tools produce it. Use `comment`, `batch`, `ack`, and `react` tools to create and manage comments.

## Common Workflows

### Read a document

```
remargin get path="docs/design.md"
remargin get path="docs/design.md" start_line=1 end_line=50
remargin get path="docs/design.md" line_numbers=true
remargin get path="docs/design.md" start_line=50 end_line=60 line_numbers=true
```

### List files in a directory

```
remargin ls path="docs/"
```

### Add a comment

```
remargin comment file="docs/design.md" content="This section needs more detail on error handling."
```

### Reply to a comment

```
remargin comment file="docs/design.md" content="Good point, I'll expand this." reply_to="abc"
```

### Add a comment after a specific line

```
remargin comment file="docs/design.md" content="Consider edge case X here." after_line=42
```

### Add multiple comments at once

```
remargin batch file="docs/design.md" comments=[{content: "First note", after_line: 10}, {content: "Second note", after_line: 25}]
```

### Acknowledge comments

```
remargin ack file="docs/design.md" ids=["abc", "def"]
```

### React to a comment

```
remargin react file="docs/design.md" id="abc" emoji="👍"
```

### Find pending comments across documents

```
remargin query path="docs/" pending=true
remargin query path="." pending_for="eduardo"
```

### Search for text across documents

```
remargin search pattern="notification"
remargin search pattern="error" path="docs/" scope="comments"
remargin search pattern="TODO|FIXME" regex=true ignore_case=true context=2
```

### Review a document (full workflow)

1. `ls` to find the document
2. `get` to read its contents
3. `comments` to see existing discussion
4. `comment` or `batch` to add your review comments
5. Process and `ack` comments addressed to you (see below)
6. `query` to check for anything else pending

### Processing comments addressed to you

When comments are addressed to you (via `to` field) or the user asks you to "process" comments, follow this workflow **in order**. Ack is the **last step**, not the first.

1. **Read** the comment and any referenced documents, links, or files mentioned in it
2. **Reason** about what the comment is saying — what is the author asking, deciding, or informing you about?
3. **Execute** any actionable items:
   - If the comment asks you to read something, read it and form an understanding
   - If the comment asks you to do work, do the work (create files, update docs, write code, create tasks)
   - If the comment makes a decision, update your plans and any affected documents accordingly
   - If the comment asks a question, reply with a substantive answer (not a summary of the question)
4. **Reply** with a comment (via `reply_to`) that demonstrates you did the work — reference specifics, share conclusions, raise concerns. Do not reply with summaries of what the comment said back to the person who wrote it.
5. **Ack** the comment only after all the above is complete. Ack means "I have fully addressed this." A premature ack is a lie — it tells the author their comment was handled when it wasn't.

**Common mistakes to avoid:**
- Do NOT ack immediately after reading. Ack is not "I read this."
- Do NOT reply with a surface-level summary. "Understood, phase 1 is CLI backend" adds nothing.
- Do NOT ack and then start doing the work. The work must be done before the ack.
- Do NOT skip referenced files. If the comment says "look at X", you must read X before acking.

### Multiple comments on the same file

When placing 2 or more comments on the same file, **always use `batch`**. Do not call `comment` sequentially.

Why: each `comment` call inserts a block into the file, shifting all subsequent line numbers. If you place comment A at line 50, line 80 in the original file is now line 90 (or whatever). Your second `comment --after-line 80` will land in the wrong place.

`batch` is atomic — all line numbers are resolved against the original document in a single operation. No displacement.

```
remargin batch file="docs/design.md" operations=[
  {content: "First note", after_line: 50},
  {content: "Second note", after_line: 80},
  {content: "Reply to abc", reply_to: "abc"}
]
```

### Write document content

```
remargin write path="docs/design.md" content="Updated content here..."
```

The `write` tool preserves all existing comments in the document. It will not destroy comment blocks.

### Create a new document

```
remargin write path="docs/new-doc.md" content="# New Document\n\nInitial content." create=true
```

The `create` flag creates a new file. It will fail if the file already exists (to prevent accidental overwrites).

## Comment Display Format

When displaying comments to the user (after `comments`, `comment`, `batch`, or during a review), **always use this exact format**. This format enables ctrl+clickable `file:line` links in the terminal.

### Starting a discussion or review

When the user asks to review or discuss a document, show the document path as a clickable link first:

```
docs/design.md
```

Then display comments in the format below.

### Single comment

```
docs/design.md:25
  abc · eduardo (human) · 2026-04-06 14:32
  │ The comment content goes here, wrapped
  │ across multiple lines as needed.
  │ pending
```

### Threaded reply (indented under parent)

```
docs/design.md:25
  abc · eduardo (human) · 2026-04-06 14:32
  │ I think the registry should support key rotation.
  │ What happens when someone's key is compromised?
  │ pending

  docs/design.md:35
    xyz · claude (agent) · 2026-04-06 14:33
    │ ⤷ reply-to: abc
    │ Good question. I'd add a `revoked_keys` list to the
    │ registry entry. When a key is revoked, all comments
    │ signed with it get flagged but not deleted.
    │ ✓ acked by eduardo @ 2026-04-06 15:00
```

### Footer

```
─────
3 comments · 2 pending
```

### Rules

1. **`file:line` per comment**: Every comment gets its own `path:line` link on a line by itself. This is required for ctrl+click to work in the terminal.
2. **Repeat file path per comment**: Even in threads where multiple comments reference the same file, repeat the full `path:line` on each.
3. **Root comments indent 2 spaces**: The `id · author · timestamp` header line starts with 2 spaces.
4. **Replies indent 2 more**: Each level of reply nesting adds 2 spaces (reply = 4 spaces, reply-to-reply = 6 spaces, etc.).
5. **Content lines use `│` bar prefix**: All content lines start with `│` at the same indent as the header.
6. **Threading marker**: Replies show `│ ⤷ reply-to: <id>` as the first content line.
7. **Reactions before status**: If the comment has reactions, show them on their own line before the status line (e.g. `│ 👍 jorge, alice`).
8. **Status as last content line**: Show `│ pending` or `│ ✓ acked by <who> @ <when>`.
9. **Content truncation at 5 lines**: When content exceeds 5 lines, show the first 4 lines fully, then truncate the 5th line with `...`. Users can use `get` to read the full document.
10. **Hide acked comments by default**: Do not show acked comments unless the user asks for them or you are displaying a full thread. This keeps the review focused on what needs attention.
11. **Timestamp format**: Use short format in the header: `YYYY-MM-DD HH:MM` (no timezone, no seconds).
12. **Blank line between comments**: Separate each comment block with a blank line.
13. **Summary footer**: End with a `─────` separator and a line showing `N comments · M pending`.

## Key Concepts

- **Identity**: Every comment has an author (string identifier) and type (`human` or `agent`)
- **Threading**: Comments can reply to other comments via `reply_to` (direct parent) and `thread` (root ancestor)
- **Acknowledgment**: Comments are acknowledged with `ack`, recording who and when (full timestamp)
- **Integrity**: Every comment gets a checksum. In strict mode, comments are also signed with Ed25519 keys
- **Batch atomicity**: Multiple comment operations in one `batch` call produce a single document write
- **Comment preservation**: The tool guarantees no comments are lost during writes — the comment list before and after must match exactly with only the expected delta
