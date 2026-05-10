import { z } from "zod/v4";

const VerifyFailureRow$Schema = z.looseObject({
  checksum_ok: z.boolean(),
  id: z.string(),
  signature: z.string(),
});

const VerifyFailure$Schema = z.looseObject({
  error_kind: z.literal("verify_failed"),
  failures: z.array(VerifyFailureRow$Schema),
  headline: z.string(),
  hint: z.string(),
  mode: z.string(),
  path: z.string(),
});

export type VerifyFailure = z.infer<typeof VerifyFailure$Schema>;

/**
 * Try to read a verify-gate refusal out of an error message. The CLI's
 * `--json` mode emits the structured shape on stderr when the
 * post-write verify gate trips; the backend wraps stderr verbatim into
 * the rejection reason. This helper attempts to peel that envelope back
 * off so the UI can render a headline + hint instead of a blob.
 *
 * Returns `null` for any error that isn't a verify-failure shape
 * (network, parse, permissions, …) — caller falls back to the raw
 * message in that case.
 */
export function parseVerifyFailure(err: unknown): VerifyFailure | null {
  const message = readMessage(err);
  if (!message) return null;
  // The backend prepends nothing to the JSON, but the rejection includes a
  // trailing `\n  command: …` footer — strip the footer before parsing.
  const trimmed = stripCommandFooter(message);
  // Find the first balanced `{ … }` block; users sometimes see the JSON
  // preceded by a stray "error: " or a leading whitespace prefix.
  const candidate = extractJsonObject(trimmed);
  if (!candidate) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(candidate);
  } catch {
    return null;
  }
  const result = VerifyFailure$Schema.safeParse(parsed);
  return result.success ? result.data : null;
}

function readMessage(err: unknown): string | null {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return null;
}

function stripCommandFooter(message: string): string {
  const marker = "\n  command: ";
  const idx = message.indexOf(marker);
  return idx >= 0 ? message.slice(0, idx) : message;
}

function extractJsonObject(message: string): string | null {
  const start = message.indexOf("{");
  if (start < 0) return null;
  let depth = 0;
  let inString = false;
  let escape = false;
  for (let i = start; i < message.length; i++) {
    const ch = message[i];
    if (escape) {
      escape = false;
      continue;
    }
    if (ch === "\\") {
      escape = true;
      continue;
    }
    if (ch === '"') {
      inString = !inString;
      continue;
    }
    if (inString) continue;
    if (ch === "{") depth++;
    else if (ch === "}") {
      depth--;
      if (depth === 0) {
        return message.slice(start, i + 1);
      }
    }
  }
  return null;
}
