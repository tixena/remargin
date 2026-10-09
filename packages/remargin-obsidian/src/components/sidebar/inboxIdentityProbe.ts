/** Decides when the inbox re-probes the caller's identity. */

/**
 * Gate value for the inbox's identity probe, fed to the probe effect's
 * dependency array. `null` means "already resolved — never probe again",
 * so the healthy path spends exactly one CLI call per mount no matter how
 * many times `refreshKey` bumps (every ack/reply bumps it). While the
 * identity is still unresolved the value tracks `refreshKey`, so an
 * explicit user refresh re-probes.
 */
export function identityProbeKey(me: string | null, refreshKey: number | undefined): number | null {
  return me === null ? (refreshKey ?? 0) : null;
}
