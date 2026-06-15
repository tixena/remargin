import { z } from "zod/v4";

// Output-parsing for the CLI `--json` responses, extracted from
// `RemarginBackend` so it can be unit-tested directly: the backend class uses a
// parameter-property constructor that the test runner's strip-only loader
// cannot parse, so anything a test needs to import lives here instead.

/**
 * Parse CLI stdout against a Zod schema and surface a readable error on
 * validation failure so callers can tell the difference between a broken CLI
 * version and a transient runtime problem.
 */
export function parseEnvelope<T>(raw: string, schema: z.ZodType<T>, label: string): T {
  let payload: unknown;
  try {
    payload = JSON.parse(raw);
  } catch (err) {
    throw new Error(`remargin ${label}: could not parse JSON (${(err as Error).message})`);
  }
  const result = schema.safeParse(payload);
  if (!result.success) {
    throw new Error(`remargin ${label}: output did not match schema: ${result.error.message}`);
  }
  return result.data;
}

/**
 * Validate only an envelope's payload array against the strict generated
 * element schema, ignoring every envelope-level metadata key (`elapsed_ms`,
 * query's `base_path`, and anything added later). Element strictness is what
 * catches an un-modeled key like the `sl`/`el` graft; the envelope wrapper is
 * just a carrier and is deliberately not validated, so legitimate metadata can
 * never cause a false "did not match schema" rejection.
 */
export function parsePayloadArray<E>(
  raw: string,
  key: string,
  element: z.ZodType<E>,
  label: string
): E[] {
  let payload: unknown;
  try {
    payload = JSON.parse(raw);
  } catch (err) {
    throw new Error(`remargin ${label}: could not parse JSON (${(err as Error).message})`);
  }
  const arr = (payload as Record<string, unknown> | null)?.[key];
  const result = z.array(element).safeParse(arr);
  if (!result.success) {
    throw new Error(`remargin ${label}: output did not match schema: ${result.error.message}`);
  }
  return result.data;
}

// Loose envelope schemas for outputs not yet backed by a generated element
// schema (sandbox, registry). Tolerant by design until those outputs are typed.
export const SandboxListEntry$Schema = z.looseObject({
  path: z.string(),
  since: z.string(),
});

export const SandboxListEnvelope$Schema = z.looseObject({
  files: z.array(SandboxListEntry$Schema),
});

export const Participant$Schema = z.looseObject({
  name: z.string(),
  display_name: z.string(),
  type: z.enum(["human", "agent"]),
  status: z.enum(["active", "revoked"]),
  pubkeys: z.number(),
});

export const RegistryEnvelope$Schema = z.looseObject({
  participants: z.array(Participant$Schema),
});

export const SandboxRemoveEnvelope$Schema = z.looseObject({
  removed: z.array(z.string()).optional(),
  skipped: z.array(z.string()).optional(),
  failed: z
    .array(
      z.looseObject({
        path: z.string(),
        reason: z.string(),
      })
    )
    .optional(),
});
