/** Shortens a directory path for display. */

/**
 * Abbreviate a directory path by progressively truncating leading segments
 * to their first character until the total length fits within `maxChars`.
 *
 * Segments are abbreviated left-to-right; the rightmost segments are kept
 * intact as long as possible so the user can still identify the deepest
 * directories.
 *
 * Examples:
 *   abbreviatePath("src/01_personal/remargin/ui", 20) -> "s/0/remargin/ui"
 *   abbreviatePath("src/ui", 100)                     -> "src/ui"
 *   abbreviatePath("", 10)                             -> ""
 */
export function abbreviatePath(dirPath: string, maxChars: number): string {
  if (!dirPath) return "";

  const segments = dirPath.split("/").filter(Boolean);
  if (segments.length === 0) return "";

  if (joined(segments) <= maxChars) return segments.join("/");

  const abbreviated = [...segments];
  for (let i = 0; i < abbreviated.length; i++) {
    if (abbreviated[i].length > 1) {
      abbreviated[i] = abbreviated[i][0];
    }
    if (joined(abbreviated) <= maxChars) break;
  }

  return abbreviated.join("/");
}

/** Total character length of segments joined by `/`. */
function joined(segments: string[]): number {
  return segments.reduce((acc, s) => acc + s.length, 0) + Math.max(0, segments.length - 1);
}
