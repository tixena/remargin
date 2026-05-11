/**
 * Pure string transforms for the `system_prompt:` block of a
 * `.remargin.yaml` file. The editor reads the existing file, asks one
 * of these helpers for the new contents, then writes the result back —
 * so callers never have to round-trip through serde_yaml.
 *
 * The splice preserves every other line byte-for-byte (including
 * comments, blank lines, key order, and indentation). Only the lines
 * that make up the `system_prompt:` block are rewritten; the rest of
 * the file is concatenated unchanged.
 */

export interface SpliceResult {
  /** Full new file content. */
  content: string;
  /** True when the splice did nothing — caller can skip the write. */
  noop: boolean;
}

export interface SystemPromptBlock {
  /** Optional human-readable name. Empty / undefined ⇒ no `name:` line. */
  name?: string;
  /** Body. Empty string is allowed and written verbatim. */
  prompt: string;
}

/**
 * Insert or replace a `system_prompt:` block in `existing`. Surrounding
 * YAML is preserved byte-for-byte. When `existing` has no block the new
 * one is appended at EOF with one blank-line separator.
 */
export function spliceSystemPrompt(existing: string, block: SystemPromptBlock): SpliceResult {
  const rendered = renderBlock(block);
  const range = findBlockRange(existing);
  if (range) {
    const before = existing.slice(0, range.start);
    const after = existing.slice(range.end);
    const next = `${before}${rendered}${after}`;
    return { content: next, noop: next === existing };
  }
  const trimmed = existing.replace(/\s+$/u, "");
  const prefix = trimmed.length === 0 ? "" : `${trimmed}\n\n`;
  const next = `${prefix}${rendered}`;
  return { content: next, noop: next === existing };
}

/**
 * Remove the `system_prompt:` block from `existing`. No-op when the
 * block is absent. Trims one leading blank line if the removal would
 * otherwise leave a double-blank gap.
 */
export function removeSystemPrompt(existing: string): SpliceResult {
  const range = findBlockRange(existing);
  if (!range) return { content: existing, noop: true };
  const before = existing.slice(0, range.start);
  const after = existing.slice(range.end);
  let next = `${before}${after}`;
  // Collapse runs of 3+ consecutive newlines (i.e. more than one blank
  // line in a row) down to two — keeps one blank line between
  // surviving fields without accumulating gaps.
  next = next.replace(/\n{3,}/gu, "\n\n");
  // Trim trailing blank lines at EOF so removing a block at the end of
  // the file doesn't leave a dangling empty line.
  while (next.endsWith("\n\n")) {
    next = next.slice(0, -1);
  }
  return { content: next, noop: next === existing };
}

/**
 * Render a `system_prompt:` block as YAML. Bodies that span multiple
 * lines or contain YAML-sensitive characters use the `|` block scalar
 * style so the writer never has to escape anything.
 */
function renderBlock(block: SystemPromptBlock): string {
  const lines: string[] = ["system_prompt:"];
  if (block.name !== undefined && block.name !== "") {
    lines.push(`  name: ${quoteScalar(block.name)}`);
  }
  lines.push(...renderPromptField(block.prompt));
  return `${lines.join("\n")}\n`;
}

function renderPromptField(prompt: string): string[] {
  if (prompt === "") {
    return ['  prompt: ""'];
  }
  if (prompt.includes("\n") || needsBlockStyle(prompt)) {
    const body = prompt.endsWith("\n") ? prompt.slice(0, -1) : prompt;
    const indented = body
      .split("\n")
      .map((line) => `    ${line}`)
      .join("\n");
    return ["  prompt: |", indented];
  }
  return [`  prompt: ${quoteScalar(prompt)}`];
}

/**
 * True when a string contains a character that flow-style YAML would
 * either reject or interpret as syntax (so we'd rather use a block
 * scalar than reason about escapes).
 */
function needsBlockStyle(s: string): boolean {
  return /[#:&*!|>'"%@`]/u.test(s) || /^\s/u.test(s) || /\s$/u.test(s);
}

function quoteScalar(s: string): string {
  // Plain scalars are safe when they have none of the special chars
  // needsBlockStyle catches. Anything else gets double-quoted with
  // backslash escaping.
  if (!needsBlockStyle(s) && !s.includes("\n")) return s;
  const escaped = s.replace(/\\/gu, "\\\\").replace(/"/gu, '\\"');
  return `"${escaped}"`;
}

interface BlockRange {
  /** Byte offset of the `system_prompt:` line's first character. */
  start: number;
  /** Byte offset just past the last newline of the block. */
  end: number;
}

/**
 * Locate the `system_prompt:` block in `existing` by line scanning.
 * Returns `undefined` when no block exists. The block ends at the next
 * top-level key (column 0, non-blank, not a YAML continuation) or EOF.
 */
function findBlockRange(existing: string): BlockRange | undefined {
  const lines = existing.split("\n");
  let startLine = -1;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (/^system_prompt\s*:/u.test(line)) {
      startLine = i;
      break;
    }
  }
  if (startLine === -1) return undefined;
  let endLine = lines.length;
  for (let i = startLine + 1; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (line === "") continue;
    // Top-level key = first non-space char at column 0 that is not
    // a list dash. Sequence items under system_prompt would be indented,
    // so a bare `- ` at column 0 also breaks the block.
    if (!/^\s/u.test(line)) {
      endLine = i;
      break;
    }
  }
  // Convert line indices back to byte offsets. Split lost the
  // newlines so we add 1 per line up to but not including endLine.
  let start = 0;
  for (let i = 0; i < startLine; i++) {
    start += (lines[i]?.length ?? 0) + 1;
  }
  let end = start;
  for (let i = startLine; i < endLine; i++) {
    end += (lines[i]?.length ?? 0) + 1;
  }
  // The final block line may not have a trailing newline if EOF — clamp
  // end to existing.length to keep slicing correct.
  if (end > existing.length) end = existing.length;
  return { start, end };
}
