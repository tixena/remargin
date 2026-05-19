/**
 * Build a path with the configured base URL prefix.
 * `import.meta.env.BASE_URL` is `'/remargin/'` in production and `'/'` in dev
 * with no base; this helper normalizes leading/trailing slashes so we never
 * emit `//foo`.
 */
export const asset = (p: string): string => {
  const base = import.meta.env.BASE_URL.replace(/\/+$/, '');
  const path = p.replace(/^\/+/, '');
  return `${base}/${path}`;
};
