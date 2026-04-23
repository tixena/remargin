/**
 * Subcommands that accept the `IdentityArgs` flag group on the CLI
 * (`--config`, `--identity`, `--type`, `--key`). `assembleExecArgs`
 * consults this set via its `identityAccepted` parameter to decide
 * whether to forward the flags built from plugin settings.
 *
 * `identity` landed here in rem-3dw0 so the plugin's read-path for `me`
 * resolves under the same flags the write-path uses — otherwise the
 * viewer's own acks look like "someone else acked" and the ack UI
 * branch flips (the rem-lcx regression surfaced via the inverted
 * AckToggle / AckButton behavior).
 *
 * Kept in its own file so unit tests can import it without pulling in
 * `RemarginBackend.ts`, whose constructor uses TypeScript parameter
 * properties that the test runner's strip-only loader cannot parse.
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
  "purge",
  "react",
  "rm",
  "sandbox",
  "sign",
  "verify",
  "write",
]);
