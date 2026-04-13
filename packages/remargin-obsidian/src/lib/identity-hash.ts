/**
 * Derive a short, stable identity fingerprint for display in comment
 * headers. Returns a lowercase three-character code.
 *
 * Callers should pass the most stable identity they have. When a full
 * identity string (a hash) is available, the first three characters are
 * returned verbatim. When only a human-readable author name is available,
 * a deterministic hash is computed so that the same name always renders
 * with the same shortcode.
 *
 * Empty input returns an empty string.
 */
export function identityShort(input: string | undefined | null): string {
  if (!input) return "";
  const trimmed = input.trim().toLowerCase();
  if (!trimmed) return "";

  // If the caller passed a long-looking hex/base32-ish token, the first
  // three characters are already a fingerprint — keep them.
  if (trimmed.length >= 8 && /^[0-9a-z]+$/.test(trimmed)) {
    return trimmed.slice(0, 3);
  }

  // Otherwise derive a deterministic shortcode. FNV-1a 32-bit is small,
  // well-distributed for short strings, and easy to read.
  let hash = 0x811c9dc5;
  for (let i = 0; i < trimmed.length; i++) {
    hash ^= trimmed.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  // Encode three characters from an alphabet that excludes visually
  // confusing glyphs (0/o, 1/l/i).
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  const out: string[] = [];
  let unsigned = hash >>> 0;
  for (let i = 0; i < 3; i++) {
    out.push(alphabet[unsigned % alphabet.length] ?? "x");
    unsigned = Math.floor(unsigned / alphabet.length);
  }
  return out.join("");
}
