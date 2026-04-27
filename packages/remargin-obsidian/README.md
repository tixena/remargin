# remargin-obsidian

Obsidian plugin for the [remargin](../../README.md) commenting system. Adds a sidebar that
reads, writes, and threads remargin comments inline with your notes, plus optional editor-side
widgets that pretty-print remargin fenced blocks where they live in the document.

The plugin is a thin React + CodeMirror 6 layer on top of the `remargin` CLI; the CLI does the
file I/O, parsing, signing, and integrity checks. See the repository root for the full design
and CLI documentation.

## Editor widgets (pretty-print)

When enabled, replaces the raw remargin fenced blocks in your editor with rich, read-only
widgets that show the author, timestamp, and rendered markdown body. Editing always still
happens in the sidebar.

**Enable:** Settings -> Remargin -> Editor widgets.

**Where it applies:**

- Reading mode: widgets render instead of raw fences.
- Live Preview: same widgets, plus a small overlay so the cursor never lands inside a widget.
- Source Mode: raw fences stay (escape hatch for direct YAML inspection).

**Click a widget:** the sidebar scrolls to the matching card and flashes briefly. The Reply
button on the sidebar card is the only way to start a reply.

**Collapse / expand:** per session — closes when you reload Obsidian. Collapse state is shared
between reading mode and Live Preview, so toggling in one mirrors in the other.

**Default:** off on first release. We may flip the default to on after two releases of stable
behavior.

## Development

```bash
pnpm install
pnpm -F remargin-obsidian build       # production bundle into main.js
pnpm -F remargin-obsidian dev         # watch-mode rebuild
pnpm -F remargin-obsidian lint        # biome check
pnpm -F remargin-obsidian test        # node --test runner
pnpm -F remargin-obsidian typecheck   # tsgo --noEmit
```

The plugin loads from `main.js`, `manifest.json`, and `styles.css` at the package root. To run
inside Obsidian during development, symlink (or copy) those three files into
`<vault>/.obsidian/plugins/remargin/` and reload the vault.
