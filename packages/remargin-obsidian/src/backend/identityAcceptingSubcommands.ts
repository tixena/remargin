/**
 * Subcommands that accept the `IdentityArgs` flag group on the CLI
 * (`--config`, `--identity`, `--type`, `--key`). `assembleExecArgs`
 * consults this set via its `identityAccepted` parameter to decide
 * whether to forward the flags built from plugin settings.
 *
 * `identity` is included so the read-path for `me` resolves under the
 * same flags the write-path uses — otherwise the viewer's own acks
 * look like "someone else acked" and the ack UI branch flips.
 *
 * Kept in its own file so unit tests can import it without pulling in
 * `RemarginBackend.ts`, whose constructor uses TypeScript parameter
 * properties the test runner's strip-only loader cannot parse.
 */
export const IDENTITY_ACCEPTING_SUBCOMMANDS = new Set([
  "ack",
  "batch",
  "comment",
  "delete",
  "edit",
  "identity",
  "migrate",
  "plan",
  "prompt",
  "purge",
  "react",
  "rm",
  "sandbox",
  "sign",
  "verify",
  "write",
]);
