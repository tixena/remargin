/** The identity of one read the sidebar issues. */

/**
 * Identity of one read the sidebar issues. A result whose token is no
 * longer the current one has been superseded and must not be shown or
 * stored. `parts` carries everything that makes a read distinct: the
 * refresh counter, the subject, and any counter a section bumps itself.
 */
export function readToken(...parts: (string | number)[]): string {
  return JSON.stringify(parts);
}
