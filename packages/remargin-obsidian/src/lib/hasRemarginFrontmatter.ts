/** Detects whether a document carries remargin-managed frontmatter. */

/**
 * Detect whether a markdown document's leading YAML frontmatter block
 * contains any `remargin_*` field.
 *
 * A "bare" markdown file — one `remargin write` has never touched —
 * has no `remargin_total` / `remargin_pending` / `remargin_pending_for`
 * / `remargin_last_activity` injected. The file-header
 * 'Initialize' affordance uses that absence as its trigger.
 *
 * The parser is intentionally lenient: it handles
 * - no frontmatter at all (returns false),
 * - an empty frontmatter block (returns false),
 * - an unclosed frontmatter block (returns false — treat as garbage),
 * - leading Unicode BOM (stripped),
 * - `\r\n` line endings,
 * - indented field names (still detected; YAML allows it).
 *
 * Any line inside the block whose `key:` segment starts with
 * `remargin_` counts as a hit. The check does NOT parse values — it
 * only looks at keys — because a partial frontmatter write that
 * injects `remargin_pending: 0` is proof enough that the file is
 * already managed.
 */
export function hasRemarginFrontmatter(contents: string): boolean {
  const block = extractFrontmatterBlock(contents);
  if (block == null) return false;
  for (const line of block.split("\n")) {
    // The first `key:` token on each line; leading indent is tolerated.
    const match = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/);
    if (match && match[1].startsWith("remargin_")) return true;
  }
  return false;
}

/**
 * Return the raw body of the leading YAML frontmatter block, or
 * `null` when the document has no opening fence or the fence is
 * unclosed.
 */
function extractFrontmatterBlock(contents: string): string | null {
  const normalized = stripBom(contents).replace(/\r\n/g, "\n");
  if (!normalized.startsWith("---")) return null;
  const afterFirstFence = normalized.indexOf("\n");
  if (afterFirstFence < 0) return null;
  const rest = normalized.slice(afterFirstFence + 1);
  // The closing fence must sit on its own line, so a `---` inside a field does not match.
  const closing = rest.indexOf("\n---");
  if (closing < 0) return null;
  return rest.slice(0, closing);
}

function stripBom(s: string): string {
  return s.charCodeAt(0) === 0xfe_ff ? s.slice(1) : s;
}
