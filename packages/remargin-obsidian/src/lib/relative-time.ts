/**
 * Format a timestamp as a short relative string for the comment card header.
 *
 * Today (less than 24h ago): `Xh` for hours, `Xm` for minutes, `now` for
 * under a minute.
 * Yesterday (1–6 days ago): `Xd`.
 * Older: abbreviated month + day, e.g. `Apr 7`.
 *
 * Returns an empty string for missing or unparseable input.
 *
 * `now` is injected for deterministic testing; callers in production should
 * rely on the default (`Date.now()`).
 */
export function formatRelative(ts: string | Date | undefined, now: Date = new Date()): string {
  if (!ts) return "";
  const then = ts instanceof Date ? ts : new Date(ts);
  const thenMs = then.getTime();
  if (Number.isNaN(thenMs)) return "";

  const diffMs = now.getTime() - thenMs;
  if (diffMs < 0) return "now";

  const minute = 60 * 1000;
  const hour = 60 * minute;
  const day = 24 * hour;

  if (diffMs < minute) return "now";
  if (diffMs < hour) return `${Math.floor(diffMs / minute)}m`;
  if (diffMs < day) return `${Math.floor(diffMs / hour)}h`;
  if (diffMs < 7 * day) return `${Math.floor(diffMs / day)}d`;

  // Older than a week — show `Mon D` (e.g. `Apr 7`).
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  return `${months[then.getMonth()]} ${then.getDate()}`;
}
