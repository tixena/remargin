/**
 * Line-snap helper for the sidebar `+` button.
 *
 * Remargin rejects comments inserted inside an existing remargin block because
 * it would corrupt the YAML/content boundary. When the user clicks `+` with
 * the cursor anywhere inside such a block, we snap the target line forward to
 * the first line after the enclosing block's closing fence so the new comment
 * lands in a legal position.
 *
 * Both inputs and outputs are 1-indexed line numbers (matching `remargin
 * --after-line`). `lines` is the file content split on `\n`.
 */

interface BlockRange {
  /** 1-indexed line number of the opening ```remargin fence. */
  startLine: number;
  /** 1-indexed line number of the matching closing fence. */
  endLine: number;
}

/**
 * Walk `lines` once and collect every well-formed remargin block. A block
 * starts on a line matching `^`{3,}`remargin` and ends on the first line that
 * is exactly the same number of backticks, so stacked fences from nested code
 * samples do not confuse the scanner.
 */
function findRemarginBlocks(lines: string[]): BlockRange[] {
  const blocks: BlockRange[] = [];
  let inBlock = false;
  let fence = "";
  let startLine = 0;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!inBlock) {
      const match = line.match(/^(`{3,})remargin\s*$/);
      if (match) {
        inBlock = true;
        fence = match[1];
        startLine = i + 1;
      }
      continue;
    }
    if (line.trim() === fence) {
      blocks.push({ startLine, endLine: i + 1 });
      inBlock = false;
      fence = "";
      startLine = 0;
    }
  }

  if (inBlock) {
    // Unclosed block: treat it as running to end of file so a cursor anywhere
    // inside still snaps past it (which lands at EOF).
    blocks.push({ startLine, endLine: lines.length });
  }

  return blocks;
}

/**
 * Given a file's lines and a 1-indexed target line, return a target line that
 * is guaranteed to be outside of any remargin block.
 *
 * - If `targetLine` is outside every block, it is returned unchanged.
 * - If it falls inside a block (inclusive of both fence lines), the returned
 *   line is the line immediately after the block's closing fence.
 * - If that snapped line falls inside another (stacked/adjacent) block, the
 *   snap repeats until the target lands outside every block.
 * - If the last block runs to end of file, the result is the file length — in
 *   practice this means the CLI will append the new comment at EOF.
 *
 * The function never returns a line number less than `targetLine`; the snap is
 * strictly forward so user intent ("comment about this region") is preserved.
 */
export function snapAfterCommentBlock(lines: string[], targetLine: number): number {
  if (targetLine < 1) return targetLine;
  const blocks = findRemarginBlocks(lines);
  if (blocks.length === 0) return targetLine;

  let snapped = targetLine;
  // Multiple passes handle stacked/adjacent blocks: each snap might land us
  // inside the next block, which needs another snap, and so on.
  // Bounded by the number of blocks (each block can only be crossed once).
  for (let i = 0; i < blocks.length; i++) {
    const enclosing = blocks.find((b) => snapped >= b.startLine && snapped <= b.endLine);
    if (!enclosing) break;
    snapped = enclosing.endLine + 1;
  }

  // Cap at file length: `--after-line N` where N > lines.length still appends
  // at EOF, but returning a stable number keeps callers predictable.
  if (snapped > lines.length) return lines.length;
  return snapped;
}
