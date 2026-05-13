import { join as joinPath } from "node:path";

const SLUG_RE = /[^a-z0-9_-]+/g;

function pad(n: number): string {
  return n.toString().padStart(2, "0");
}

function localIsoSeconds(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}-${pad(d.getMinutes())}-${pad(d.getSeconds())}`;
}

function slugify(name: string): string {
  const trimmed = name.trim().toLowerCase().replace(SLUG_RE, "-").replace(/^-+|-+$/g, "");
  return trimmed.length > 0 ? trimmed : "default";
}

/** Absolute path: <vault>/remargin_logs/runs/<ts>__<slug>.log */
export function submitLogPath(vaultPath: string, promptName: string | null, now?: Date): string {
  const ts = localIsoSeconds(now ?? new Date());
  const slug = slugify(promptName ?? "default");
  return joinPath(vaultPath, "remargin_logs", "runs", `${ts}__${slug}.log`);
}

/** Vault-relative path used to open the log in an Obsidian leaf. */
export function submitLogVaultPath(promptName: string | null, now?: Date): string {
  const ts = localIsoSeconds(now ?? new Date());
  const slug = slugify(promptName ?? "default");
  return `remargin_logs/runs/${ts}__${slug}.log`;
}
