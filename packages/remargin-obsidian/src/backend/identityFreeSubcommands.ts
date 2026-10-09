/** The gate deciding which CLI subcommands receive the identity flags. */

/**
 * Subcommands that reject identity flags. A wrong entry fails loud: the CLI errors on an
 * unexpected argument, never a silent identity switch.
 */
export const IDENTITY_FREE_SUBCOMMANDS = new Set(["resolve-mode", "obsidian", "registry"]);

/**
 * The exec gate: forward identity unless the subcommand is identity-free
 * or the invocation is a bare-flag probe like `--version`.
 */
export function acceptsIdentity(subcommand: string | undefined): boolean {
  return (
    subcommand !== undefined &&
    !subcommand.startsWith("-") &&
    !IDENTITY_FREE_SUBCOMMANDS.has(subcommand)
  );
}
