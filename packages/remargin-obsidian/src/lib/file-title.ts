/**
 * Extract a human-readable title from a markdown document.
 *
 * Strategy:
 *   1. Prefer the first ATX H1 heading (`# Title`) found in the file body,
 *      searching before any other heading level to avoid picking up an H1
 *      buried inside a deeper section.
 *   2. Fall back to a frontmatter `title:` field when no H1 exists.
 *   3. Finally, fall back to the filename stem (no `.md` extension).
 *
 * The function is intentionally tolerant of YAML frontmatter: an opening
 * `---` fence (and its matching closing `---`) is skipped before scanning
 * for the H1 so that frontmatter dividers are not misread as content.
 */
export function extractTitle(contents: string, filename: string): string {
  const body = stripFrontmatter(contents);
  const h1 = matchFirstH1(body);
  if (h1) return h1;
  const fm = matchFrontmatterTitle(contents);
  if (fm) return fm;
  return filenameStem(filename);
}

/** Strip a leading YAML frontmatter block (`---`...`---`) from `contents`. */
function stripFrontmatter(contents: string): string {
  if (!contents.startsWith("---")) return contents;
  const afterFirstFence = contents.indexOf("\n");
  if (afterFirstFence < 0) return contents;
  const rest = contents.slice(afterFirstFence + 1);
  const closing = rest.indexOf("\n---");
  if (closing < 0) return contents;
  const afterClosing = closing + "\n---".length;
  // Advance past the newline that follows the closing fence, if any.
  const nextNewline = rest.indexOf("\n", afterClosing);
  return nextNewline >= 0 ? rest.slice(nextNewline + 1) : "";
}

/** Return the first ATX H1 (`# Title`) in `body`, trimmed, or null. */
function matchFirstH1(body: string): string | null {
  const lines = body.split("\n");
  for (const line of lines) {
    const m = line.match(/^#\s+(.+?)\s*$/);
    if (m) return m[1].trim();
  }
  return null;
}

/**
 * Read a `title:` field from the YAML frontmatter block. Supports bare
 * values, single-quoted strings, and double-quoted strings; values are
 * returned trimmed of surrounding whitespace and quotes.
 */
function matchFrontmatterTitle(contents: string): string | null {
  if (!contents.startsWith("---")) return null;
  const afterFirstFence = contents.indexOf("\n");
  if (afterFirstFence < 0) return null;
  const rest = contents.slice(afterFirstFence + 1);
  const closing = rest.indexOf("\n---");
  if (closing < 0) return null;
  const block = rest.slice(0, closing);
  for (const line of block.split("\n")) {
    const m = line.match(/^title:\s*(.+?)\s*$/);
    if (!m) continue;
    const raw = m[1];
    if (raw.startsWith("'") && raw.endsWith("'")) return raw.slice(1, -1);
    if (raw.startsWith('"') && raw.endsWith('"')) return raw.slice(1, -1);
    return raw;
  }
  return null;
}

/** Return the filename without its `.md` extension. */
function filenameStem(filename: string): string {
  const basename = filename.split("/").pop() ?? filename;
  const dot = basename.lastIndexOf(".");
  return dot > 0 ? basename.slice(0, dot) : basename;
}
