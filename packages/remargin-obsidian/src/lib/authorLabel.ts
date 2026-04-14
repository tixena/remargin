/**
 * Map a participant id to its render metadata: the label to display and
 * the tooltip title (if different from the label).
 *
 * This is a pure helper so every sidebar component produces the same
 * `"<Display Name>" + title="<id>"` pattern without duplicating the
 * conditional logic.
 *
 * Rules:
 * 1. `resolveDisplayName(id)` returns the registered display name, or
 *    the id itself when the registry is silent on it (see
 *    `resolveDisplayNameFrom`).
 * 2. The tooltip `title` is only set when the display differs from the
 *    id. When they match — either because the registry has no
 *    `display_name` or because the display name happens to equal the id
 *    — the `title` is `undefined` so React omits the attribute rather
 *    than rendering a redundant hover.
 */
export interface AuthorLabel {
  label: string;
  title: string | undefined;
}

export function authorLabel(
  id: string,
  resolveDisplayName: (id: string) => string
): AuthorLabel {
  const label = resolveDisplayName(id);
  return {
    label,
    title: label === id ? undefined : id,
  };
}
