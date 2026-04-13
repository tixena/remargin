/**
 * Patch the top-level `mode:` field of a minimal YAML document used by
 * `.remargin.yaml`. Preserves every other line, comment, and blank line.
 *
 * Rules:
 * - If an existing top-level `mode:` key is present, its value is rewritten
 *   in place and the rest of the line (indentation, trailing comment) is
 *   kept intact.
 * - If no top-level `mode:` key exists, a new `mode: <value>` line is
 *   appended. When the file is non-empty and does not already end with a
 *   newline, one is added before the appended line.
 * - Only top-level keys (no leading whitespace) are considered. This matches
 *   how `.remargin.yaml` is actually authored — the file is a flat map.
 *
 * This deliberately avoids a full YAML parse/serialise round trip so the
 * plugin does not clobber the user's formatting, comments, or key order.
 */
export function patchModeInYaml(source: string, mode: string): string {
  const lines = source.split("\n");
  const keyRe = /^(mode\s*:)(\s*)([^#\n]*?)(\s*)(#.*)?$/;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] ?? "";
    // Only top-level keys — any leading whitespace means this is nested
    // under something else and should not be touched.
    if (/^\s/.test(line)) continue;
    const match = keyRe.exec(line);
    if (!match) continue;
    const [, key, spaceAfterColon, , trailingSpace, comment] = match;
    const gap = spaceAfterColon && spaceAfterColon.length > 0 ? spaceAfterColon : " ";
    const tail = comment ? `${trailingSpace ?? " "}${comment}` : "";
    lines[i] = `${key}${gap}${mode}${tail}`;
    return lines.join("\n");
  }

  // No existing top-level `mode:` — append.
  if (source.length === 0) {
    return `mode: ${mode}\n`;
  }
  const needsLeadingNewline = !source.endsWith("\n");
  return `${source}${needsLeadingNewline ? "\n" : ""}mode: ${mode}\n`;
}
