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

## Critical Rule: Never Use Filesystem Tools for Markdown

**NEVER use `Read`, `Edit`, or `Write` tools to manipulate markdown documents that remargin manages.** Always use the remargin MCP tools instead. This ensures:

- Comments are never accidentally deleted or corrupted
- Document integrity (checksums, signatures) is preserved
- Comment threading and acknowledgment state stays consistent
- Structural lint checks run before and after every operation

Use `Grep` and `Glob` for discovery (finding files, searching content), but all reads and writes go through remargin.

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
| `get` | Read a file's contents (supports `start_line`/`end_line` for ranges) |
| `write` | Write file contents (comment-preserving — never destroys existing comments) |
| `metadata` | Get document metadata (frontmatter, comment counts, pending status) |

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

### Review a document (full workflow)

1. `ls` to find the document
2. `get` to read its contents
3. `comments` to see existing discussion
4. `comment` or `batch` to add your review comments
5. `ack` to acknowledge comments addressed to you
6. `query` to check for anything else pending

### Write document content

```
remargin write path="docs/design.md" content="Updated content here..."
```

The `write` tool preserves all existing comments in the document. It will not destroy comment blocks.

## Key Concepts

- **Identity**: Every comment has an author (string identifier) and type (`human` or `agent`)
- **Threading**: Comments can reply to other comments via `reply_to` (direct parent) and `thread` (root ancestor)
- **Acknowledgment**: Comments are acknowledged with `ack`, recording who and when (full timestamp)
- **Integrity**: Every comment gets a checksum. In strict mode, comments are also signed with Ed25519 keys
- **Batch atomicity**: Multiple comment operations in one `batch` call produce a single document write
- **Comment preservation**: The tool guarantees no comments are lost during writes — the comment list before and after must match exactly with only the expected delta
