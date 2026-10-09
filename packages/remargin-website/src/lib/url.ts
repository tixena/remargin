/** URL helpers for the website. */

/**
 * Build a path with the configured base URL prefix. Leading and trailing slashes are
 * normalized so the result never contains `//foo`.
 */
export const asset = (p: string): string => {
  const base = import.meta.env.BASE_URL.replace(/\/+$/, '');
  const path = p.replace(/^\/+/, '');
  return `${base}/${path}`;
};
