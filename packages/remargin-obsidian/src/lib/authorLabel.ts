/** The display label and tooltip title of a participant. */

/** How a participant renders: the label to display and, when it differs, the tooltip title. */
export interface AuthorLabel {
  label: string;
  title: string | undefined;
}

/**
 * Maps a participant id to its label and tooltip. `title` is set only when the display name
 * differs from the id, so React omits a hover that would just repeat the label.
 */
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
